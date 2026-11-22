#!/usr/bin/env python3
"""Task materials in a meeting room: one helper puts the originals up, every other agent reads them by short key.

usage: task.py [--room ROOM] add <task> <path> --summary TEXT [--key KEY]   put the file under a short key
       task.py [--room ROOM] list <task>                    keys, sizes and three-line summaries; no bodies
       task.py [--room ROOM] get <task> <key> [--lines A-B | --q WORDS]  one material's or result slot's text;
                                                    numbered lines of a range, or of lines holding any word
       task.py [--room ROOM] put <task> <slot> [file]       a result from the file (or stdin) into results[slot]

The task is the room variable task_<task> = {"materials": {key: {path, text, summary, chars, at}}, "results": {}}.
Hand another agent only the task name and the result slot; it lists, then gets the keys it needs. The key is the
file name without extension unless --key is given. --summary is required: whoever read the original in a reading
room writes the three lines others choose by. Materials written before this tool ({path: text}) list with their
title and first two lines, marked "(머리)" because they are cut, not written. Rooms are reached as in bin/mail.py.
"""
import base64
import datetime
import importlib.util
import inspect
import os
from pathlib import Path
import re
import shlex
import sys

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("_library_common", HERE / "common.py")
common = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(common)

CUT = 90  # characters kept from each summary line

ADD = """import base64
_t = globals().setdefault({var!r}, {{}})
_m = _t.setdefault('materials', {{}})
_t.setdefault('results', {{}})
_m[{key!r}] = {{'path': {path!r}, 'text': base64.b64decode({b64!r}).decode(), 'summary': {summary!r},
               'chars': {chars!r}, 'at': {at!r}}}
print('넣음', {var!r}, {key!r}, {chars!r}, '자 · 재료', len(_m), '건')
"""
LIST = """{head}
_t = globals().get({var!r})
if not isinstance(_t, dict):
    print('과제 없음', {var!r})
else:
    _m = _t.get('materials') or {{}}
    print({var!r}, '재료', len(_m), '건')
    for k, v in _m.items():
        if isinstance(v, dict):
            print(f"\\n[{{k}}] {{v.get('chars', len(v.get('text', '')))}}자 {{v.get('path', '')}}")
            print(v.get('summary', ''))
        else:  # materials written before this tool: {{path: text}}
            print(f"\\n[{{k}}] {{len(str(v))}}자 (옛 형식)")
            print(head_summary(str(v)))
    if 'materials' not in _t:  # a meeting task (crew_meeting) or its source piece: {{목적, 안건, 재료, …}} — top-level keys
        for k, v in _t.items():
            if k != 'results':
                print(f"\\n[{{k}}] {{len(str(v))}}자")
                print(str(v).splitlines()[0][:CUT] if str(v).strip() else '')
    _r = _t.get('results') or {{}}
    print('\\n결과 자리:', ', '.join(f"{{k}}({{len(str(v))}}자)" for k, v in _r.items()) or '없음')
    print('본문: task.py get', {task!r}, '<키> [--lines A-B | --q "낱말"]')
"""
GET = """_t = globals().get({var!r})
_m = (_t or {{}}).get('materials') or {{}} if isinstance(_t, dict) else {{}}
_r = (_t or {{}}).get('results') or {{}} if isinstance(_t, dict) else {{}}
_top = {{k: w for k, w in _t.items() if k != 'results'}} if isinstance(_t, dict) and 'materials' not in _t else {{}}
_v = _m.get({key!r}, _r.get({key!r}, _top.get({key!r})))  # a material, a result slot, then a meeting task's top-level key
if isinstance(_v, dict) and 'text' not in _v:  # a result slot some agent wrote as {{name: value}} — one "name: value" line each
    _x = '\\n'.join(f'{{k}}: {{w}}' for k, w in _v.items())
elif isinstance(_v, list):
    _x = '\\n'.join(str(w) for w in _v)
else:
    _x = _v.get('text', '') if isinstance(_v, dict) else str(_v)
_rng, _q = {rng!r}, {words!r}
if _v is None:  # a room keeps running, so no SystemExit here
    print('키 없음', {key!r}, '— 있는 키:', ', '.join([*_m, *_top]) or '없음', '· 결과 자리:', ', '.join(_r) or '없음')
elif _q:
    _ls = _x.splitlines()
    _hit = [i for i, l in enumerate(_ls) if any(w in l for w in _q)]
    _show = sorted({{j for i in _hit for j in (i - 1, i, i + 1) if 0 <= j < len(_ls)}})
    for n, j in enumerate(_show):
        if n and j != _show[n - 1] + 1:
            print('…')
        print(f'{{j + 1}}: {{_ls[j]}}')
    print('--', len(_ls), '줄 중 낱말 든 줄', len(_hit))
elif _rng:
    _ls = _x.splitlines()
    for i in range(_rng[0], min(_rng[1], len(_ls)) + 1):
        print(f'{{i}}: {{_ls[i - 1]}}')
    print('--', len(_ls), '줄 중', _rng[0], '-', min(_rng[1], len(_ls)))
else:
    print(_x)
"""
PUT = """import base64
_t = globals().setdefault({var!r}, {{}})
_t.setdefault('materials', {{}})
_t.setdefault('results', {{}})[{slot!r}] = base64.b64decode({b64!r}).decode()
print('넣음', {var!r}, 'results', {slot!r}, len(_t['results'][{slot!r}]), '자')
"""


