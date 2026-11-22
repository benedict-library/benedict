#!/usr/bin/env python3
"""Run bounded librarian cycles: prebuild index, sync rooms, measure, and journal results.

--clean instead tidies the rooms' named variables by the registry's `clean` choice (true = the rules below, or a map
of reading/meeting/librarian to rules): a room keeps its `keep` names and names starting with its `prefixes`, except
dated working names (`_YYMMDD` older than expire_days, prefixes in expire_prefixes) — those and any other name are
first dumped as JSON (<registry dir>/.library-state/room_dump/<room>/<YYMMDD>/, or --dump-dir as the room sees it)
and then removed. With unknown "report", undated names outside the prefixes are kept and counted. `shells` also
removes modules and _ names. ROOM_ names (the room's settings) stay whatever the rules say. The search room is never
cleaned (its index lives in _ names). One journal line follows.
copies ({name: {"file": path} or {"command": "prints JSON"}, optional timeout}): after the sync each value is set as
one search-room variable if the snapshot's per-variable cap can keep it; a failing copy keeps the old value, fails the
cycle, and the journal line "사본 올리기" names every result.
--clean then runs the registry's `mail_reap` and `restart` (after: clean) choices; --reap and --restart run one alone.

mail_reap (true or {ttl_days: 7, export_dir}): meeting-room mailbox topics whose last reply is older than ttl_days are
written one JSON file per topic (<dump>/<meeting room>/<YYMMDD>/mail/), checked by size, then removed from `mail`.
restart (true or {rooms: [reading], after: clean|cycle}): each room is stopped and started again so Python hands its
freed memory back to the system. The snapshot keeps what it can; variables it cannot keep (over the per-variable cap)
are dumped as JSON first — a variable neither can keep stops the restart before anything happens — and a full sync
refills the rooms' documents and index. Variable count and memory before and after go into one journal line.
journal_db (true or {dsn_env: LIBRARY_JOURNAL_DSN, table: library.journal}, see journal.sql): every journal line is
inserted into the table first with psql (LIBRARY_PSQL, default psql); the room's `journal` holds the copy. A failing
insert fails the step — the line is not quietly left in the room only.
Choices not set say "not configured".
"""
import argparse
import datetime
import json
import os
import pickle
import re
from pathlib import Path
import shlex
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from library_local import Library
import room_sync
import kernel_room  # noqa: E402  (room_sync puts runtime/ on the path; the snapshot's caps and secret-name rule)

CLEAN_DEFAULT = {
    "reading": {"keep": ["docs", "sync_meta", "ROOM_DOC_ROOT", "ROOM_CARD_PREFIXES"], "prefixes": ["work_", "reading_"],
                "shells": True, "expire_days": 7},
    "meeting": {"keep": ["mail"], "prefixes": ["task_", "work_", "reading_"], "shells": True, "expire_days": 7,
                "unknown": "report"},
    "librarian": {"keep": ["journal"], "prefixes": [], "shells": True},
}
NAMES_CODE = "import json as _j\nprint(_j.dumps(sorted(k for k in globals() if not k.startswith('__'))))\ndel _j\n"
BASE = ("ROOM", "open")
SETTING = "ROOM_"   # a room's settings (the sync's ROOM_DOC_ROOT, ROOM_CARD_PREFIXES, ...) — kept whatever the rules say


def kept(rule, n):
    return n.startswith(("_", SETTING)) or n in rule.get("keep", []) or n in BASE


def dated(name):
    m = re.search(r"_(\d{6})(?:_|$)", name)
    try:
        return datetime.date(2000 + int(m.group(1)[:2]), int(m.group(1)[2:4]), int(m.group(1)[4:6])) if m else None
    except ValueError:
        return None


def clean_targets(rule, names, today):
    """Named variables to dump and remove under one room rule (_ names and modules are the shell step's)."""
    out = []
    for n in names:
        if kept(rule, n):
            continue
        d = dated(n)
        expired = d is not None and (today - d).days > rule.get("expire_days", 10 ** 6)
        if n.startswith(tuple(rule.get("prefixes", []))):
            if not (expired and n.startswith(tuple(rule.get("expire_prefixes", ["task_", "work_", "reading_"])))):
                continue
        elif rule.get("unknown") == "report" and not expired and len(n) > 2:
            continue
        out.append(n)
    return out


