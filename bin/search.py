#!/usr/bin/env python3
"""Search every library the asker may read — or one library — with one command.

usage: search.py [--library KEY] [--top N] [--excerpt N] [--web] [--all] "question"
       search.py --forget-seen

Without --library: the central library's search room (with today's pages, meeting-room variables and decisions),
the other libraries' rooms or boxes (only near hits), glossary terms in the question, and this machine's memory.
With --library: that library alone — its room, its box, or a plain grep of its wiki when the room is stale or
unreachable (the fallback is printed, never silent).
With --web: after the above, the web too. bin/web.py keeps the originals on the library's shelf and files the answer
in its cabinet, so the same question is found inside next time. This takes minutes; without --web nothing goes out.

Settings come from the environment, else from ~/.config/library/search.env (KEY=VALUE lines):
  LIBRARY_ROOM_EXEC      command prefix that reaches the room host ('' = this machine)
  LIBRARY_ROOM_CMD       command on the room host that runs Python from stdin in a room; the room name is appended
  LIBRARY_ROOM_LIB       comma-separated folders on the room host holding the room modules
  LIBRARY_ROOM_DSN_FILE  file on the room host with the vector database URL (KEY=VALUE)
  LIBRARY_REGISTRY_ROOM  room that holds the registry copy `library_registry` and the terms `library_terms`
  LIBRARY_WIKI_EXEC      command prefix that reaches where the wiki roots are mounted ('' = this machine);
                         a wiki root that is not there is looked for on the room host (LIBRARY_ROOM_EXEC)
  LIBRARY_BOX_EXEC       command prefix that reaches the box host ('' = this machine)
  LIBRARY_BOX_CMD        command on the box host that searches a box library; the question is appended
                         (a registry row's own box_cmd wins, so several box libraries can each have theirs)
  LIBRARY_MEMORY_DIR     this machine's memory folder (optional)
  LIBRARY_SAME_NAME_LIBS comma-separated libraries whose pages carry a name in the file name; one-library search
                         also lists up to 20 pages named after the rarest question word (optional)
  LIBRARY_TRACE_EXEC     command prefix that reaches the trace host (optional)
  LIBRARY_TRACE_CMD      command that records one trace per use of a room tool (search, reading, meeting, librarian);
                         {room} becomes that room, {who} LIBRARY_MAIL_FROM or the host name, {yymmdd} today's date,
                         and --prompt --output --elapsed --rc are appended (optional)
  LIBRARY_SOURCE_LOG     file that gets one line per search: time, where the answer came from, question (optional)
  LIBRARY_WEB_EXEC       command prefix that reaches the machine holding the shelf ('' = this machine)
  LIBRARY_WEB_CMD        command on that machine that runs bin/web.py; library key and question are appended
  LIBRARY_WEB_TIMEOUT    seconds for the whole web step (default 540)
  LIBRARY_SESSION_ENV    name of the environment variable that holds the asker's session key (default LIBRARY_SESSION).
                         With a key, side lines this session was already shown (today's pages, room variables, memory)
                         and the closing guidance line are printed once; hits are always printed. --all prints
                         everything; --forget-seen clears the session's record (run it when the asker's context is reset).
  LIBRARY_SEEN_DIR       where those per-session records live (default ~/.cache/library/seen)
  LIBRARY_CENTRAL_HITS   how many of the central library's hits to print (default: all). The heading and record lines
                         always stay; one line names how many more there are, and --all prints them.
"""
import concurrent.futures
import datetime
import glob
import json
import os
import re
import shlex
import socket
import subprocess
import sys
import time
from pathlib import Path

ROOM_QUERY = Path(__file__).resolve().parent.parent / "rooms" / "room_query.py"
MAX_AGE_H = 26      # a room synced longer ago than this is stale
REFS_ALPHA = 0.3    # weight of how often a page was cited as evidence
STOP = re.compile(r"^(STALE|.*Traceback)")
TRACE_KEEP = 2000   # characters of a use's output kept in its trace


