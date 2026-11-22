#!/usr/bin/env python3
"""Bring originals into a reading room variable and show only the part that is needed.

usage: read.py --var TOPIC [--library KEY] [--attach] [--q "words"] [--lines A-B] PATH [PATH ...]

The originals are copied into the reading room variable reading_<TOPIC>_<yymmdd> as {path: {text, read_at}}, so
what a session read stays in the library after the conversation is compacted or ends. The screen shows only the
lines holding the question words (with one line around), or the --lines range, or the headings and first lines.

PATH is absolute, or relative to the library's wiki_root (a path whose first folder is another library's wiki
folder, as search prints for other libraries, resolves under that library). Without --library the central
library is used. Box libraries read inside their own box. Settings are the same as search.py.

With --attach, a local absolute file outside all registered roots is
sent with the code, because the reading room's machine cannot open it; it lands in the same variable.
"""
import datetime
import re
import sys
from pathlib import Path, PurePosixPath, PureWindowsPath

sys.path.insert(0, str(Path(__file__).resolve().parent))
import search  # noqa: E402  (settings, registry and room access are shared with search)

ROOM_READ = Path(__file__).resolve().parent.parent / "rooms" / "room_read.py"
HEAD = 20  # lines shown when there are no question words and no range
READS = "print('READS', (globals().get('ROOM_READS') or [''])[0] if 'docs' not in globals() else '')\n"


def room_path(value):
    """Registry paths belong to the room host, which may use a different OS."""
    return PureWindowsPath(value) if PureWindowsPath(value).drive else PurePosixPath(value)


def locate(reg, key, path):
    """(library key, absolute path). A path under another library's wiki folder belongs to that library."""
    original = room_path(path)
    roots = {k: room_path(r["wiki_root"]) for k, r in reg.items() if r.get("wiki_root")}
    if original.is_absolute():
        owners = [k for k, root in roots.items() if type(original) is type(root) and original.is_relative_to(root)]
        owner = max(owners, key=lambda k: len(roots[k].parts)) if owners else key
        return owner, str(original)
    first = original.parts[0] if original.parts else ""
    other = next((k for k, root in roots.items() if root.name == first), None)
    if other:
        return other, str(roots[other].parent / path)
    if key not in roots:
        raise SystemExit(f"등록부에 {key} wiki_root 없음 — 절대 경로로 준다: {path}")
    return key, str(roots[key] / path)


def var_name(topic, day=None):
    t = re.sub(r"[^0-9A-Za-z가-힣_]+", "_", topic).strip("_")
    if not t:
        raise SystemExit("--var 주제가 비었다")
    return f"reading_{t}_{(day or datetime.date.today()):%y%m%d}"


def read_code(paths, var, words, lines, local=None, root=None):
    s = ROOM_READ.read_text(encoding="utf-8")
    return (s.replace("__PATHS__", repr(paths)).replace("__VAR__", repr(var)).replace("__WORDS__", repr(words))
            .replace("__LINES__", repr(lines)).replace("__HEAD__", str(HEAD)).replace("__ROOT__", repr(root)).replace("__LOCAL__", repr(local or {})))


def reading_room(cfg, room):
    """The room that holds the text: the room itself, or the reading room a search room points to (ROOM_READS)."""
    rc, out, err = search.room_exec(cfg, room, READS, 15, 30)
    m = re.search(r"^READS ?(.*)$", out, re.M) if rc == 0 else None
    if not m:
        raise SystemExit(f"{room} 실에 못 붙음: rc={rc} {(out or err).strip()[:120]}")
    return m.group(1).strip() or room


def main(argv):
    args, var, key, q, lines = list(argv[1:]), None, None, "", None
    attach = False
    while args and args[0].startswith("--"):
        flag = args.pop(0)
        if flag == "--attach":
            attach = True
        elif flag == "--var":
            var = args.pop(0)
        elif flag == "--library":
            key = args.pop(0)
        elif flag == "--q":
            q = args.pop(0)
        elif flag == "--lines":
            a, _, b = args.pop(0).partition("-")
            lines = (int(a), int(b or a))
        else:
            raise SystemExit(__doc__)
    if not var or not args:
        raise SystemExit(__doc__)
    cfg = search.settings()
    reg = search.registry(cfg)
    key = key or next(k for k, v in reg.items() if v.get("central"))
    if key not in reg:
        raise SystemExit(f"등록부에 없는 도서관: {key} (있는 것: {' '.join(reg)})")
    if reg[key].get("search") == "box":
        raise SystemExit(f"{key}는 컨테이너 도서관이다 — 원문은 그 컨테이너 안에서 읽는다")
    groups, local = {}, {}
    for p in args:
        if ".." in Path(p).parts:
            raise SystemExit("path must stay inside the selected library")
        k, path = locate(reg, key, p)
        root = reg[k].get("wiki_root")
        if not root:
            raise SystemExit("library has no readable wiki")
        if not room_path(path).is_relative_to(room_path(root)):
            if not attach:
                raise SystemExit("use --attach to share a host file with the reading room")
            file = Path(p)
            if not file.is_file() or file.resolve() != file.absolute():
                raise SystemExit("attachment must be a regular file at its resolved path")
            local[p] = file.read_bytes().decode("utf-8")
            groups.setdefault(key, []).append(p)
            continue
        if k != key and not (k in reg[key].get("reads", []) or
                              (reg[key].get("central") and "reads" not in reg[key])):
            raise SystemExit("library is outside the selected read scope")
        groups.setdefault(k, []).append(path)
    words = sorted({w.lower() for w in re.findall(r"[가-힣]{2,}|[A-Za-z0-9]{2,}", q)}, key=len, reverse=True)[:8]
    rc_all = 0
    for k, paths in groups.items():
        if reg[k].get("search") == "box":
            raise SystemExit(f"{k}는 컨테이너 도서관이다 — 원문은 그 컨테이너 안에서 읽는다")
        room = reading_room(cfg, reg[k]["room"])
        sent = {p: local[p] for p in paths if p in local}
        rc, out, err = search.room_exec(cfg, room, read_code(paths, var_name(var), words, lines, sent, reg[k]["wiki_root"]), 20, 40)
        sys.stdout.write(out)
        if rc != 0 or "Traceback" in out:
            print(f"[{room} 열람실 읽기 실패 rc={rc}] {err.strip()[:200]}", file=sys.stderr)
            rc_all = 1
    return rc_all


if __name__ == "__main__":
    sys.exit(search.traced("reading", lambda: main(sys.argv)))
