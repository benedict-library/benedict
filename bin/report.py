#!/usr/bin/env python3
"""Report a finished piece of work: keep it in a library's filing cabinet, then hand it to the channel you connected.

usage: report.py <library key> "title" [file]      the report body is the file, else stdin

1. Keep: the report is saved like any answered question (bin/save_query.py: registry row, save rules). A report that cannot be kept is not sent.
2. Send: LIBRARY_REPORT_CMD (environment, else search.env) is a command that gets the body on stdin and the title in
   LIBRARY_REPORT_TITLE — a chat webhook script, a notifier, a mail command. The library names no service; each user
   connects their own. Not set: nothing leaves this machine and the output says so. Set but failing: a failure with
   the command's exit code; nothing else is tried. LIBRARY_REPORT_TIMEOUT is the seconds the command may take
   (default 60).
"""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys

HERE = Path(__file__).resolve().parent


def _load(name):
    spec = importlib.util.spec_from_file_location(f"_library_{name}", HERE / f"{name}.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


search = _load("search")
save_query = _load("save_query")


def send(cfg, title, body):
    """-> (exit code, message). The connected command gets the body on stdin."""
    cmd = cfg.get("LIBRARY_REPORT_CMD", "").strip()
    if not cmd:
        return 0, "보고 보내기: 설정 안 됨 (LIBRARY_REPORT_CMD) — 서류철에만 남겼다"
    timeout = float(cfg.get("LIBRARY_REPORT_TIMEOUT") or 60)
    try:
        r = subprocess.run(search.split_cmd(cmd), input=body, text=True, capture_output=True, timeout=timeout,
                           env={**os.environ, "LIBRARY_REPORT_TITLE": title})
    except subprocess.TimeoutExpired:
        return 124, f"보고 보내기 실패: {timeout:g}초 안에 끝나지 않음"
    except OSError as error:
        return 127, f"보고 보내기 실패: {error}"
    if r.returncode:
        return r.returncode, f"보고 보내기 실패 rc={r.returncode}: {(r.stderr or r.stdout).strip()[-300:]}"
    return 0, "보고 보냄"


def run(row, title, body, cfg):
    """Keep, then send. -> (exit code, output lines)."""
    if not body.strip():
        return 2, ["빈 보고는 남기지 않는다"]
    try:
        code, msg = save_query.save(row, title, body)
    except ValueError as error:
        code, msg = 2, f"거부: {error}"
    if code or not msg.startswith("저장"):
        return code or 1, [msg, "보고 보내기: 서류철에 남기지 못해 보내지 않았다"]
    rc, sent = send(cfg, title, body)
    return rc, [msg, sent]


def main(argv):
    if len(argv) not in (3, 4):
        raise SystemExit(__doc__)
    body = Path(argv[3]).read_text(encoding="utf-8") if len(argv) == 4 else sys.stdin.read()
    code, lines = run(save_query.load_row(argv[1]), argv[2], body, search.settings())
    for line in lines:
        print(line, file=sys.stdout if code == 0 else sys.stderr)
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv))