def unknown_names(rule, names):
    if rule.get("unknown") != "report":
        return []
    return [n for n in names if not kept(rule, n)
            and not n.startswith(tuple(rule.get("prefixes", []))) and len(n) > 2 and dated(n) is None]


def dump_code(folder, names):
    """Room code writing each name as JSON (else repr) into folder before removal — the way back."""
    return ("import json as _j, os as _o\n"
            f"_d = {str(folder)!r}\n_o.makedirs(_d, exist_ok=True)\n"
            f"for _n in {list(names)!r}:\n"
            "    if _n not in globals(): continue\n"
            "    try: open(_d + '/' + _n + '.json', 'w', encoding='utf-8').write(_j.dumps(globals()[_n], ensure_ascii=False, default=str))\n"
            "    except Exception: open(_d + '/' + _n + '.repr.txt', 'w', encoding='utf-8').write(repr(globals()[_n])[:2000000])\n"
            "print(_d)\ndel _j, _o, _d, _n\n")


def clean(config, key=None, prefix=None, dump_dir=None, dry=False, today=None):
    lib = Library(config)
    key, row, _ = lib.selected(key)
    choice = row.get("clean", False)
    if not choice:
        return {"library": key, "clean": "not configured"}
    rules = CLEAN_DEFAULT if choice is True else choice
    names = room_sync.room_names(lib, key, prefix)
    today = today or datetime.date.today()
    dump = Path(dump_dir) if dump_dir else lib.config.parent / ".library-state" / "room_dump"
    parts = []
    for kind, rule in rules.items():
        room = names[kind]
        found = json.loads(room_sync.room_command("exec", room, NAMES_CODE).strip().splitlines()[-1])
        targets, unknown = clean_targets(rule, found, today), unknown_names(rule, found)
        if dry:
            parts.append(f"{room} 이름 {len(targets)}개(dry-run: {' '.join(targets[:8])})")
            continue
        if targets:
            room_sync.room_command("exec", room, dump_code(dump / room / f"{today:%y%m%d}", targets))
            room_sync.room_command("clean", room, args=targets)
        shells = room_sync.room_command("clean", room).strip() if rule.get("shells") else "(nothing)"
        n_shell = 0 if shells.endswith("(nothing)") else len(shells.split()) - 1
        parts.append(f"{room} 이름 {len(targets)} · 껍질 {n_shell}" + (f" · 규칙 밖 {len(unknown)}" if unknown else ""))
    result = " · ".join(parts)
    if not dry:
        write_journal(row, names["librarian"], line("변수 정리", ",".join(names[k] for k in rules), result))
    return {"library": key, "clean": "dry-run" if dry else "done", "result": result}


def line(kind, target, result):
    return {"kind": kind, "target": target, "result": result, "at": datetime.datetime.now().isoformat(timespec="seconds")}


def aware(at):
    """An ISO time with its offset (a time without one is this machine's local time)."""
    t = datetime.datetime.fromisoformat(at)
    return (t if t.tzinfo else t.astimezone()).isoformat()


PG_PARAMS = {"host": "PGHOST", "port": "PGPORT", "user": "PGUSER", "password": "PGPASSWORD", "dbname": "PGDATABASE",
             "sslmode": "PGSSLMODE", "application_name": "PGAPPNAME", "connect_timeout": "PGCONNECT_TIMEOUT"}


def pg_env(dsn):
    """libpq environment variables from a postgresql:// URI — so the password never appears on psql's command line."""
    from urllib.parse import parse_qsl, unquote, urlsplit
    u = urlsplit(dsn)
    if u.scheme not in ("postgresql", "postgres"):
        raise RuntimeError("journal_db DSN must be a postgresql:// URI")
    env = {}
    if u.username:
        env["PGUSER"] = unquote(u.username)
    if u.password:
        env["PGPASSWORD"] = unquote(u.password)
    if u.hostname:
        env["PGHOST"] = u.hostname
    if u.port:
        env["PGPORT"] = str(u.port)
    if u.path.strip("/"):
        env["PGDATABASE"] = unquote(u.path.lstrip("/"))
    for k, v in parse_qsl(u.query):
        if k in PG_PARAMS:
            env[PG_PARAMS[k]] = v
    return env