def settings():
    cfg = {}
    f = os.environ.get("LIBRARY_SEARCH_ENV")
    if not f:
        try:
            f = Path.home() / ".config" / "library" / "search.env"
        except RuntimeError:  # no home directory (Windows without USERPROFILE): environment only
            f = None
    if f and Path(f).exists():
        for line in Path(f).read_text(encoding="utf-8").splitlines():
            if "=" in line and not line.lstrip().startswith("#"):
                k, v = line.split("=", 1)
                cfg[k.strip()] = v.strip()
    cfg.update({k: v for k, v in os.environ.items() if k.startswith("LIBRARY_")})
    return cfg


def split_cmd(text, windows=os.name == "nt"):
    """A command setting as argv. On Windows backslashes in paths stay and only quotes around a part are removed."""
    if not windows:
        return shlex.split(text)
    return [p[1:-1] if len(p) > 1 and p[0] == p[-1] and p[0] in "'\"" else p for p in shlex.split(text, posix=False)]


def on(prefix, cmd):
    """Run cmd (a list) here, or on another machine through prefix (e.g. 'ssh host')."""
    return split_cmd(prefix) + [shlex.join(cmd)] if prefix.strip() else cmd


def run(argv, stdin="", timeout=60):
    try:
        r = subprocess.run(argv, input=stdin, capture_output=True, text=True, encoding="utf-8", errors="replace",
                           timeout=timeout)
        return r.returncode, r.stdout, r.stderr
    except subprocess.TimeoutExpired:
        return 124, "", f"timeout {timeout}s"


def room_exec(cfg, room, code, t_in=15, t_out=20):
    cmd = split_cmd(cfg["LIBRARY_ROOM_CMD"]) + [room]
    prefix = cfg.get("LIBRARY_ROOM_EXEC", "")
    if not prefix.strip():
        return run(cmd, code, t_in)
    # A dropped connection must not leave the room call running on the far machine.
    return run(on(prefix, ["timeout", str(t_in)] + cmd), code, t_out)


def query_code(cfg, q, top, excerpt, entry=False, same_name_max=0):
    s = ROOM_QUERY.read_text(encoding="utf-8")
    lib = [d for d in cfg.get("LIBRARY_ROOM_LIB", "").split(",") if d]
    s = (s.replace("__QUERY__", repr(q)).replace("__TOPN__", str(int(top))).replace("__MAXAGE__", str(MAX_AGE_H))
         .replace("__REFSALPHA__", str(REFS_ALPHA)).replace("__ROOMLIB__", repr(lib))
         .replace("__DSNFILE__", repr(cfg.get("LIBRARY_ROOM_DSN_FILE", ""))))
    head = "_SEARCH_KEEP = True\n" + ("_ENTRY = True\n" if entry else "")
    return head + s + query_tail(excerpt, same_name_max)


def query_tail(excerpt, same_name_max):
    """Room code run after the search: excerpts, same-name pages, then forget this question's working names."""
    # Excerpts: for each top page, the lines holding question words (and the lines around them).
    tail = (f"\nif 'top' in dir() and '_read' in dir():\n"
            f"    print(_read('excerpts', [_p for _p, _ in top], words, {int(excerpt)}), end='')\n") if excerpt > 0 else ""
    if same_name_max:
        # Pages named after the rarest question word, so a reader can open the right original directly.
        tail += f"""
_names = [p.rsplit('/', 1)[-1].lower() for p in docs] if 'top' in dir() and 'docs' in dir() else []
_qw = [w for w in set(words) if len(w) >= 2 and _df.get(w, 0) and any(w in n for n in _names)] if _names else []
if _qw:
    _key = min(_qw, key=lambda w: (_df[w], -len(w))); _shown = {{p for p, _ in top}}
    _rel = [p for p in docs if _key in p.rsplit('/', 1)[-1].lower() and p not in _shown][:{int(same_name_max)}]
    if _rel:
        print('## 같은 이름 문서(' + _key + ') — 발췌에 답이 없으면 이 중 맞는 문서를 연다')
        print('\\n'.join(_rel))
"""
    tail += ("\nglobals().pop('_SEARCH_KEEP', None)\n"
             "if hasattr(_fz, 'forget_query'): _fz.forget_query(globals(), tuple(dict.fromkeys(_fz.SEARCH_QUERY_NAMES + "
             "('_names', '_qw', '_key', '_shown', '_rel'))))\n")
    return tail


