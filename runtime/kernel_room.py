#!/usr/bin/env python3
"""Persistent Python room host (ported from the existing kernel-room implementation).

Usage: room.py start|exec|vars|status|stop|interrupt|clean|kill ROOM [arguments]
       room.py list
Explicit LIBRARY_ROOM_HOME is required. A room executes trusted Python code: it is
not a sandbox. Run under a dedicated container account with read-only shelf mounts.
Snapshots contain trusted pickle data and must not come from untrusted users.
Optional LIBRARY_ROOM_TRANSPORT is an explicit command prefix receiving all CLI args.
A file ROOM.route in LIBRARY_ROOM_HOME forwards every command for that room to the command
prefix on its first line (a room running in its library's container). A later line
'socket DIR' sends exec, clean, vars, status, stop and interrupt straight to DIR/ROOM.sock,
skipping the command's start-up; start and kill still go through the command.
"""
import ast, contextlib, io, json, os, pickle, queue, re, shlex, signal, sys, tempfile, threading, time, traceback, types

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import room_channel, room_process  # noqa: E402  (what differs between Linux, macOS and Windows)

ROOT = os.environ.get("LIBRARY_ROOM_HOME", "")
MAX_ROOMS = int(os.environ.get("LIBRARY_ROOM_MAX", "16"))
MAX_OUTPUT = 64 * 1024                 # 셀 출력 상한(레퍼런스 동일)
SNAP_MAX_VAR = 16 * 1024 * 1024        # 변수 하나 상한(레퍼런스 동일)
SNAP_MAX_TOTAL = 256 * 1024 * 1024     # 방 전체 상한
SNAP_SYNC_UNDER = 0.05                 # 저장이 이보다 빠르면 실행 직후 저장, 느리면 디바운스
SNAP_DEBOUNCE, SNAP_MAX_LAG = 1.5, 30  # 마지막 변경 1.5s 뒤 저장, 계속 바쁘면 30s마다
GRACE = 2.0                            # 인터럽트 무시 시 강제 종료까지 유예
SECRET_RE = re.compile(r"(pass(word|wd)?|secret|token|api_?key|private_?key|credential|pat)$", re.I)
BASE = {"__name__", "__builtins__", "ROOM", "open"}


def validate(room):
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,47}", room):
        raise ValueError("invalid room name")
    if not ROOT or not os.path.isabs(ROOT):
        raise ValueError("set LIBRARY_ROOM_HOME to an explicit absolute writable directory")


def paths(room):
    validate(room)
    os.makedirs(ROOT, mode=0o700, exist_ok=True)
    b = os.path.join(ROOT, room)
    return {"sock": b + ".sock", "pid": b + ".pid", "log": b + ".log", "snap": b + ".snap.pkl", "man": b + ".snap.json"}


def route(room):
    """Command prefix of a room that runs elsewhere (its container: the route file's first line), or ''."""
    try:
        return (open(os.path.join(ROOT, room + ".route"), encoding="utf-8").read().strip().splitlines() or [""])[0].strip()
    except OSError:
        return ""


def route_socket(room):
    """DIR/ROOM.sock from the route file's 'socket DIR' line, or ''."""
    try:
        lines = open(os.path.join(ROOT, room + ".route"), encoding="utf-8").read().splitlines()[1:]
    except OSError:
        return ""
    folder = next((l.split(None, 1)[1].strip() for l in lines if l.startswith("socket ") and len(l.split(None, 1)) == 2), "")
    return os.path.join(os.path.expanduser(folder), room + ".sock") if folder else ""


def alive(room):
    try:
        return room_process.is_alive(int(open(paths(room)["pid"]).read()))
    except (OSError, ValueError):
        return False


def open_rooms():
    os.makedirs(ROOT, exist_ok=True)
    return [f[:-4] for f in os.listdir(ROOT) if f.endswith(".pid") and alive(f[:-4])]