def journal_db_write(choice, room, kind, target, result, at):
    """Insert one journal line with psql; values travel as psql variables (:'name'), never inside the SQL text."""
    rule = {} if choice is True else choice
    env = rule.get("dsn_env", "LIBRARY_JOURNAL_DSN")
    dsn = os.environ.get(env)
    if not dsn:
        raise RuntimeError(f"journal_db is chosen but {env} is not set")
    sql = (f"INSERT INTO {rule.get('table', 'library.journal')} (at, room, kind, target, result) "
           "VALUES (:'at', :'room', :'kind', :'target', :'result');\n")
    args = [*shlex.split(os.environ.get("LIBRARY_PSQL") or "psql"), "-X", "-q", "-v", "ON_ERROR_STOP=1"]
    for name, value in (("at", aware(at)), ("room", room), ("kind", kind), ("target", target), ("result", result)):
        args += ["-v", f"{name}={value}"]
    done = subprocess.run(args, input=sql, text=True, encoding="utf-8", capture_output=True, timeout=30,
                          env={**os.environ, **pg_env(dsn)})
    if done.returncode:
        raise RuntimeError("journal_db insert failed: " + (done.stderr or done.stdout).strip()[:300])


def write_journal(row, room, entry, db_line=None):
    """The database first when the row chooses journal_db (a failure stops here), then the copy in the room's journal.
    db_line = (kind, target, result, at) when the room entry is not itself one line (a cycle record)."""
    if row.get("journal_db"):
        journal_db_write(row["journal_db"], room, *(db_line or (entry["kind"], entry["target"], entry["result"], entry["at"])))
    room_sync.room_command("exec", room, f"journal = globals().get('journal') or []\njournal.append({entry!r})\n"
                           "del journal[:-2000]\nprint(len(journal))\n")


REAP_BODY = r'''
def _reap(folder, ttl):
    import json, os, datetime, hashlib, re
    mail = globals().setdefault('mail', {})
    cut = datetime.datetime.now() - datetime.timedelta(days=ttl)
    out = {'reaped': [], 'kept': 0, 'undated': 0, 'mismatch': []}
    for topic in list(mail):
        entries = mail[topic]
        try:
            at = datetime.datetime.fromisoformat(entries[-1]['at'])
            if at.tzinfo:
                at = at.astimezone().replace(tzinfo=None)
        except Exception:
            out['undated'] += 1
            continue
        if at >= cut:
            out['kept'] += 1
            continue
        data = json.dumps({'topic': topic, 'entries': entries}, ensure_ascii=False, default=str).encode('utf-8')
        name = (re.sub(r'[^\w.-]+', '_', str(topic))[:80] or 'topic') + '-' + hashlib.sha1(str(topic).encode('utf-8')).hexdigest()[:8]
        os.makedirs(folder, exist_ok=True)
        path = os.path.join(folder, name + '.json')
        with open(path, 'wb') as f:
            f.write(data)
        if os.path.getsize(path) == len(data):
            del mail[topic]
            out['reaped'].append(path)
        else:
            out['mismatch'].append(path)
    print(json.dumps(out, ensure_ascii=False))
_reap(**_reap_args)
del _reap, _reap_args
'''


def dump_base(lib, dump_dir, setting=None):
    if setting:
        return Path(setting) if Path(setting).is_absolute() else lib.config.parent / setting
    return Path(dump_dir) if dump_dir else lib.config.parent / ".library-state" / "room_dump"