def room_search(cfg, room, q, top, excerpt, entry=False, same_name_max=0, t_in=15, t_out=20):
    """Room hits, or (None, reason) when the room is stale or unreachable."""
    rc, out, err = room_exec(cfg, room, query_code(cfg, q, top, excerpt, entry, same_name_max), t_in, t_out)
    if rc == 0 and out.strip() and not STOP.match(out.strip()) and "Traceback" not in out:
        return out.rstrip("\n") + "\n", None
    return None, f"rc={rc} {(out or err).strip()[:80]}"


GREP = r"""root="$1"; pat="$2"; shift 2
[ -d "$root" ] || { echo "위키 루트 없음: $root" >&2; exit 3; }
[ -f "$root/index.md" ] && grep -i -E -m 20 -- "$pat" "$root/index.md" | sed "s|^|$root/index.md:|"
dirs=(); for d in "$@"; do [ -d "$root/$d" ] && dirs+=("$root/$d"); done
[ ${#dirs[@]} -gt 0 ] && grep -rIin -E -m 3 --include='*.md' -- "$pat" "${dirs[@]}" | cut -c1-260 | head -60
true"""


def local_wiki_grep(root, dirs, pattern):
    """Search a mounted wiki without depending on shell utilities."""
    root = Path(root)
    if not root.is_dir():
        return 3, "", f"위키 루트 없음: {root}"
    resolved_root = root.resolve()
    pattern = re.compile(pattern, re.IGNORECASE)
    hits = []

    def matches(path, limit):
        with path.open(encoding="utf-8", errors="replace") as stream:
            for number, line in enumerate(stream, 1):
                if "\0" in line:
                    break
                if pattern.search(line):
                    yield number, line.rstrip("\r\n")
                    limit -= 1
                    if limit == 0:
                        break
    try:
        index = root / "index.md"
        if not index.is_symlink() and index.is_file():
            hits.extend(f"{index}:{line}\n" for _, line in matches(index, 20))
        count = 0
        for directory in dirs:
            folder = root / directory
            if not folder.resolve().is_relative_to(resolved_root):
                continue
            for path in folder.rglob("*.md"):
                if path.is_symlink() or not path.resolve().is_relative_to(resolved_root) or not path.is_file():
                    continue
                for number, line in matches(path, 3):
                    hits.append(f"{path}:{number}:{line}"[:260] + "\n")
                    count += 1
                    if count == 60:
                        return 0, "".join(hits), ""
    except OSError as exc:
        return 1, "", str(exc)
    return 0, "".join(hits), ""


def wiki_grep(cfg, row, q):
    """Plain grep of a library's wiki: index.md first, then its folders; question words are ORed.
    A root missing where the shared wikis are mounted is looked for on the room host (wikis kept beside their rooms)."""
    if not row.get("wiki_root"):
        return None, "등록부에 wiki_root 없음"
    words = [re.escape(w) for w in re.findall(r"[가-힣]{2,}|[A-Za-z0-9]{2,}", q)][:8] or [re.escape(q)]
    dirs = [d for d in (row.get("dirs") or "entities,concepts,queries").split(",") if d]
    cmd = ["bash", "-c", GREP, "_", row["wiki_root"], "|".join(words)] + dirs
    hosts = list(dict.fromkeys([cfg.get("LIBRARY_WIKI_EXEC", ""), cfg.get("LIBRARY_ROOM_EXEC", "")]))
    for host in hosts:
        if host.strip():
            rc, out, err = run(on(host, cmd), "", 40)
        else:
            rc, out, err = local_wiki_grep(row["wiki_root"], dirs, "|".join(words))
        if rc == 0:
            return out or "(no hits)\n", None
        if rc != 3:   # 3 = root not on that machine; anything else is a real failure
            break
    if rc == 3:
        return None, f"rc=3 위키 루트 없음: {row['wiki_root']}"
    return None, f"rc={rc} {err.strip()[:80]}"


def box_search(cfg, q, row=None):
    """A box library's search: its registry row's own box_cmd, else the one LIBRARY_BOX_CMD."""
    cmd = (row or {}).get("box_cmd") or cfg.get("LIBRARY_BOX_CMD")
    if not cmd:
        return None, "컨테이너 검색 명령 없음(등록부 box_cmd·LIBRARY_BOX_CMD)"
    rc, out, err = run(on(cfg.get("LIBRARY_BOX_EXEC", ""), split_cmd(cmd) + [q]), "", 70)
    return (out.rstrip("\n") + "\n", None) if rc == 0 and out.strip() else (None, f"rc={rc} {(out or err).strip()[:80]}")


