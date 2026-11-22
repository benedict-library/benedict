#!/usr/bin/env python3
"""Synchronize a local registry library into four bundled persistent rooms — sending only what changed.

The reading room owns document bodies; the search room owns only their lexical index.
Meeting and librarian rooms preserve the mailbox (`mail`, see bin/mail.py) and operational journals.

A state file (path -> [mtime in nanoseconds, size]) remembers what the rooms were sent, so a later run sends only changed and
deleted documents. --full sends everything again; --path FILE is the save hook (it checks FILE is in the library and
also sends any other document written without a sync). A room whose contents no longer match the state (restarted,
tokenizer changed) is refilled in full by the same run, and the run says so.
"""
import argparse
import base64
import datetime
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import zlib

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from library_local import Library, ROOM_KINDS, atomic_text

ROOM = Path(__file__).resolve().parent / "room.py"
RUNTIME = Path(__file__).resolve().parents[1] / "runtime"
sys.path.insert(0, str(RUNTIME))
import wiki_room_fuse as fuse

CHUNK = 200   # documents per room call
PRELUDE = f"import sys, base64, json, zlib\nif {str(RUNTIME)!r} not in sys.path: sys.path.insert(0, {str(RUNTIME)!r})\nimport wiki_room_fuse as fuse\n"


def room_command(command, room, code=None, args=()):
    result = subprocess.run([sys.executable, str(ROOM), command, room, *args], input=code,
                            text=True, encoding="utf-8", capture_output=True, timeout=120)
    if result.returncode:
        raise RuntimeError(f"{command} {room} failed: {result.stderr or result.stdout}")
    return result.stdout


def packed(obj):
    """Room code that rebuilds obj (compressed, so a chunk of bodies stays one short line)."""
    raw = base64.b64encode(zlib.compress(json.dumps(obj, ensure_ascii=False).encode("utf-8"), 6)).decode("ascii")
    return f"json.loads(zlib.decompress(base64.b64decode('{raw}')).decode('utf-8'))"


def room_names(lib, key, prefix=None):
    """Room names: the registry's `rooms` choice, else <prefix>-<kind> (prefix defaults to the library key)."""
    rooms = lib.registry["libraries"][key].get("rooms", {})
    return {kind: rooms.get(kind) or f"{prefix or key}-{kind}" for kind in ROOM_KINDS}


def copy_extras(r):
    """Fields other readers of the copy use as they are: order, golden set, librarian step, folders (comma list)."""
    out = {k: r[k] for k in ("ord", "golden", "librarian") if k in r}
    if r.get("dirs"):
        out["dirs"] = ",".join(r["dirs"])
    return out


def registry_copy(lib, key, row, root, names):
    """This library first, then only the libraries it may read (reads) — a room never learns of others."""
    registry = [{"key": key, "name": row.get("name", key), "central": True, "wiki_root": str(root),
                 "room": names["search"], "search": "room", "status": "active", **copy_extras(row)}]
    for other, r in lib.reads(key):
        if r.get("status", "active") != "active":
            continue
        copy = {"key": other, "name": r.get("name", other), "central": False,
                "room": r.get("room") or r.get("rooms", {}).get("search") or f"{other}-search",
                "search": r.get("search", "room"), "status": "active", **copy_extras(r)}
        if "wiki_root" in r:
            copy["wiki_root"] = str((lib.config.parent / r["wiki_root"]).resolve())
        if "maxd" in r:
            copy["maxd"] = r["maxd"]
        registry.append(copy)
    return registry


def room_sections(lib, row, root, names):
    """The search room's side-section choices with paths made absolute and the meeting room filled in."""
    sections = json.loads(json.dumps(row.get("sections", {})))
    if "today" in sections:
        sections["today"]["dirs"] = [str(root / d) if not Path(d).is_absolute() else d for d in sections["today"].get("dirs", [])]
    if "meeting_vars" in sections:
        sections["meeting_vars"].setdefault("room", names["meeting"])
    return sections


def rrf_code(root):
    """Copy of the rank-learning weights (<wiki_root>/.wiki-pending/rrf_weights.json) for readers that cannot see the
    wiki; rooms read the file itself."""
    weights = root / ".wiki-pending" / "rrf_weights.json"
    return f"rrf_weights = {packed(json.loads(weights.read_text(encoding='utf-8')))}\n" if weights.is_file() else ""


def terms_code(lib, row):
    """library_terms from the registry's terms file; without the choice the room keeps whatever another librarian put there."""
    if "terms" not in row:
        return ""
    terms = json.loads((lib.config.parent / row["terms"]).read_text(encoding="utf-8"))
    if not isinstance(terms, list) or any(not isinstance(t, dict) or not t.get("표준어") for t in terms):
        raise ValueError(f"terms file must be a list of entries with 표준어: {row['terms']}")
    return f"library_terms = {packed(terms)}\n"