def reap(config, key=None, prefix=None, dump_dir=None, today=None):
    """Export then remove mailbox topics whose last reply is older than the registry's mail_reap ttl_days."""
    lib = Library(config)
    key, row, _ = lib.selected(key)
    choice = row.get("mail_reap", False)
    if not choice:
        return {"library": key, "reap": "not configured"}
    rule = {} if choice is True else choice
    names = room_sync.room_names(lib, key, prefix)
    room = names["meeting"]
    folder = dump_base(lib, dump_dir, rule.get("export_dir")) / room / f"{today or datetime.date.today():%y%m%d}" / "mail"
    code = f"_reap_args = {{'folder': {str(folder)!r}, 'ttl': {float(rule.get('ttl_days', 7))!r}}}\n" + REAP_BODY
    out = json.loads(room_sync.room_command("exec", room, code).strip().splitlines()[-1])
    result = (f"{room} 회신 주제 {len(out['reaped'])}개 내보내고 뺌 · 남김 {out['kept']}"
              + (f" · 날짜 없음 {out['undated']}" if out["undated"] else "")
              + (f" · 크기 다름 {len(out['mismatch'])}(남겨 둠)" if out["mismatch"] else ""))
    write_journal(row, names["librarian"], line("회신함 정리", room, result))
    if out["mismatch"]:
        raise RuntimeError(f"{room}: exported size differs, topics kept: {out['mismatch']}")
    return {"library": key, "reap": "done", "result": result, "files": out["reaped"]}


KEEP_BODY = r'''
def _keep(folder, cap, total_cap, secret, refill):
    import json, os, pickle, re, types
    ns, total, kept, dumped, lost = globals(), 0, [], [], []
    for name in list(ns):
        value = ns.get(name)
        if name.startswith('_') or name in ('ROOM', 'open') or isinstance(value, types.ModuleType):
            continue
        why = 'secret name' if re.search(secret, name, re.I) else None
        if why is None:
            try:
                size = len(pickle.dumps(value, protocol=pickle.HIGHEST_PROTOCOL))
                why = 'over cap' if size > cap or total + size > total_cap else None
            except Exception:
                why = 'cannot pickle'
        if why is None:
            total += size
            kept.append(name)
            continue
        if name in refill:
            continue
        if why != 'secret name':
            try:
                data = json.dumps(value, ensure_ascii=False).encode('utf-8')
                os.makedirs(folder, exist_ok=True)
                with open(os.path.join(folder, name + '.json'), 'wb') as f:
                    f.write(data)
                dumped.append(name)
                continue
            except (TypeError, ValueError):
                pass
        lost.append(name + ' (' + why + ')')
    print(json.dumps({'kept': kept, 'dumped': dumped, 'lost': lost}))
_keep(**_keep_args)
del _keep, _keep_args
'''
RESTART_DEFAULT = {"rooms": ["reading"], "after": "clean"}
REFILLED = {"docs", "sync_meta", "ROOM_DOC_ROOT", "ROOM_CARD_PREFIXES", "ROOM_READS", "library_registry", "ROOM_SECTIONS",
            "library_terms", "rrf_weights"}   # what a full sync writes again


def room_state(room):
    """(variables, memory MB or '?') from the room's status line."""
    m = re.search(r"vars=(\d+) rss=(\S+?)MB", room_sync.room_command("status", room))
    return (int(m.group(1)), m.group(2) if m.group(2).isdigit() else "?") if m else ("?", "?")


START_TRIES = 3


def start_again(room, tries=START_TRIES, wait=2.0):
    """Start a stopped room, trying again after a failure; returns the try that worked, else says the room is down."""
    error = None
    for n in range(1, tries + 1):
        try:
            room_sync.room_command("start", room)
            room_sync.room_command("status", room)
            return n
        except RuntimeError as e:
            error = e
            time.sleep(wait * n)
    raise RuntimeError(f"{room}: stopped but did not start again after {tries} tries — the room is down: {error}")


