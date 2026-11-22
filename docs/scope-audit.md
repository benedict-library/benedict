# Nine-part scope audit

For the current Docker distribution, use the [Rust installation guide](docker-setup.md) and [fresh-install checks](verification.md). The table below describes the file CLI and Python compatibility adapters. The default Rust room service uses SQLite or the configured PostgreSQL backend. Semantic curation is work performed by the librarian agent; automated maintenance indexes documents, evaluates retrieval, and files a completed observation with its sources.

The full information-management library requires all nine responsibilities below. A file-only CLI is a useful
entry point, not a substitute for the room, librarian, evaluation, skill and durability layers. Embeddings may be
disabled; those responsibilities may not be relabelled optional to declare the full plan complete.

| Required part | Executable implementation | Adapter/example | Remaining limits |
|---|---|---|---|
| 1. Registry | `bin/library.py init`; versioned local JSON | `examples/portable.json`; SQL `registry.sql` | No new production registry migration or multi-library authorization server |
| 2. Search room/search | Persistent local lexical index; `bin/search.py` room client | `bin/room_sync.py` loads bundled room index | Semantic/vector model/provider/DB setup remains optional and unbundled |
| 3. Reading/read | `bin/library.py read`; `bin/read.py` loads original into a named variable | `main-reading` room and `reading_<topic>_<date>` variables | Markdown only; not a PDF/media reader |
| 4. Filing/save | `bin/library.py save`; quality/rule checks, page and log | Existing SQL-backed `bin/save_query.py` | Multi-file transaction and concurrent-writer coordination incomplete |
| 5. Room runtime/sync | `bin/room.py`, `runtime/kernel_room.py`, `bin/room_sync.py` | Search/reading/meeting/librarian rooms with snapshots | Trusted code/pickle only; no hostile-code sandbox; no production host cutover |
| 6. Librarian | `bin/librarian.py` sync/index/journal cycle | Once or bounded/operator-controlled interval; optional golden run | No general non-Markdown extraction or production cron migration |
| 7. Golden evaluation | `bin/evaluate.py` rank/hit/timing and optional offline key coverage | `examples/golden.jsonl` synthetic questions | Key coverage is not semantic correctness; no answer-generation service or private benchmark |
| 8. Skill intake | `bin/publish_skills.py` committed Git blob to provenance-bearing card | `examples/skills/` fictional skill copied into a committed source repo | No remote fetch/push, deployment mapping, distributed lock or review DB |
| 9. Wiki git/backup | `bin/wiki_backup.py snapshot/restore` local Git bundle with manifest | Restore to a new target and verify document hashes | No remote/NAS backup schedule, DB dump, room-snapshot backup, retention or production migration |

## Executable baseline versus full rollout

The isolated baseline implements all nine rows within the stated limits. Verify the actual tests and container
round trips in [verification](verification.md), not merely the existence of entry points. A passing synthetic
baseline does not establish production readiness, multi-host compatibility or real retrieval quality.

The full rollout still requires separately authorized production caller migration, three-host/agent verification,
legacy wrapper retirement and zombie checks, real access-control verification, deployment backup/retention,
and a separate disclosure/license/history review before outside access. None of those operations is performed by
these local commands. No source-history rewrite, production wrapper deletion or public release is included.

The reference synchronization/daily routines perform room/index synchronization, evaluation and journaling; they
are not themselves Git snapshot/restore implementations. The bundled Git-bundle path is an explicitly defined
durability implementation for the plan, verified by local clone/hash checks, not a claimed verbatim port of a
nonexistent backup routine. It commits Markdown in the explicitly selected wiki and never contacts a remote.

## Optional adapters, not optional responsibilities

- **Embeddings:** choosing no embeddings is supported lexical mode. Optional hybrid deployments must provide their
  provider/model and vector DB schema; BM25 results must not be called semantic results.
- **External SQL registry:** an alternative to the local JSON registry; the registration responsibility remains.
- **Remote transport/backup storage:** deployment choices. Local snapshots do not satisfy an off-host disaster-
  recovery policy until operators separately configure and test external copies.
- **Content adapters:** Markdown is the supported source format. Other formats require explicit ingestion adapters;
  keeping the original shelf read-only does not imply its binary contents are searchable.

Private scientific/value-research systems and datasets are outside this information-management package. Examples
and golden questions are synthetic. The method documents explain this implementation without copying an
organization's governance corpus or making a second authority for private policy.