# ── 스냅샷: 불을 꺼도 작업대 위 재료가 남는다 ─────────────────
def snapshot(ns, room):
    p = paths(room)
    payload, skipped, total = {}, [], 0
    for name in list(ns):
        if name.startswith("_") or name in BASE:
            continue
        v = ns.get(name)
        if isinstance(v, types.ModuleType):
            continue
        if SECRET_RE.search(name):
            skipped.append({"name": name, "reason": "secret name"}); continue
        try:
            blob = pickle.dumps(v, protocol=pickle.HIGHEST_PROTOCOL)
        except Exception as e:
            skipped.append({"name": name, "reason": f"{type(e).__name__}: {str(e)[:120]}"}); continue
        if len(blob) > SNAP_MAX_VAR:
            skipped.append({"name": name, "reason": f"exceeds per-variable cap ({len(blob)}B)"}); continue
        if total + len(blob) > SNAP_MAX_TOTAL:
            skipped.append({"name": name, "reason": "exceeds total cap"}); continue
        payload[name] = blob; total += len(blob)
    man = {"saved": sorted(payload), "skipped": skipped, "bytes": total, "time": time.strftime("%Y-%m-%d %H:%M:%S")}
    for target, data, mode in ((p["snap"], payload, "wb"), (p["man"], man, "w")):
        fd, tmp = tempfile.mkstemp(dir=ROOT, prefix=room + ".", suffix=".tmp")  # 임시→원자 교체
        with os.fdopen(fd, mode, **({} if mode == "wb" else {"encoding": "utf-8"})) as f:
            pickle.dump(data, f, protocol=pickle.HIGHEST_PROTOCOL) if mode == "wb" else json.dump(data, f, ensure_ascii=False)
        if mode == "wb" and os.path.exists(target):
            os.replace(target, target + ".prev")  # 직전 판 하나를 남긴다 — 잘못 덮어도 되돌릴 곳
        os.replace(tmp, target)
    return man


def restore(ns, room):
    p = paths(room)
    if not os.path.exists(p["snap"]):
        return None
    with open(p["snap"], "rb") as f:
        payload = pickle.load(f)
    restored, failed = [], []
    for name, blob in payload.items():
        try:
            ns[name] = pickle.loads(blob); restored.append(name)
        except Exception as e:
            failed.append({"name": name, "reason": f"{type(e).__name__}: {str(e)[:120]}"})
    return {"restored": restored, "failed": failed}


# ── 방(서버): 리더 스레드가 받고, 메인 스레드가 실행한다 ─────────
class _Capped(io.StringIO):
    """출력 상한: 넘치는 만큼은 버리고 잘렸다는 표시만 남긴다."""
    def __init__(self, cap):
        super().__init__(); self.cap = cap; self.dropped = 0

    def write(self, s):
        room = self.cap - self.tell()
        if room <= 0:
            self.dropped += len(s); return len(s)
        if len(s) > room:
            self.dropped += len(s) - room; s = s[:room]
        return super().write(s)

    def text(self):
        t = self.getvalue()
        return t + (f"\n[... output truncated at {self.cap} chars, {self.dropped} dropped ...]" if self.dropped else "")


def run_code(ns, code):
    """REPL처럼: 마지막 문장이 식이면 그 repr을 출력에 덧붙인다."""
    out = _Capped(MAX_OUTPUT)
    tree = ast.parse(code)
    last = ast.Expression(tree.body.pop().value) if tree.body and isinstance(tree.body[-1], ast.Expr) else None
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
        exec(compile(tree, "<room>", "exec"), ns)
        if last is not None:
            v = eval(compile(last, "<room>", "eval"), ns)
            if v is not None:
                ns["_"] = v; print(repr(v))
    return out.text()


