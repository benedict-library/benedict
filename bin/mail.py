#!/usr/bin/env python3
"""Mailbox in a meeting room: agents leave answers under a topic and others collect them later.

usage: mail.py [--room ROOM] put <topic> [file]      put the file (or stdin) under topic; sender: --from, LIBRARY_MAIL_FROM or host name
       mail.py [--room ROOM] get <topic> [--clear]   print the topic's entries (--clear empties the topic after printing)
       mail.py [--room ROOM] list                    topics with counts and the last time

The mailbox is the room variable `mail` = {topic: [{"from", "at", "text"}]}. Answers survive sessions and time gaps
because they sit in the room, not in a conversation. The room is --room, else LIBRARY_MAIL_ROOM. Rooms are reached
through bin/common.py (LIBRARY_ROOM_EXEC, LIBRARY_ROOM_CMD from the environment or search.env); without
LIBRARY_ROOM_CMD the bundled bin/room.py runs rooms on this machine (LIBRARY_ROOM_HOME).
"""
import base64
import importlib.util
import os
from pathlib import Path
import shlex
import socket
import sys

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("_library_common", HERE / "common.py")
common = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(common)

PUT = """import base64, datetime
mail = globals().setdefault('mail', {{}})
mail.setdefault({topic!r}, []).append({{
    'from': {sender!r},
    'at': datetime.datetime.now().isoformat(timespec='seconds'),
    'text': base64.b64decode({b64!r}).decode(),
}})
print('넣음', {topic!r}, len(mail[{topic!r}]), '건')
"""
GET = """mail = globals().setdefault('mail', {{}})
items = mail.get({topic!r}) or []
for i, m in enumerate(items, 1):
    print(f"[{{i}}] {{m['at']}} {{m['from']}}")
    print(m['text'])
print('--', len(items), '건')
if {clear!r}:
    mail.pop({topic!r}, None)
    print('비움')
"""
LIST = """mail = globals().get('mail') or {}
if not mail:
    print('회신함 비어 있음')
for k, v in mail.items():
    _last = v[-1].get('at', '미상') if v and isinstance(v[-1], dict) else '미상'  # entries outside the format still count
    print(k, ':', len(v), '건 (마지막', _last, ')')
"""


def code_for(argv, stdin):
    cmd = argv[0] if argv else ""
    if cmd == "put" and len(argv) in (2, 3):
        # trailing newlines are dropped, as a shell's $(cat) does, so every tool stores the same text
        body = (Path(argv[2]).read_text(encoding="utf-8") if len(argv) == 3 else stdin.read()).rstrip("\n")
        if not body:
            raise ValueError("빈 내용은 넣지 않는다")
        sender = os.environ.get("LIBRARY_MAIL_FROM") or socket.gethostname()
        return PUT.format(topic=argv[1], sender=sender, b64=base64.b64encode(body.encode()).decode())
    if cmd == "get" and len(argv) in (2, 3) and argv[2:] in ([], ["--clear"]):
        return GET.format(topic=argv[1], clear=argv[2:] == ["--clear"])
    if cmd == "list" and len(argv) == 1:
        return LIST
    raise ValueError(__doc__)


def main(argv):
    args, room = list(argv[1:]), None
    while args and args[0] in ("--room", "--from"):
        flag, value = args.pop(0), args.pop(0)
        if flag == "--room":
            room = value
        else:
            os.environ["LIBRARY_MAIL_FROM"] = value
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
    if rc:
        print(f"회신함 실 {room} 실패 rc={rc}: {err.strip()[:200]}", file=sys.stderr)
    return rc


if __name__ == "__main__":
    sys.exit(common.traced("meeting", lambda: main(sys.argv)))