READ_REGISTRY = "import json as _j\nprint(_j.dumps(globals().get('library_registry'), ensure_ascii=False))\ndel _j\n"
TERMS_CODE = """import json as _j, re as _r
_q = _r.sub(r"\\s+", "", {q!r}).casefold()
_t = globals().get("library_terms")
_h = None if _t is None else sorted(({{"w": _w, "e": _e}} for _e in _t for _w in {{_e["표준어"], *_e["별칭"], _e.get("약어") or ""}}
      if len(_r.sub(r"\\s+", "", _w)) >= 2 and _r.sub(r"\\s+", "", _w).casefold() in _q), key=lambda _x: -len(_x["w"]))
print(_j.dumps(_h, ensure_ascii=False))
del _j, _r, _q, _t, _h
"""


def registry(cfg):
    """Registry rows in order, from the registry room's copy. No copy = no search (the librarian uploads it)."""
    rc, out, err = room_exec(cfg, cfg["LIBRARY_REGISTRY_ROOM"], READ_REGISTRY, 20, 40)
    rows = json.loads(out.strip().splitlines()[-1]) if rc == 0 and out.strip() else None
    if not rows:
        raise SystemExit(f"도서관 등록부 사본을 못 읽음({cfg['LIBRARY_REGISTRY_ROOM']} library_registry) — 사서가 올린 뒤 다시: {err.strip()[:120]}")
    return {r["key"]: {k: v for k, v in r.items() if v is not None} for r in rows}


def term_lines(cfg, q, n=5):
    rc, out, _ = room_exec(cfg, cfg["LIBRARY_REGISTRY_ROOM"], TERMS_CODE.format(q=q), 20, 40)
    lines = out.strip().splitlines()
    hits = json.loads(lines[-1]) if rc == 0 and lines else None
    if hits is None:
        return ["(용어 사본 없음)"]
    res, seen = [], set()
    for h in hits:
        e = h["e"]
        if (e["표준어"], e["절"]) in seen:
            continue
        seen.add((e["표준어"], e["절"]))
        head = h["w"] if h["w"] == e["표준어"] else f"{h['w']} → {e['표준어']}"
        res.append(f"{head} (§{e['절']}): {(e['실체'] or '').splitlines()[0][:100] if e['실체'] else ''}")
        if len(res) >= n:
            break
    return res or ["(문장에 용어집 말 없음)"]


def memory_lines(mem_dir, q, n=5):
    """Memory files (except MEMORY.md) sharing two or more question words, most overlap first."""
    if not mem_dir or not os.path.isdir(mem_dir):
        return ["해당 없음(메모리 폴더 없음)"]
    toks = set(re.findall(r"[0-9A-Za-z_]+|[가-힣]{2,}", q.lower()))
    hits = []
    for f in sorted(glob.glob(os.path.join(mem_dir, "*.md"))):
        if not f.endswith("MEMORY.md"):
            k = sum(w in Path(f).read_text(encoding="utf-8").lower() for w in toks)
            if k >= 2:
                hits.append((k, os.path.basename(f)))
    return [f for _, f in sorted(hits, key=lambda x: -x[0])[:n]]


def near_section(row, key, out, max_d=None):
    """Another library's hits, shown only when its nearest vector is within the library's distance limit."""
    name = row.get("name", key)
    max_d = row.get("maxd", 0.20) if max_d is None else max_d
    m = re.search(r"최근접 ([0-9.]+)", out or "")
    if not m:
        return [f"## 다른 도서관 — {name}: 검색 못 함(거리 없음)"]
    d = float(m.group(1))
    if d > max_d:
        return [f"## 다른 도서관 — {name}: 가까운 자료 없음(최근접 {d:.3f})"]
    base = short_base(row.get("wiki_root", ""))
    rows = [f"{t.group(1)[len(base):] if base and t.group(1).startswith(base) else t.group(1)} — {t.group(2)}"
            for t in (re.match(r"^(/.+?\.md):[0-9]+:title: (.*)$", l) for l in out.splitlines()[:40]) if t]
    return [f"## 다른 도서관 — {name} (최근접 {d:.3f})"] + rows


