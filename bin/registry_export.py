#!/usr/bin/env python3
"""Write the JSON registry from the library registry table (library.registry), so the table stays the one source.

usage: registry_export.py --out library.json

Reads active rows with psql through LIBRARY_REGISTRY_DSN. Columns become registry fields (dirs: comma list -> list);
the `options` column (jsonb) carries the other choices (reads, rooms, sections, terms, cards, index_block, clean).
The result is validated as a registry before it replaces --out; an invalid table leaves the old file in place.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from library_local import Library

SQL = """SELECT coalesce(json_agg(r ORDER BY r.ord), '[]'::json) FROM (
  SELECT key, ord, name, central, room, search, maxd, golden, librarian, wiki_root, dirs, status, save_block, options
  FROM library.registry WHERE status = 'active') r;"""
FIELDS = ("name", "central", "room", "search", "maxd", "wiki_root", "status", "save_block", "ord", "golden", "librarian")


def to_registry(rows):
    libraries = {}
    for r in rows:
        row = {k: r[k] for k in FIELDS if r.get(k) is not None}
        if r.get("dirs"):
            row["dirs"] = [d for d in r["dirs"].split(",") if d]
        row.update(r.get("options") or {})
        libraries[r["key"]] = row
    return {"version": 1, "libraries": libraries}


def write(rows, out):
    out = Path(out)
    fd, name = tempfile.mkstemp(prefix=".registry-", dir=out.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(to_registry(rows), ensure_ascii=False, indent=1) + "\n")
        Library(name)   # validates; raises ValueError on a bad table
        os.replace(name, out)
    finally:
        if os.path.exists(name):
            os.unlink(name)
    return {"out": str(out), "libraries": [r["key"] for r in rows]}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    try:
        dsn = os.environ.get("LIBRARY_REGISTRY_DSN") or sys.exit("LIBRARY_REGISTRY_DSN not set")
        r = subprocess.run(["psql", dsn, "-Atq", "-v", "ON_ERROR_STOP=1"], input=SQL, capture_output=True, text=True, timeout=30)
        if r.returncode:
            raise ValueError("registry table not readable: " + r.stderr.strip()[-200:])
        print(json.dumps(write(json.loads(r.stdout), args.out), ensure_ascii=False))
        return 0
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        print(json.dumps({"error": str(error)}, ensure_ascii=False), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import search  # noqa: E402  (records the use; see search.traced)
    sys.exit(search.traced("librarian", main))