def restart(config, key=None, prefix=None, dump_dir=None, today=None):
    """Stop and start the chosen rooms (memory back to the system), keeping their variables, then refill in full."""
    lib = Library(config)
    key, row, _ = lib.selected(key)
    choice = row.get("restart", False)
    if not choice:
        return {"library": key, "restart": "not configured"}
    rule = {**RESTART_DEFAULT, **({} if choice is True else choice)}
    names = room_sync.room_names(lib, key, prefix)
    stamp = f"{today or datetime.date.today():%y%m%d}"
    before, dumped, down, tries = {}, {}, 0.0, {}
    for kind in rule["rooms"]:
        room = names[kind]
        before[room] = room_state(room)
        args = {"folder": str(dump_base(lib, dump_dir) / room / stamp), "cap": kernel_room.SNAP_MAX_VAR,
                "total_cap": kernel_room.SNAP_MAX_TOTAL, "secret": kernel_room.SECRET_RE.pattern, "refill": sorted(REFILLED)}
        out = json.loads(room_sync.room_command("exec", room, f"_keep_args = {args!r}\n" + KEEP_BODY).strip().splitlines()[-1])
        if out["lost"]:
            raise RuntimeError(f"{room}: restart would lose {', '.join(out['lost'])} — neither the snapshot nor a JSON "
                               "dump keeps them; remove or convert them first (nothing was restarted)")
        dumped[room] = out["dumped"]
        t = time.time()
        room_sync.room_command("stop", room)
        try:
            tries[room] = start_again(room)
        except RuntimeError as e:
            write_journal(row, names["librarian"], line("다시 띄우기 실패", room, str(e)[:300]))
            raise
        down += time.time() - t
        found = set(json.loads(room_sync.room_command("exec", room, NAMES_CODE).strip().splitlines()[-1]))
        missing = sorted(set(out["kept"]) - found)
        if missing:
            raise RuntimeError(f"{room}: started again without {', '.join(missing)} (see the room log; the previous "
                               "snapshot is kept as .prev)")
    t = time.time()
    synced = room_sync.synchronize(config, key, prefix, full=True)
    fill = time.time() - t
    parts = []
    for room, (n0, m0) in before.items():
        n1, m1 = room_state(room)
        parts.append(f"{room} 변수 {n0}→{n1} · 메모리 {m0}→{m1}MB"
                     + (f" · 상한 밖 덤프 {', '.join(dumped[room])}" if dumped[room] else "")
                     + (f" · 시작 {tries[room]}번째" if tries[room] > 1 else ""))
    result = " · ".join(parts) + f" · 끊김 {down:.1f}초 · 채움 {fill:.0f}초(문서 {synced['documents']})"
    write_journal(row, names["librarian"], line("다시 띄우기", ",".join(before), result))
    return {"library": key, "restart": "done", "result": result, "dumped": dumped}


def tidy_rooms(config, key=None, prefix=None, dump_dir=None, dry=False):
    """--clean: variable clean-up, then mailbox reaping, then the restart chosen to follow the clean-up."""
    out = clean(config, key, prefix, dump_dir, dry)
    if dry:
        out["reap"] = out["restart"] = "dry-run: not run"
        return out
    out["reap"] = reap(config, key, prefix, dump_dir)
    _, row, _ = Library(config).selected(key)
    chosen = row.get("restart", False)
    after = (RESTART_DEFAULT if chosen is True else {**RESTART_DEFAULT, **(chosen or {})})["after"]
    out["restart"] = restart(config, key, prefix, dump_dir) if chosen and after == "clean" else (
        "not configured" if not chosen else f"after {after}")
    return out


SOURCE_DIRS_DEFAULT = ["entities", "concepts"]   # encyclopedia pages cite their shelf sources; saved answers (queries) need not


def front_matter(text):
    """Front matter key -> raw value (simple `key: value` lines), or {} without a leading --- block."""
    if not text.startswith("---\n"):
        return {}
    end = text.find("\n---", 4)
    if end == -1:
        return {}
    out = {}
    for line in text[4:end].splitlines():
        if ":" in line and not line.startswith((" ", "-")):
            k, v = line.split(":", 1)
            out[k.strip()] = v.strip().strip("'\"")
    return out


def page_title(text):
    title = front_matter(text).get("title")
    if title:
        return title
    for line in text.splitlines():
        if line.startswith("# "):
            return line[2:].strip()
    return None