class Room:
    def __init__(self, room, idle=0.0):
        self.room, self.idle, self.p = room, idle, paths(room)
        self.ns = {"__name__": "__room__", "ROOM": room, "open": self._open}
        self.q = queue.Queue()
        self.lock = threading.Lock()
        self.active = self.active_conn = None   # 실행 중인 셀 id·연결
        self.reads = {}                         # 방이 읽은 파일 → mtime (낡음 감지)
        self.started = self.last_used = self.last_saved = time.time()
        self.dirty, self.save_cost = False, 0.0
        self.log = open(self.p["log"], "a", encoding="utf-8")

    # 재료 낡음: 방 안 open()은 읽은 파일의 mtime을 기억한다
    def _open(self, file, mode="r", *a, **k):
        f = open(file, mode, *a, **k)
        if isinstance(file, (str, bytes, os.PathLike)) and not any(c in mode for c in "wax+"):
            try: self.reads[os.fspath(file)] = os.stat(file).st_mtime
            except OSError: pass
        return f

    def stale(self):
        out = []
        for path, mt in self.reads.items():
            try:
                if os.stat(path).st_mtime != mt: out.append(path)
            except OSError:
                out.append(path)
        return out

    def logln(self, msg):
        self.log.write(f"{time.strftime('%Y-%m-%d %H:%M:%S')} {msg}\n"); self.log.flush()

    def save(self, tag):
        if getattr(self, "keep_snap", None):  # 복원 못 한 방은 빈 작업대로 기존 스냅샷을 덮지 않는다
            if not self.keep_snap_logged:
                self.logln(f"snapshot[{tag}] 건너뜀 — {self.keep_snap} (기존 스냅샷 보존, 소유 계정으로 다시 열 것)")
                self.keep_snap_logged = True
            self.dirty, self.last_saved = False, time.time()
            return
        try:
            t = time.time(); m = snapshot(self.ns, self.room); self.save_cost = time.time() - t
            self.logln(f"snapshot[{tag}] saved={len(m['saved'])} skipped={len(m['skipped'])} {m['bytes']}B {self.save_cost:.3f}s")
        except Exception:
            self.logln(f"snapshot error {traceback.format_exc().splitlines()[-1]}")
        self.dirty, self.last_saved = False, time.time()

    def user_names(self):
        return sorted(k for k in self.ns if not k.startswith("_") and k not in BASE)

    # ── 중단: SIGINT는 실행 중인 셀에만 꽂힌다. 무시하면 유예 뒤 방을 강제 종료 ──
    def _sigint(self, signum, frame):
        if self.active is not None:
            raise KeyboardInterrupt

    def interrupt(self, rid=None, why="interrupt"):
        with self.lock:
            if self.active is None or (rid is not None and rid != self.active):
                return False
            rid = self.active
        room_process.interrupt_main(threading.main_thread().ident)
        t = threading.Timer(GRACE, self._force, args=(rid, why)); t.daemon = True; t.start()
        return True

    def _force(self, rid, why):
        with self.lock:
            if self.active != rid:
                return
            conn = self.active_conn
        self.logln(f"force kill: {why} ignored for {GRACE}s")
        self._reply(conn, {"ok": False, "out": f"room killed: {why} ignored for {GRACE:g}s (last snapshot kept)"})
        self._remove_files(); self.log.close()
        os._exit(137)

    def _reply(self, conn, resp):
        try: conn.sendall((json.dumps(resp) + "\n").encode())
        except OSError: pass
        conn.close()

    def _remove_files(self):
        for k in ("sock", "pid"):
            try: os.unlink(self.p[k])
            except OSError: pass

    # ── 리더 스레드: interrupt·status는 즉시, 나머지는 큐 ──
    def _reader(self, listener):
        while True:
            got = listener.accept()
            if got is None:
                continue
            conn, rfile = got
            try:
                req = json.loads(rfile.readline())
            except Exception:
                conn.close(); continue
            cmd = req.get("cmd", "exec")
            if cmd == "interrupt":
                self._reply(conn, {"ok": True, "out": "interrupted" if self.interrupt() else "idle (nothing running)"})
            elif cmd == "status":
                self._reply(conn, {"ok": True, "out": self.status_text()})
            else:
                self.q.put((conn, req))

    def status_text(self):
        st = "busy" if self.active is not None else "idle"
        stale = self.stale()
        s = (f"pid={os.getpid()} up={int(time.time()-self.started)}s {st} queued={self.q.qsize()} "
             f"vars={len(self.user_names())} rss={room_process.rss_mb(os.getpid())}MB stale={len(stale)}")
        return s + "".join(f"\n  stale: {f}" for f in stale)

    # ── 메인 스레드 ──
    def _exec(self, conn, req):
        code, timeout = req.get("code", ""), float(req.get("timeout") or os.environ.get("LIBRARY_ROOM_TIMEOUT") or 0)
        rid = f"{time.time():.6f}"
        with self.lock:
            self.active, self.active_conn = rid, conn
        timer = None
        if timeout > 0:
            timer = threading.Timer(timeout, self.interrupt, args=(rid, f"timeout {timeout:g}s")); timer.daemon = True; timer.start()
        t0 = time.time()
        try:
            resp = {"ok": True, "out": run_code(self.ns, code)}
        except KeyboardInterrupt:
            why = f"timeout {timeout:g}s" if timer and not timer.is_alive() else "interrupt"
            resp = {"ok": False, "out": f"KeyboardInterrupt ({why}) after {time.time()-t0:.1f}s"}
        except Exception:
            resp = {"ok": False, "out": traceback.format_exc()}
        finally:
            if timer: timer.cancel()
            with self.lock:
                self.active = self.active_conn = None
        if resp["ok"]:  # 작은 방은 바로 저장(kill -9 에도 안전), 큰 방은 디바운스(성능)
            self.save("exec") if self.save_cost < SNAP_SYNC_UNDER else setattr(self, "dirty", True)
        return resp

    def _clean(self, args):
        if args == ["--all"]:
            removed = [k for k in self.ns if k not in BASE]
            self.reads.clear()
        elif args:
            removed = [k for k in args if k in self.ns and k not in BASE]
        else:  # 껍질: 모듈·_접두
            removed = [k for k, v in self.ns.items() if k not in BASE and (k.startswith("_") or isinstance(v, types.ModuleType))]
        for k in removed:
            del self.ns[k]
        self.save("clean")
        return {"ok": True, "out": "removed: " + (" ".join(removed) or "(nothing)")}

    def serve(self):
        listener = room_channel.Listener(self.p["sock"])
        open(self.p["pid"], "w").write(str(os.getpid()))
        self.logln(f"start pid={os.getpid()} idle={self.idle:g}")
        try:
            r = restore(self.ns, self.room)
            if r: self.logln(f"restore {len(r['restored'])} failed={r['failed']}")
        except Exception:
            self.keep_snap, self.keep_snap_logged = "복원 실패: " + traceback.format_exc().splitlines()[-1], False
            self.logln(f"restore error {self.keep_snap}")
        signal.signal(signal.SIGINT, self._sigint)
        threading.Thread(target=self._reader, args=(listener,), daemon=True).start()
        while True:
            try:
                conn, req = self.q.get(timeout=1.0)
            except queue.Empty:
                now = time.time()
                if self.dirty and (now - self.last_used >= SNAP_DEBOUNCE or now - self.last_saved >= SNAP_MAX_LAG):
                    self.save("auto")
                if self.idle and now - self.last_used >= self.idle:
                    if self.dirty: self.save("idle")
                    self.logln("idle stop"); break
                continue
            except KeyboardInterrupt:
                continue  # 늦게 도착한 SIGINT: 셀이 없으면 무시
            cmd, t0 = req.get("cmd", "exec"), time.time()
            self.last_used = t0
            if cmd == "stop":
                if self.dirty: self.save("stop")
                resp = {"ok": True, "out": "bye"}
            elif cmd == "vars":
                resp = {"ok": True, "out": "\n".join(f"{k}: {type(self.ns[k]).__name__}" for k in self.user_names())}
            elif cmd == "clean":
                resp = self._clean(req.get("args", []))
            else:
                resp = self._exec(conn, req)
            self.last_used = time.time()
            self.logln(f"{cmd} ok={resp['ok']} {self.last_used-t0:.3f}s")
            self._reply(conn, resp)
            if cmd == "stop":
                break
        listener.close(); self._remove_files(); self.logln("stop"); self.log.close()


