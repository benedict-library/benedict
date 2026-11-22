#!/usr/bin/env python3
"""Settings, room calls and use records shared by the Python room tools (mail, task, read, web and the librarian's).

Search itself is the Rust `benedict search`; this module holds only what the remaining Python tools need.

Settings come from the environment, else from ~/.config/library/search.env (KEY=VALUE lines; LIBRARY_SEARCH_ENV
names another file):
  LIBRARY_ROOM_EXEC      command prefix that reaches the room host ('' = this machine)
  LIBRARY_ROOM_CMD       command on the room host that runs Python from stdin in a room; the room name is appended
  LIBRARY_REGISTRY_ROOM  room that holds the registry copy `library_registry`
  LIBRARY_TRACE_EXEC     command prefix that reaches the trace host (optional)
  LIBRARY_TRACE_CMD      command that records one trace per use of a room tool (reading, meeting, librarian, search);
                         {room} becomes that room, {who} LIBRARY_MAIL_FROM or the host name, {yymmdd} today's date,
                         and --prompt --output --elapsed --rc are appended (optional)
"""
import datetime
import json
import os
import shlex
import socket
import subprocess
import sys
import time
from pathlib import Path

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


READ_REGISTRY = "import json as _j\nprint(_j.dumps(globals().get('library_registry'), ensure_ascii=False))\ndel _j\n"


def registry(cfg):
    """Registry rows in order, from the registry room's copy. No copy = nothing to read (the librarian uploads it)."""
    rc, out, err = room_exec(cfg, cfg["LIBRARY_REGISTRY_ROOM"], READ_REGISTRY, 20, 40)
    rows = json.loads(out.strip().splitlines()[-1]) if rc == 0 and out.strip() else None
    if not rows:
        raise SystemExit(f"cannot read the registry copy ({cfg['LIBRARY_REGISTRY_ROOM']} library_registry); "
                         f"run the librarian, then try again: {err.strip()[:120]}")
    return {r["key"]: {k: v for k, v in r.items() if v is not None} for r in rows}


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