def synchronize(config, key=None, prefix=None, full=False, path=None):
    lib = Library(config)
    key, row, root = lib.selected(key)
    names = room_names(lib, key, prefix)
    for room in names.values():
        room_command("start", room)
    _, paths = lib.document_paths(key)
    files = {str(root / rel): resolved for rel, resolved in paths.items()}
    cur = {p: [f.stat().st_mtime_ns, f.stat().st_size] for p, f in files.items()}
    if path is not None and str(Path(path).absolute()) not in cur:
        raise ValueError(f"--path not in library {key}: {path}")
    state_file = lib.config.parent / ".library-state" / (
        hashlib.sha256(f"{lib.config}\0{key}\0{names['reading']}".encode()).hexdigest()[:16] + "-rooms.json")
    state = {} if full or not state_file.is_file() else json.loads(state_file.read_text(encoding="utf-8"))["files"]
    targets = sorted(p for p, v in cur.items() if state.get(p) != v)
    deleted = sorted(p for p in state if p not in cur)
    # A room that does not hold what the state says it was sent is refilled in full (self-healing).
    n_read = int(room_command("exec", names["reading"], "print(len(globals().get('docs', {})))\n").split()[-1])
    n_search, tokv = room_command("exec", names["search"], "print(len(globals().get('_len', {})), globals().get('_tokv'))\n").split()[-2:]
    reading_full = full or n_read != len(state)
    search_full = full or int(n_search) != len(state) or (state and tokv != str(fuse.TOKV))
    room_command("exec", names["reading"], "docs = {}\n" if reading_full else "docs = globals().get('docs', {})\n")
    if search_full:
        room_command("exec", names["search"], PRELUDE + "for _k in fuse.INDEX_KEYS: globals().pop(_k, None)\n")
    send = sorted(cur) if reading_full or search_full else targets
    changed = set(targets)
    for i in range(0, len(send), CHUNK):
        batch = {p: lib.room_body(key, files[p]) for p in send[i:i + CHUNK]}
        to_read = batch if reading_full else {p: b for p, b in batch.items() if p in changed}
        to_search = batch if search_full else {p: b for p, b in batch.items() if p in changed}
        if to_read:
            room_command("exec", names["reading"], PRELUDE + f"docs.update({packed(to_read)})\n")
        if to_search:
            room_command("exec", names["search"], PRELUDE + f"fuse.index_apply(globals(), {packed(to_search)}, [])\n")
    if deleted and not reading_full:
        room_command("exec", names["reading"], PRELUDE + f"for _k in {packed(deleted)}: docs.pop(_k, None)\n")
    if deleted and not search_full:
        room_command("exec", names["search"], PRELUDE + f"fuse.index_apply(globals(), {{}}, {packed(deleted)})\n")
    counts = (room_command("exec", names["reading"], "print(len(docs))\n").split()[-1],
              room_command("exec", names["search"], "print(len(globals().get('_len', {})))\n").split()[-1])
    if counts != (str(len(cur)), str(len(cur))):
        raise RuntimeError(f"rooms hold {counts[0]} bodies / {counts[1]} index entries, disk {len(cur)} — run --full")
    at = datetime.datetime.now().isoformat(timespec="seconds")
    meta = f"ROOM_DOC_ROOT = {str(root)!r}\nROOM_CARD_PREFIXES = {row.get('cards', [])!r}\nsync_meta = {{'at': {at!r}, 'n': {len(cur)}}}\n"
    registry = registry_copy(lib, key, row, root, names)
    room_command("exec", names["reading"], PRELUDE + meta + f"ROOM_INDEX_BLOCK_RULES = {row.get('index_block', {})!r}\n")
    room_command("exec", names["search"], PRELUDE + meta + f"ROOM_READS = {[names['reading']]!r}\n"
                 f"library_registry = {registry!r}\nROOM_SECTIONS = {room_sections(lib, row, root, names)!r}\n" + terms_code(lib, row) + rrf_code(root))
    room_command("exec", names["meeting"], "mail = globals().get('mail', {})\n")
    room_command("exec", names["librarian"], "journal = globals().get('journal', [])\n")
    atomic_text(state_file, json.dumps({"updated": at, "rooms": names, "files": cur}, ensure_ascii=False))
    return {"library": key, "documents": len(cur), "sent": len(targets), "deleted": len(deleted),
            "reading_full": reading_full, "search_full": bool(search_full), "rooms": names,
            "reads": [r["key"] for r in registry[1:]], "sync_at": at, "search_engine": "lexical", "embedding": "disabled"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", required=True)
    parser.add_argument("--library")
    parser.add_argument("--prefix")
    parser.add_argument("--full", action="store_true", help="send every document again")
    parser.add_argument("--path", help="save hook: the page just written (must be in the library)")
    args = parser.parse_args()
    if args.full and args.path:
        parser.error("--full and --path do not go together")
    try:
        print(json.dumps(synchronize(args.config, args.library, args.prefix, args.full, args.path), ensure_ascii=False))
        return 0
    except (ValueError, OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(json.dumps({"error": str(error)}, ensure_ascii=False), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import common  # noqa: E402  (records the use; see common.traced)
    sys.exit(common.traced("librarian", main))