def serve(room, idle):
    """Run the host. An error that ends it is appended to ROOM.log, where start shows it."""
    try:
        Room(room, idle).serve()
    except Exception:
        with open(paths(room)["log"], "a", encoding="utf-8") as f:
            f.write(f"{time.strftime('%Y-%m-%d %H:%M:%S')} host error\n{traceback.format_exc()}")
        raise


def log_tail(room, lines=6):
    try:
        return "".join(open(paths(room)["log"], encoding="utf-8", errors="replace").readlines()[-lines:])
    except OSError:
        return ""


# ── 클라이언트 ────────────────────────────────────────────
def start(room, idle):
    p = paths(room)
    if alive(room):
        print(f"already open: {room} pid={open(p['pid']).read()}"); return
    rooms = open_rooms()
    if len(rooms) >= MAX_ROOMS:
        print(f"room limit {MAX_ROOMS} reached ({', '.join(rooms)}). stop one first.", file=sys.stderr); sys.exit(1)
    for k in ("sock", "pid"):  # 죽은 방의 잔해
        try: os.unlink(p[k])
        except OSError: pass
    room_process.spawn_host(lambda: serve(room, idle), [os.path.abspath(__file__), "serve", room, str(idle)])
    for _ in range(50):
        if os.path.exists(p["sock"]) and os.path.exists(p["pid"]):
            print(f"open: {room} pid={open(p['pid']).read()}"); return
        time.sleep(0.1)
    print(f"failed to open {room}\n{log_tail(room)}".rstrip(), file=sys.stderr); sys.exit(1)