def var_of(task):
    t = re.sub(r"[^0-9A-Za-z가-힣_]+", "_", task).strip("_")
    if not t:
        raise ValueError("과제 이름이 비었다")
    return t if t.startswith("task_") else "task_" + t


def head_summary(text):
    """Title plus the first two lines of prose, each cut to CUT characters."""
    lines = text.splitlines()
    title, body, i = "", [], 0
    if lines and lines[0].strip() == "---":
        # the closing "---" ends the header; an unclosed one ends at the first line that is not "key: value" or indented
        end = next((j for j in range(1, len(lines)) if lines[j].strip() == "---"), None)
        if end is None:
            end = next((j for j in range(1, len(lines)) if not re.match(r"\s|[\w-]+:", lines[j])), len(lines))
        title = next((l.split(":", 1)[1].strip().strip("'\"") for l in lines[1:end] if l.startswith("title:")), "")
        i = end + (end < len(lines) and lines[end].strip() == "---")
    for l in lines[i:]:
        s = l.strip()
        if s.startswith("#"):
            title = title or s.lstrip("#").strip()
        elif s and not s.startswith(("|", "```", "<", "![")) and len(body) < 2:
            body.append(s.lstrip("-*> ").strip())
        if title and len(body) == 2:
            break
    return "\n".join(l[:CUT] for l in ["(머리) " + (title or "제목 없음"), *body])


def lines_of(value):
    m = re.fullmatch(r"(\d+)-(\d+)", value)
    if not m or int(m.group(1)) < 1 or int(m.group(1)) > int(m.group(2)):
        raise ValueError(f"--lines 는 A-B (1 ≤ A ≤ B): {value}")
    return int(m.group(1)), int(m.group(2))


def option(argv, flag):
    if flag in argv:
        i = argv.index(flag)
        if i + 1 >= len(argv):
            raise ValueError(f"{flag} 값이 없다")
        value = argv[i + 1]
        del argv[i:i + 2]
        return value
    return None


def code_for(argv, stdin):
    argv = list(argv)
    cmd = argv[0] if argv else ""
    if cmd == "add":
        key, summary = option(argv, "--key"), option(argv, "--summary")
        if len(argv) != 3:
            raise ValueError(__doc__)
        if not (summary or "").strip():
            raise ValueError("--summary 가 필요하다 — 원문을 읽은 사람이 3줄 요약을 쓴다")
        path = Path(argv[2]).resolve()
        text = path.read_text(encoding="utf-8")
        if not text.strip():
            raise ValueError(f"빈 파일은 넣지 않는다: {path}")
        key = key or path.stem
        return ADD.format(var=var_of(argv[1]), key=key, path=str(path), b64=base64.b64encode(text.encode()).decode(),
                          summary=summary.strip(), chars=len(text),
                          at=datetime.datetime.now().isoformat(timespec="seconds"))
    if cmd == "list" and len(argv) == 2:
        # the room runs the same summary for materials written before this tool
        head = f"import re\nCUT = {CUT}\n" + inspect.getsource(head_summary)
        return LIST.format(head=head, var=var_of(argv[1]), task=argv[1])
    if cmd == "get":
        rng, q = option(argv, "--lines"), option(argv, "--q")
        if len(argv) == 3:
            return GET.format(var=var_of(argv[1]), key=argv[2], rng=lines_of(rng) if rng else None,
                              words=q.split() if q else None)
    if cmd == "put" and len(argv) in (3, 4):
        body = (Path(argv[3]).read_text(encoding="utf-8") if len(argv) == 4 else stdin.read()).rstrip("\n")
        if not body:
            raise ValueError("빈 내용은 넣지 않는다")
        return PUT.format(var=var_of(argv[1]), slot=argv[2], b64=base64.b64encode(body.encode()).decode())
    raise ValueError(__doc__)


def main(argv):
    args, room = list(argv[1:]), None
    if args[:1] == ["--room"] and len(args) > 1:
        room = args[1]
        del args[:2]
    cfg = common.settings()
    room = room or cfg.get("LIBRARY_MAIL_ROOM")
    try:
        if not room:
            raise ValueError("set --room or LIBRARY_MAIL_ROOM")
        code = code_for(args, sys.stdin)
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        return 2
    cfg.setdefault("LIBRARY_ROOM_CMD", shlex.join([sys.executable, str(HERE / "room.py"), "exec"]))
    rc, out, err = common.room_exec(cfg, room, code, 60, 120)
    sys.stdout.write(out)
    if rc == 0 and out.startswith(("키 없음", "과제 없음")):
        return 1
    if rc:
        print(f"과제 실 {room} 실패 rc={rc}: {(err or out).strip()[:200]}", file=sys.stderr)
    return rc


if __name__ == "__main__":
    sys.exit(common.traced("meeting", lambda: main(sys.argv)))
