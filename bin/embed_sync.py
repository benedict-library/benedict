#!/usr/bin/env python3
"""Embed a library's new and changed documents into the vector table — only what changed.

usage: embed_sync.py --config C [--library K] [--path FILE] [--limit N] [--dry-run] [--local-only] [--chunks] [--force]
                     [--prune]

Needs LIBRARY_EMBED_URL (and the other LIBRARY_EMBED_* settings, see runtime/wiki_embed_client.py) and LIBRARY_PG_DSN,
a PostgreSQL database with pgvector and the table in embeddings.sql. Without them the command says "not configured".

A document is skipped when its indexed text (index_block applied) has the same hash and the same model as its row.
While the local server is up, rows written by another model are embedded again, so questions and documents share one
space; while it is down, only the configured fallback is used and nothing is re-embedded. Documents under 100
characters are not embedded. --path embeds that one saved page (save hook); --chunks writes one row per 1,000-character
piece ('<path>::c<n>', local server only); --force embeds unchanged documents too. --prune also removes this library's
rows (and pieces) whose document is no longer on disk; it is explicit because a row's library is known only by its path.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "runtime"))
from library_local import Library
import wiki_embed_client as wc

MAX_CONSEC_FAIL = 3
UPSERT = ("INSERT INTO wiki_embeddings(doc_path,file_hash,front_matter,embedding,created_at,model) "
          "VALUES(%s,%s,%s,%s::vector,now(),%s) ON CONFLICT (doc_path) DO UPDATE SET file_hash=EXCLUDED.file_hash, "
          "front_matter=EXCLUDED.front_matter, embedding=EXCLUDED.embedding, created_at=EXCLUDED.created_at, model=EXCLUDED.model")


class PgStore:
    """The vector table in PostgreSQL (embeddings.sql)."""
    def __init__(self, dsn):
        import psycopg2
        self.conn = psycopg2.connect(dsn, connect_timeout=10)
        self.cur = self.conn.cursor()

    def rows(self, prefix):
        self.cur.execute("SELECT doc_path, file_hash, model FROM wiki_embeddings WHERE starts_with(doc_path, %s)", (prefix,))
        return self.cur.fetchall()

    def upsert(self, path, file_hash, front_matter, vector, model):
        self.cur.execute(UPSERT, (path, file_hash, json.dumps(front_matter, ensure_ascii=False),
                                  "[" + ",".join(f"{x:.8g}" for x in vector) + "]", model))

    def delete(self, paths):
        self.cur.execute("DELETE FROM wiki_embeddings WHERE doc_path = ANY(%s)", (list(paths),))

    def commit(self):
        self.conn.commit()

    def by_model(self):
        self.cur.execute("SELECT coalesce(model,'(none)'), count(*) FROM wiki_embeddings GROUP BY 1")
        return dict(self.cur.fetchall())


def front_matter(text):
    parts = text.split("---")
    if len(parts) < 3:
        return {}
    found = (re.match(r"^(\w[\w_-]*):\s*(.+)$", line.strip()) for line in parts[1].splitlines())
    return {m.group(1): m.group(2).strip().strip("\"'") for m in found if m}


def sync(lib, key, store, path=None, limit=0, dry=False, local_only=False, chunks=False, force=False, prune=False):
    key, row, root = lib.selected(key)
    if prune and path is not None:
        raise ValueError("--prune works on the whole library, not with --path")
    _, paths = lib.document_paths(key)
    files = {str(root / rel): resolved for rel, resolved in paths.items() if not Path(rel).name.startswith("_index")}
    only = None
    if path is not None:
        only = str(Path(path).resolve())
        if only not in files:
            raise ValueError(f"--path not in library {key}: {path}")
    local_ok = wc.local_alive()
    target_model = wc.model_name() if local_ok else wc.GEMINI_MODEL
    if not local_ok and (local_only or chunks):
        raise RuntimeError("local embedding server not answering — --local-only/--chunks stop here")
    existing, existing_model, existing_chunks = {}, {}, {}
    for doc_path, file_hash, model in store.rows(os.path.join(str(root), "")):
        parent = wc.chunk_parent(doc_path)
        if parent:
            existing_chunks.setdefault(parent, {})[doc_path] = (file_hash, model)
        else:
            existing[doc_path], existing_model[doc_path] = file_hash, model
    n = dict(embedded=0, reembedded_for_model=0, skipped=0, failed=0, chunks_written=0, removed=0)
    consec = 0
    for doc_path, resolved in files.items():
        if only and doc_path != only:
            continue
        raw = lib.index_body(key, resolved)
        if len(raw.strip()) < 100:
            continue
        slug = Path(doc_path).stem
        fm = front_matter(raw)
        title = fm.get("title", slug)
        file_hash = hashlib.md5(raw.encode("utf-8")).hexdigest()
        if chunks:
            parts = wc.chunk_texts(raw)
            keys = [wc.chunk_key(doc_path, i) for i in range(len(parts))]
            old = existing_chunks.get(doc_path, {})
            if not force and set(old) == set(keys) and all(h == file_hash and m == target_model for h, m in old.values()):
                n["skipped"] += 1
                continue
        else:
            unchanged = existing.get(doc_path) == file_hash
            model_stale = local_ok and existing_model.get(doc_path) != target_model
            if unchanged and not force and not model_stale:
                n["skipped"] += 1
                continue
            if unchanged and not force:
                n["reembedded_for_model"] += 1
        if limit and n["embedded"] >= limit:
            break
        if dry:
            n["embedded"] += 1
            continue
        try:
            if chunks:
                vectors = [v for j in range(0, len(parts), 16) for v in wc.embed_local_batch(parts[j:j + 16], "doc", title)]
            else:
                text = (f"{title}\n{fm.get('tags', '')}\n{slug}\n" + raw)[:wc.MAX_CHARS]
                vector, model = wc.embed(text, "doc", title=title, allow_fallback=not local_ok)
        except Exception as error:
            n["failed"] += 1
            consec += 1
            print(f"embedding failed {slug}: {type(error).__name__}", file=sys.stderr)
            if (local_only or chunks) and consec >= MAX_CONSEC_FAIL:
                store.commit()
                raise RuntimeError(f"local embedding failed {consec} times in a row — stopped after {n['embedded']} documents")
            continue
        consec = 0
        if chunks:
            stale = [k for k in old if k not in set(keys)]
            if stale:
                store.delete(stale)
            for i, (chunk_path, vector) in enumerate(zip(keys, vectors)):
                store.upsert(chunk_path, file_hash, {"chunk_of": doc_path, "chunk": i, "of": len(keys), "title": title},
                             vector, wc.model_name())
            n["chunks_written"] += len(keys)
        else:
            store.upsert(doc_path, file_hash, fm, vector, model)
        n["embedded"] += 1
        if n["embedded"] % 25 == 0:
            store.commit()
    if prune:
        gone = [p for p in existing if p not in files] + [c for parent, cs in existing_chunks.items() if parent not in files for c in cs]
        n["removed"] = len(gone)
        if gone and not dry:
            store.delete(gone)
    if not dry:
        store.commit()
    return {"library": key, "mode": "chunks" if chunks else "doc", **n, "total_on_disk": len(files), "dry_run": dry,
            "target_model": target_model, "local_alive": local_ok, "rows_by_model": store.by_model()}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--config", required=True)
    parser.add_argument("--library")
    parser.add_argument("--path")
    parser.add_argument("--limit", type=int, default=0)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--local-only", action="store_true")
    parser.add_argument("--chunks", action="store_true")
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--prune", action="store_true", help="remove rows of documents gone from disk")
    args = parser.parse_args()
    try:
        if not os.environ.get("LIBRARY_EMBED_URL") or not os.environ.get("LIBRARY_PG_DSN"):
            raise ValueError("embedding not configured: set LIBRARY_EMBED_URL and LIBRARY_PG_DSN")
        result = sync(Library(args.config), args.library, PgStore(os.environ["LIBRARY_PG_DSN"]), args.path,
                      args.limit, args.dry_run, args.local_only, args.chunks, args.force, args.prune)
        print(json.dumps(result, ensure_ascii=False))
        return 0
    except Exception as error:
        print(json.dumps({"error": f"{type(error).__name__}: {error}"}, ensure_ascii=False), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import common  # noqa: E402  (records the use; see common.traced)
    sys.exit(common.traced("librarian", main))