def _letters(s):
    return re.sub(r"[^0-9A-Za-z가-힣]", "", s).lower()


def tidy(line):
    """'<path> — <title>' → '<path>' when the file name already spells the whole title (its date and hash aside),
    so the same words are not printed twice. A shortened file name keeps its title."""
    path, sep, title = line.partition(" — ")
    if not sep:
        return line
    stem = os.path.splitext(os.path.basename(path.strip()))[0]
    stem = re.sub(r"_[0-9a-f]{12}$", "", re.sub(r"^\d{6}_", "", stem))
    return path if _letters(stem) and _letters(stem) == _letters(title) else line


def top_hits(lines, n):
    """The heading and record lines ('# …') and the first n hits; one line says how many more were left out."""
    if n is None:
        return lines
    kept, shown, more = [], 0, 0
    for line in lines:
        if line.startswith("# "):
            kept.append(line)
        elif shown < n:
            kept.append(line)
            shown += 1
        else:
            more += 1
    return kept + ([f"(검색실 hit {more}건 더 — 모두 보려면 --all)"] if more else [])


def short_base(wiki_root):
    """Prefix to cut from another library's hit paths: the wiki's parent, or its grandparent when the wiki folder is
    just `wiki` (`/srv/libraries/<key>/wiki`) so the shown path keeps the library name (`<key>/wiki/...`)."""
    root = (wiki_root or "").rstrip("/")
    if not root:
        return ""
    parent = os.path.dirname(root)
    return (os.path.dirname(parent) if os.path.basename(root) == "wiki" else parent) + "/"


def log_source(cfg, kind, q):
    f = cfg.get("LIBRARY_SOURCE_LOG")
    if f:
        Path(f).expanduser().parent.mkdir(parents=True, exist_ok=True)
        with open(Path(f).expanduser(), "a", encoding="utf-8") as fh:
            fh.write(f"{datetime.datetime.now():%Y-%m-%dT%H:%M:%S}\t{kind}\t{q}\n")


def one(cfg, reg, key, q, top=3, excerpt=1200):
    """One library: its box, or its room; a stale or unreachable room falls back to a grep of its wiki."""
    row = reg.get(key)
    if row is None:
        raise SystemExit(f"등록부에 없는 도서관: {key} (있는 것: {' '.join(reg)})")
    if row.get("search") == "box":
        out, why = box_search(cfg, q, row)
        log_source(cfg, f"컨테이너({key})", q)
        return out if out is not None else f"[{key} 컨테이너 검색 실패: {why}]\n"
    if row.get("room"):
        out, why = room_search(cfg, row["room"], q, top, excerpt, same_name_max=20 if key in cfg.get("LIBRARY_SAME_NAME_LIBS", "").split(",") else 0)
        if out is not None:
            log_source(cfg, f"검색실({key})", q)
            return out
        print(f"[{key} 검색실 미정합 또는 불가({why}) → 위키 직접 검색]", file=sys.stderr)
    out, why = wiki_grep(cfg, row, q)
    log_source(cfg, f"위키 직접({key})", q)
    return out if out is not None else f"[{key} 위키 직접 검색 실패: {why}]\n"