def tidy(config, key=None):
    """Rule checks a librarian can run without judgment: encyclopedia pages without `sources`, one title on several
    pages, and the registry row's forbidden_terms found in pages. Report only — fixing is the librarians' team's work."""
    lib = Library(config)
    key, row, _ = lib.selected(key)
    _, docs = lib.documents(key)
    source_dirs = tuple(d.rstrip("/") + "/" for d in row.get("source_dirs", SOURCE_DIRS_DEFAULT))
    forbidden = [t for t in row.get("forbidden_terms", []) if isinstance(t, str) and t]
    missing, titles, found = [], {}, {}
    for path in sorted(docs):
        text = docs[path]
        if path.startswith(source_dirs) and not front_matter(text).get("sources"):
            missing.append(path)
        title = page_title(text)
        if title:
            titles.setdefault(title, []).append(path)
        for term in forbidden:
            if term in text:
                found.setdefault(term, []).append(path)
    return {"documents": len(docs), "missing_sources": missing,
            "duplicate_titles": {t: ps for t, ps in sorted(titles.items()) if len(ps) > 1},
            "forbidden_terms": found}


COPY_TIMEOUT = 300


def copy_value(lib, spec):
    """The JSON value of one copy: a file (relative to the registry), or what a command prints (run in the registry's folder)."""
    if "file" in spec:
        path = Path(spec["file"]) if Path(spec["file"]).is_absolute() else lib.config.parent / spec["file"]
        return json.loads(path.read_text(encoding="utf-8"))
    done = subprocess.run(shlex.split(spec["command"]), cwd=lib.config.parent, text=True, encoding="utf-8",
                          capture_output=True, timeout=spec.get("timeout", COPY_TIMEOUT))
    if done.returncode:
        raise RuntimeError(f"command exit {done.returncode}: {(done.stderr or done.stdout).strip()[-200:]}")
    return json.loads(done.stdout)


def put_copies(lib, row, names):
    """The registry's `copies`: each value read and set as one search-room variable, if the snapshot can keep it.
    A failing copy leaves that variable as it was; the others still go up, and one journal line names every result."""
    choice = row.get("copies") or {}
    if not choice:
        return {"copies": "not configured", "failed": []}
    room, parts, failed = names["search"], [], []
    for name, spec in sorted(choice.items()):
        try:
            value = copy_value(lib, spec)
            size = len(pickle.dumps(value, protocol=pickle.HIGHEST_PROTOCOL))
            if size > kernel_room.SNAP_MAX_VAR:
                raise ValueError(f"{size} bytes, over the per-variable snapshot cap {kernel_room.SNAP_MAX_VAR}")
            room_sync.room_command("exec", room, f"import base64, json, zlib\n{name} = {room_sync.packed(value)}\n")
            count = f"{len(value)}행" if isinstance(value, list) else f"{len(value)}키" if isinstance(value, dict) else "값"
            parts.append(f"{name} {count} {size / 1024:.0f}KB")
        except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
            failed.append(name)
            parts.append(f"{name} 실패: {str(error)[:160]}")
    result = " · ".join(parts)
    write_journal(row, names["librarian"], line("사본 올리기", room, result))
    return {"room": room, "result": result, "failed": failed}