def call(room, req, sock=None):
    """Connect to the selected endpoint, which may belong to a room in another process namespace."""
    p = paths(room)
    try:
        c = room_channel.connect(sock or p["sock"], float(req.get("timeout") or 30) + GRACE + 5)
    except (FileNotFoundError, ConnectionRefusedError):
        print(f"room closed: {room}  (room.py start {room})", file=sys.stderr); sys.exit(2)
    except PermissionError as error:
        print(f"room access denied: {room} ({error})", file=sys.stderr); sys.exit(3)
    except OSError as error:
        print(f"room connection failed: {room} ({error})", file=sys.stderr); sys.exit(1)
    with c, c.makefile("rwb") as f:
        f.write((json.dumps(req) + "\n").encode()); f.flush()
        line = f.readline()
    if not line:
        print(f"room died: {room} (see {p['log']}); room.py start {room} restores the last snapshot", file=sys.stderr); sys.exit(1)
    resp = json.loads(line)
    out = resp["out"]
    sys.stdout.write(out if not out or out.endswith("\n") else out + "\n")
    if req.get("cmd") == "stop":
        for _ in range(100):
            if not os.path.exists(sock or p["sock"]): break
            time.sleep(.01)
    sys.exit(0 if resp["ok"] else 1)


def kill(room):
    p = paths(room)
    try:
        room_process.terminate(int(open(p["pid"]).read())); print(f"killed: {room}")
    except Exception:
        print(f"not running: {room}")
    for k in ("sock", "pid"):
        try: os.unlink(p[k])
        except OSError: pass


def list_rooms():
    os.makedirs(ROOT, exist_ok=True)
    names = sorted({f.split(".")[0] for f in os.listdir(ROOT) if f.endswith((".pid", ".snap.json", ".route"))})
    for r in names:
        p = paths(r)
        if route(r):
            print(f"{r:<20} {'route':<14} {'-':>7}  {route(r)}"); continue
        if alive(r):
            pid = int(open(p["pid"]).read()); st, rss = "open", f"{room_process.rss_mb(pid)}MB"
        else:
            st, rss = ("off(snapshot)" if os.path.exists(p["snap"]) else "stale"), "-"
        try:
            m = json.load(open(p["man"], encoding="utf-8")); info = f"{len(m['saved'])}vars  {m['bytes']//1024}KB  snap {m['time']}"
        except Exception:
            info = "-"
        print(f"{r:<20} {st:<14} {rss:>7}  {info}")


def forward(command, args):
    if os.name == "nt":
        # Windows exec does not propagate the child exit code to the waiting caller.
        import subprocess
        parts = shlex.split(command, posix=False)
        words = [p[1:-1] if len(p) > 1 and p[0] == p[-1] and p[0] in "\'\"" else p for p in parts]
        sys.exit(subprocess.call([*words, *args]))
    words = shlex.split(command)
    os.execvp(words[0], [*words, *args])


def main(argv):
    transport = os.environ.get("LIBRARY_ROOM_TRANSPORT")
    if transport:
        forward(transport, argv[1:])
    if len(argv) < 2 or argv[1] in ("-h", "--help"):
        print(__doc__.strip()); return
    cmd, rest = argv[1], argv[2:]
    if cmd == "list":
        validate("list")
        return list_rooms()
    if not rest:
        print(__doc__.strip()); sys.exit(1)
    room, args = rest[0], rest[1:]
    validate(room)
    via = route(room)
    sock = route_socket(room) if via else ""
    if via and not (sock and cmd in ("exec", "clean", "vars", "status", "stop", "interrupt")):
        forward(via, [cmd, room, *args])
    if cmd == "start":
        idle = float(args[1]) if args[:1] == ["--idle"] else float(os.environ.get("LIBRARY_ROOM_IDLE") or 0)
        start(room, idle)
    elif cmd == "serve":  # the host itself, started by room_process.spawn_host where there is no fork
        serve(room, float(args[0]) if args else 0.0)
    elif cmd == "exec":
        timeout = None
        if args[:1] == ["--timeout"]:
            timeout, args = float(args[1]), args[2:]
        call(room, {"cmd": "exec", "code": args[0] if args else sys.stdin.read(), "timeout": timeout}, sock)
    elif cmd == "clean":
        call(room, {"cmd": "clean", "args": args}, sock)
    elif cmd in ("vars", "status", "stop", "interrupt"):
        call(room, {"cmd": cmd}, sock)
    elif cmd == "kill":
        kill(room)
    else:
        print(__doc__.strip()); sys.exit(1)


if __name__ == "__main__":
    for stream in (sys.stdin, sys.stdout, sys.stderr):  # UTF-8 whatever the system encoding (English Windows: cp1252)
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8")
    try:
        main(sys.argv)
    except (ValueError, OSError) as error:
        print(f"room error: {error}", file=sys.stderr)
        sys.exit(2)