def everything(cfg, reg, q, every=False):
    central = next(k for k, v in reg.items() if v.get("central"))
    others = [k for k, v in reg.items() if not v.get("central") and v.get("search") in ("room", "box")]
    with concurrent.futures.ThreadPoolExecutor(max_workers=4 + len(others)) as pool:
        main = pool.submit(room_search, cfg, reg[central]["room"], q, 8, 0, True, 0, 45, 60)
        near = {k: pool.submit(lambda k=k: box_search(cfg, q, reg[k])[0] if reg[k].get("search") == "box"
                               else room_search(cfg, reg[k]["room"], q, 3, 0, t_in=25, t_out=30)[0]) for k in others}
        terms = pool.submit(term_lines, cfg, q)
        mem = pool.submit(memory_lines, cfg.get("LIBRARY_MEMORY_DIR", ""), q)
        out, why = main.result()
    name = reg[central].get("name", central)
    root = reg[central].get("wiki_root", "")
    lines = [f"## 검색실 — {name}"]
    if out is None:
        print(f"[{central} 검색실 미정합 또는 불가({why}) → 위키 직접 검색]", file=sys.stderr)
        g, _ = wiki_grep(cfg, reg[central], q)
        lines += [(g or "").rstrip(), "(검색실 미정합 또는 불가 — 위키 직접 검색으로 내려가 곁 자료는 없음)"]
        log_source(cfg, f"위키 직접({central})", q)
    else:
        side = re.search(r"^## ", out, re.M)   # side sections start at the room's first '## ' heading
        body, side = (out[:side.start()], out[side.start():]) if side else (out, "")
        picked = [l for l in body.splitlines() if l.startswith("# ") or ":title:" in l]
        hits = [tidy(re.sub(r":[0-9]+:title: ", " — ", l.replace(root.rstrip("/") + "/", ""))) for l in picked][:12]
        n = cfg.get("LIBRARY_CENTRAL_HITS", "").strip()
        lines += top_hits(hits, None if every or not n else int(n))
        if side:
            lines.append(side.rstrip())
        log_source(cfg, f"검색실({central})", q)
    quiet = []   # libraries with nothing near share one line: names and distances stay, the repeated heading goes
    for k in others:
        sec = near_section(reg[k], k, near[k].result())
        m = re.match(r"## 다른 도서관 — (.+): 가까운 자료 없음\(최근접 ([0-9.]+)\)$", sec[0])
        if m and len(sec) == 1:
            quiet.append(f"{m.group(1)} {m.group(2)}")
        else:
            lines += [sec[0]] + [tidy(r) for r in sec[1:]]
    if quiet:
        lines.append("## 다른 도서관 — 가까운 자료 없음: " + " · ".join(quiet))
    lines += ["## 용어"] + terms.result() + ["## 메모리"] + mem.result()
    lines.append(f"## 다음: 위 원문 2~3개를 read.py --var <주제> --q \"<낱말>\" <경로>로 열람실에 들고 와 읽고 답한다(위키 루트 {root}). "
                 "없다고 답하기 전엔 동의어로 한 번 더.")
    return "\n".join(lines) + "\n"


def web_search(cfg, reg, key, q):
    """The web step runs where the library's shelf is. Only the library key and the question go there."""
    print(f"## 웹 검색 — {reg[key].get('name', key)}", flush=True)
    if not cfg.get("LIBRARY_WEB_CMD"):
        print("설정 안 됨(LIBRARY_WEB_CMD) — 바깥은 찾지 않았다. 에이전트의 웹 검색 도구로 공개 자료를 찾고, "
              "로컬 등록부의 library.py web-source --config <등록부> --library <도서관> <공개 질문> <원문 URL>로 보관한다.")
        return 1
    cmd = split_cmd(cfg["LIBRARY_WEB_CMD"]) + [key, q]
    rc, out, err = run(on(cfg.get("LIBRARY_WEB_EXEC", ""), cmd), "", int(cfg.get("LIBRARY_WEB_TIMEOUT", 540)))
    sys.stdout.write(out)
    log_source(cfg, f"웹({key})", q)
    if rc:
        print(f"[웹 검색 실패 rc={rc}] {err.strip()[:200]}", file=sys.stderr)
    return rc


def who(cfg):
    """Who is using the library: LIBRARY_MAIL_FROM, else this machine's name."""
    return cfg.get("LIBRARY_MAIL_FROM") or socket.gethostname()


def trace(cfg, room, prompt, output, elapsed, rc=0):
    """Record one use of a room with LIBRARY_TRACE_CMD in the background; a failed record never fails the use."""
    if not cfg.get("LIBRARY_TRACE_CMD"):
        return
    fill = {"{yymmdd}": f"{datetime.date.today():%y%m%d}", "{room}": room, "{who}": who(cfg)}
    cmd = []
    for part in split_cmd(cfg["LIBRARY_TRACE_CMD"]):
        for mark, value in fill.items():
            part = part.replace(mark, value)
        cmd.append(part)
    cmd += ["--prompt", prompt, "--output", output[:TRACE_KEEP], "--elapsed", f"{elapsed:.2f}", "--rc", str(rc)]
    try:
        subprocess.Popen(on(cfg.get("LIBRARY_TRACE_EXEC", ""), cmd), stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL, start_new_session=True)
    except OSError:
        pass