def utc():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def cycle(config, key=None, golden=None, backup_dir=None):
    started = utc()
    lib = Library(config)
    key, row, _ = lib.selected(key)
    record = {"library": key, "started": started, "steps": {}, "status": "ok"}
    exit_code = 0
    try:
        record["steps"]["index"] = lib.index(key)
        synced = room_sync.synchronize(config, key)
        record["steps"]["sync"] = synced
        record["steps"]["copies"] = put_copies(lib, row, synced["rooms"])
        found = tidy(config, key)
        record["steps"]["tidy"] = {"documents": found["documents"], "missing_sources": len(found["missing_sources"]),
                                   "duplicate_titles": len(found["duplicate_titles"]),
                                   "forbidden_terms": sum(len(v) for v in found["forbidden_terms"].values()),
                                   "examples": found["missing_sources"][:5] + sorted(found["duplicate_titles"])[:5]}
        if golden:
            measured = subprocess.run([sys.executable, str(Path(__file__).with_name("evaluate.py")),
                                       "--config", str(config), "--library", key, "--golden", str(golden)],
                                      text=True, encoding="utf-8", capture_output=True, timeout=120)
            record["steps"]["golden"] = {"exit": measured.returncode,
                                                "report": json.loads(measured.stdout) if measured.stdout.strip() else None}
            if measured.returncode not in (0, 1):
                raise ValueError("golden measurement failed: " + measured.stderr.strip()[:200])
            if measured.returncode:
                record["status"], exit_code = "warning", 1
        else:
            record["steps"]["golden"] = {"status": "not_configured"}
            record["status"], exit_code = "warning", 1
        if backup_dir:
            import wiki_backup
            stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
            record["steps"]["backup"] = wiki_backup.snapshot(config, key, Path(backup_dir) / (stamp + ".tar"))
        chosen = row.get("restart", False)
        if chosen and {**RESTART_DEFAULT, **({} if chosen is True else chosen)}["after"] == "cycle":
            record["steps"]["restart"] = restart(config, key)
        if record["steps"]["copies"]["failed"]:
            raise RuntimeError("copies failed: " + record["steps"]["copies"]["result"])
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        record["status"], exit_code = "failed", 2
        record["error"] = str(error)
    record["ended"] = utc()
    state = lib.config.parent / ".library-state"
    state.mkdir(parents=True, exist_ok=True)
    with (state / "librarian.jsonl").open("a", encoding="utf-8") as out:
        out.write(json.dumps(record, ensure_ascii=False) + "\n")
    if "sync" in record["steps"]:
        journal_room = record["steps"]["sync"]["rooms"]["librarian"]
        try:
            write_journal(row, journal_room, record, ("사서 한 바퀴", key, json.dumps(record, ensure_ascii=False), record["ended"]))
        except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
            record["journal_error"] = str(error)
            record["status"], exit_code = "failed", 2
    return record, exit_code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", required=True)
    parser.add_argument("--library")
    parser.add_argument("--golden")
    parser.add_argument("--backup-dir")
    parser.add_argument("--clean", action="store_true",
                        help="tidy room variables by the registry's clean choice, reap the mailbox, restart (after: clean), then stop")
    parser.add_argument("--reap", action="store_true", help="only the mailbox reaping (mail_reap), then stop")
    parser.add_argument("--restart", action="store_true", help="only the room restart (restart), then stop")
    parser.add_argument("--dry-run", action="store_true", help="with --clean: list what would go")
    parser.add_argument("--dump-dir", help="with --clean/--reap/--restart: dump folder as the rooms see it")
    parser.add_argument("--prefix", help="room name prefix (default: the library key)")
    parser.add_argument("--iterations", type=int, default=1)
    parser.add_argument("--interval", type=float, default=86400)
    args = parser.parse_args()
    if args.clean + args.reap + args.restart > 1:
        parser.error("--clean, --reap and --restart do not go together")
    if args.clean or args.reap or args.restart:
        try:
            if args.clean:
                out = tidy_rooms(args.config, args.library, args.prefix, args.dump_dir, args.dry_run)
            else:
                out = (reap if args.reap else restart)(args.config, args.library, args.prefix, args.dump_dir)
            print(json.dumps(out, ensure_ascii=False))
            return 0
        except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
            print(json.dumps({"status": "failed", "error": str(error)}, ensure_ascii=False), file=sys.stderr)
            return 2
    if args.iterations < 1 or args.interval <= 0:
        parser.error("iterations and interval must be positive")
    result_code = 0
    for i in range(args.iterations):
        try:
            record, code = cycle(args.config, args.library, args.golden, args.backup_dir)
        except (OSError, ValueError) as error:
            print(json.dumps({"status": "failed", "error": str(error)}), file=sys.stderr)
            return 2
        print(json.dumps(record, ensure_ascii=False), flush=True)
        result_code = max(result_code, code)
        if code == 2:
            break
        if i + 1 < args.iterations:
            time.sleep(args.interval)
    return result_code


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import search  # noqa: E402  (records the use; see search.traced)
    sys.exit(search.traced("librarian", main))