class _Kept:
    """Standard output that also keeps the first TRACE_KEEP characters written to it."""

    def __init__(self, real):
        self.real, self.text = real, ""

    def write(self, s):
        if len(self.text) < TRACE_KEEP:
            self.text += s[:TRACE_KEEP - len(self.text)]
        return self.real.write(s)

    def __getattr__(self, name):
        return getattr(self.real, name)


def utf8_stdio():
    """Text in and out of the tools is UTF-8 whatever the system's own encoding is (English Windows: cp1252)."""
    for stream in (sys.stdin, sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8")


def traced(room, run):
    """Run a room tool's main (run()) and record the use: the command line, what it printed first, time, exit code."""
    utf8_stdio()
    t0, rc, kept = time.time(), 1, _Kept(sys.stdout)
    sys.stdout = kept
    try:
        rc = run()
        return rc
    except SystemExit as e:
        rc = e.code if isinstance(e.code, int) else 0 if e.code is None else 1
        if isinstance(e.code, str):
            kept.text += e.code
        raise
    finally:
        sys.stdout = kept.real
        prompt = " ".join([Path(sys.argv[0]).name] + sys.argv[1:])
        trace(settings(), room, prompt, kept.text, time.time() - t0, rc or 0)


REPEAT_SECTIONS = ("## 오늘 들어온 글", "## 크루 방 변수", "## 메모리")


def seen_file(cfg):
    sid = os.environ.get(cfg.get("LIBRARY_SESSION_ENV") or "LIBRARY_SESSION", "").strip()
    if not sid:
        return None
    folder = Path(cfg.get("LIBRARY_SEEN_DIR") or "~/.cache/library/seen").expanduser()
    return folder / (re.sub(r"[^A-Za-z0-9_.-]", "_", sid) + ".json")


def once_per_session(cfg, text):
    """Side lines this session was already shown, and the closing guidance line, are left out; one line says how many."""
    f = seen_file(cfg)
    if f is None:
        return text
    try:
        seen = set(json.loads(f.read_text(encoding="utf-8")))
    except (OSError, ValueError):
        seen = set()
    out, section, skipped = [], None, 0
    for line in text.splitlines():
        repeatable = line.startswith("## 다음:") or (section and line.strip() and not line.startswith("## "))
        if line.startswith("## "):
            section = next((s for s in REPEAT_SECTIONS if line.startswith(s)), None)
        if repeatable and line in seen:
            skipped += 1
            continue
        if repeatable:
            seen.add(line)
        out.append(line)
    if skipped:
        out.append(f"(이 세션에서 이미 본 곁 줄 {skipped}개는 다시 싣지 않음 — 모두 보려면 --all)")
    try:
        f.parent.mkdir(parents=True, exist_ok=True)
        f.write_text(json.dumps(sorted(seen), ensure_ascii=False), encoding="utf-8")
    except OSError:
        pass
    return "\n".join(out) + "\n"


def main(argv):
    args, key, top, excerpt, web, every = list(argv[1:]), None, 3, 1200, False, False
    if args == ["--forget-seen"]:
        f = seen_file(settings())
        if f is not None and f.exists():
            f.unlink()
        return 0
    while args and args[0].startswith("--"):
        flag = args.pop(0)
        if flag == "--library":
            key = args.pop(0)
        elif flag == "--top":
            top = int(args.pop(0))
        elif flag == "--excerpt":
            excerpt = int(args.pop(0))
        elif flag == "--web":
            web = True
        elif flag == "--all":
            every = True
        else:
            raise SystemExit(__doc__)
    if not args:
        raise SystemExit(__doc__)
    q, cfg, t0 = " ".join(args), settings(), time.time()
    reg = registry(cfg)
    text = one(cfg, reg, key, q, top, excerpt) if key else everything(cfg, reg, q, every)
    sys.stdout.write(text if every or key else once_per_session(cfg, text))
    hits = "\n".join(l for l in text.splitlines() if re.match(r"^# 하이브리드|^(queries|entities|concepts)/", l))
    trace(cfg, "search", q, hits, time.time() - t0)
    if web:
        return web_search(cfg, reg, key or next(k for k, v in reg.items() if v.get("central")), q)
    return 0


if __name__ == "__main__":
    utf8_stdio()
    sys.exit(main(sys.argv))
