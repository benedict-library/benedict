# Runtime and configuration boundaries

## Docker runtime

Docker starts the Rust `benedict` command. Activation creates the registry, wiki and database. `record` writes an entry for the named room, `room memory` reads its working records, and `recall` retrieves a durable entry by ID. Agents supply the question when they record a task. A search command alone does not archive a conversation. `file` turns a selected record into a Markdown page. See the [installation settings](docker-setup.md) and [handoff checks](verification.md).

A library can opt into a shelf with its own `shelf_root`. Search indexes its Markdown and text originals. Use `read shelf:relative/path` to open the original and obtain its SHA-256 digest. Reindex after changing an original. Keep separate registries and volumes for libraries whose contents or names must remain private.

When a library's prebuilt index is missing, damaged or made for other files, `benedict search` reads that
library's documents directly, ranks them the same way and says why under `fallback` (`indexed_at` is then null).
The Python compatibility client below still refuses such a search.

## Python compatibility and local file/index mode

`bin/library.py` reads an explicit JSON registry (default `./library.json`) and selected Markdown root. It does not
load hidden room settings, contact SQL or invoke SSH. Python 3.10+ standard library is sufficient. Init/save build
the persistent lexical index; external edits require `index` or the librarian. Searches use the prepared index,
not an implicit full corpus read. The local BM25 search scores whole words. The compatibility room search also scores Korean two-character sequences and weights titles and tags, so the two modes can rank the same pages differently.

```json
{"version":1,"libraries":{"main":{"name":"Example library","wiki_root":"wiki","central":true}}}
```

Relative roots resolve from the config parent; explicit absolute roots are supported. An optional `dirs` list
(for example `["entities", "concepts", "queries"]`) limits a library's documents to those folders inside its root. An optional `cards` list of path prefixes (for
example `["entities/skill_"]`) turns on the catalog-record layer: search shows the closest catalog records (such as
skills) above the results — by vector distance when embeddings are on, otherwise by word score, marked as such. Default selection requires
exactly one central entry. An optional `reads` list names the other libraries a library may search: without
`--library`, the central library's top word hits from each are added under `near`, and `room_sync.py` copies only
those rows into the search room's registry copy (`room` default `<key>-search`, `search` `room` or `box`, `maxd` the
vector distance limit for near hits). It goes one way — the central library is read by no one, and two libraries never
read each other. A row served only by a room elsewhere may omit `wiki_root`; local commands then refuse it. Closed entries are refused; `save_block` maps rule names to regexes. Each call selects
one root. The registry is trusted configuration, not an authorization grant or a public API.

Read paths are relative Markdown paths. Traversal and external symlinks are rejected. Saves remain in the root,
require a nonblank answer, apply the configured save rules, and return errors on refusal. Atomic page replacement plus log append
is not a multi-file transaction: an error can leave a written page. Local filenames use a question-hash suffix;
changes to the same question on the same local day retain the previous answer and references in the same page. Earlier pages with only a date keep that date and mark the time as unknown. The legacy SQL writer keeps its slug-only contract.

## Bundled room mode

`bin/room.py` runs the persistent room program in `runtime/kernel_room.py`. Set `LIBRARY_ROOM_HOME` to an explicit,
dedicated absolute data directory. `bin/room_sync.py` creates selected-library search, reading, meeting and librarian
rooms. Search holds indexes; reading holds original texts; meeting holds the mailbox `mail` (`bin/mail.py put|get|list`); librarian holds its journal.
Stopping/restoring a room uses its snapshot. Room names are `<library>-<kind>` unless the registry row's `rooms` maps
`search`/`reading`/`meeting`/`librarian` to other names. A state file under `.library-state` remembers what was sent:
later runs send only changed and deleted documents, `--full` sends everything, and `--path FILE` is the save hook. A
room that no longer holds what the state says (restart, tokenizer change) is refilled in full by the next run. Saves
with `LIBRARY_ROOM_HOME` set update the rooms at once this way.

The room sync still checks and copies two older room-search choices for a deployment's own room client: `sections`
(`today`, `meeting_vars`, `decisions`) and `terms`, a JSON file copied into the search room as `library_terms`.
`benedict search` does not print them.

`bin/librarian.py --clean` tidies room variables by the registry's `clean` choice (`true` for the bundled rules, or
per-room rules: `keep`, `prefixes`, `expire_days`, `expire_prefixes`, `shells`, `unknown: report`): expired working
names and names outside the rules are dumped as JSON first, then removed, and a journal line is written. `ROOM_` names
(the room's settings, such as the sync's `ROOM_DOC_ROOT`) stay whatever the rules say.
`copies` (`{name: {"file": path} | {"command": "..."}}`, optional `timeout`) puts other copies into the search room after
each sync — a file's JSON, or the JSON a command prints (run in the registry's folder) — each as one variable the
snapshot can keep (over the per-variable cap is refused). A failing copy keeps the old value and fails the cycle; the
journal line `사본 올리기` names each result. Where the values come from (a file, a database query) stays in your registry.
Three more choices follow it, each `not configured` until set. `mail_reap` (`true` or `ttl_days`, `export_dir`) writes
mailbox topics whose last reply is older than `ttl_days` (7) one JSON file per topic, checks the size and removes them
from `mail`. `restart` (`true` or `rooms` (reading), `after`: `clean` or `cycle`) stops and starts the rooms so Python
returns freed memory to the system: variables over the snapshot's per-variable cap are dumped as JSON first, a variable
neither can keep stops the restart before it begins, and a full sync refills documents and index (searches are
lexical-only and partial until it ends). `journal_db` (`true` or `dsn_env` (`LIBRARY_JOURNAL_DSN`), `table`
(`library.journal`, see `journal.sql`)) inserts every journal line with `psql` (`LIBRARY_PSQL`) before the room copy;
a failing insert fails the step. `--reap` and `--restart` run one alone.
`bin/registry_export.py` writes the JSON registry from `library.registry` (the `options` column carries the other
choices), so a database table can stay the one source.

Rank learning: when `<wiki_root>/.wiki-pending/` exists, rooms log each search's top word and vector hits there, and a
save with `refs` labels the matching search (Beta posterior per source in `rrf_weights.json`, the fusion weights rooms
read; `room_sync.py` also copies it into the search room as `rrf_weights`). Without the folder it says
`not configured`.

An optional `index_block` map (name -> regex) keeps a matching document's body out of rooms, local indexes and
vectors: only its front matter (or first line) and a note are indexed; `read` still opens the original file.
For a large library, `records` (true, or `{"chars": N}`, default 400) makes the reading and search rooms hold each
document's bibliographic record — its front matter and the first N characters of its body — instead of the whole
body, so room memory stays small; vectors still embed the indexed text, and `read` opens the original file.
A `ROOM.route` file in `LIBRARY_ROOM_HOME` forwards every command for that room to the command prefix it holds, for
example `docker exec -i <container> python3 /app/bin/room.py`; callers keep the same room name and commands. This is trusted Python execution with trusted pickle state, not an
untrusted-code sandbox. Isolate each library by container/OS identity; mount original shelves read-only.

The included lexical room fuse makes a room host available without company modules. Hybrid search needs an
embedding server (`LIBRARY_EMBED_*`, see `runtime/wiki_embed_client.py`) and PostgreSQL with pgvector
(`LIBRARY_PG_DSN`, table `embeddings.sql`). `bin/embed_sync.py` embeds new and changed documents (same hash and model
are skipped; `--path` for one saved page, `--chunks` for 1,000-character pieces); with both settings, saves embed the
saved page at once. Setting no embeddings is explicit lexical mode,
not permission to hide a failed vector configuration.

## Existing deployment adapter

On Windows, keep the legacy room-state directory under a private user profile protected by its Windows access-control list; the POSIX token-file mode alone does not grant account isolation there. The native Unix room service is shipped inside Linux containers.

The legacy reading client accepts explicit `--attach` to bring a local document into a room. Every recipient with access to that room may read the attached contents, including after snapshot restoration.

Search is `benedict search`; `bin/read.py` and `bin/save_query.py` remain separate entry points. The room clients read
`LIBRARY_*` from the environment or a selected `LIBRARY_SEARCH_ENV` file; save uses `DATABASE_URL` and `psql`.
Do not copy deployment values into this repository.

- Room transport: `LIBRARY_ROOM_CMD`, optional `LIBRARY_ROOM_EXEC`; registry room: `LIBRARY_REGISTRY_ROOM`.
- Optional vector mode: `runtime/wiki_embed_client.py` (settings `LIBRARY_EMBED_URL` — `http://…` or `unix:///…`,
  `LIBRARY_EMBED_MODEL`, `LIBRARY_EMBED_DIM`, `LIBRARY_EMBED_PROMPTS`, `LIBRARY_EMBED_FALLBACK`), `psycopg2` in the
  image built with `--target vector`, a compatible vector table, and the database address in the room's
  `LIBRARY_PG_DSN` (or room `ROOM_PG_DSN`). Unset means lexical mode, shown in results.
- A container with no network can still reach exactly two services through Unix sockets that a separate gateway
  (`bin/gate.py --route <socket>=<host>:<port>`, standard library) publishes in a shared folder: the database socket `.s.PGSQL.5432` (DSN `…@/db?host=<folder>`) and the
  embedding server (`LIBRARY_EMBED_URL=unix://<folder>/embed.sock`).
- The optional trace settings (`LIBRARY_TRACE_*`, read by `bin/common.py`) keep their deployment behavior; the Rust CLI
  does not load them.
- Web search: `benedict search --web` runs `LIBRARY_WEB_CMD` after the inside results and puts its exit code and output
  under `web`. The setting is a shell command line; the library key and the search terms are its last two arguments,
  and nothing else is sent. `LIBRARY_WEB_TIMEOUT` (seconds, default 540) bounds it. It normally runs
  `bin/web.py` on the machine holding the shelf. There, `LIBRARY_WEB_SEARCH_CMD` is the means: it gets the question as its
  last argument and prints `{"summary": text, "sources": [{"url": ..., "note": ...}]}`. Which means runs is a deployment
  choice; a means that costs money is the deployment's decision. A library takes part only with a `web_shelf` in its
  registry `options`, so originals stay inside its information boundary. Optional `web_save_cmd` names its writer
  (question and answer appended, it prints `저장: <path>`; default `bin/save_query.py`) and `web_block` adds refusal rules
  on top of the built-in ones and `save_block`. The refusal rules catch shapes, not meaning: a question that describes
  private matters in plain words passes them, so the asker still decides what is public.
  Production HTTP(S) connects directly to a validated numeric public address at each hop; TLS verifies the original host name. Environment proxy settings do not apply to this fetcher.
  HTML gets a text copy beside the original; other binary types such as PDF are kept without text. Without `LIBRARY_WEB_CMD`, `--web` sends nothing, exits 1 and names `web-source` for a source the agent found itself.

## Additional required responsibilities

`bin/librarian.py` performs explicit index/room synchronization and journals outcomes. It accepts an explicit golden
file and optional local backup directory. Missing golden evidence is warning/exit 1, not success; a failed cycle
returns 2. Finite `--iterations` bounds repeated work; production scheduling is not automatically enabled.
`bin/evaluate.py` measures
retrieval rank and optional offline answer-key coverage. `bin/publish_skills.py` reads only committed local Git
skills, preserving YAML/body and immutable commit/source/tree provenance in the chosen library. Its optional
`--source-url` is a credential-free HTTPS Gitea-compatible repository URL used only to render links, never fetched.
It refuses dirty skill sources, unmanaged destinations and divergent releases; older ancestor releases are skipped.
Candidate validation precedes writes; concurrent/distributed publication is not transactionally locked.

`bin/wiki_backup.py` snapshots an explicit wiki into a verified local Git bundle and manifest archive, then can
restore to a new target with document-hash checks. It does not set a deployment remote, push, schedule NAS copies,
dump DBs, preserve room state or implement retention. These rollout gaps remain explicit in the
[nine-part scope audit](scope-audit.md).

Snapshot archives contain the selected Git history, not only current Markdown. Treat the entire history as private
until reviewed; an archive is not a sanitization/export operation. The output archive is owner-only. Git must be
installed in the selected execution environment; the default Docker image includes it. No package is
silently installed to make an unavailable backup command look successful.

## Container boundary

One information boundary uses its own Compose project, volume, room service and registry. Compose mounts the shelf originals read-only, keeps the wiki (encyclopedia, skills,
filing cabinet), indexes and room snapshots on its writable named volume, gives the search room network access for
web search, publishes no ports and runs as a non-root account. Change a shelf original at its own source with
history (a Git commit or a new version file); the library reads it again. Parallel verification uses a distinct
project name and fresh volume. Image/dependency acquisition is an operator action requiring approval; no runtime
command installs dependencies or changes daemon configuration. Pin a validated base digest for reproducibility.

The shipped synthetic suite and isolated container checks demonstrate only the paths actually listed in
[verification](verification.md). Production caller migration, three-host/agent tests, old-wrapper retirement,
permission graphs, remote backups and disclosure/history review are separate and incomplete. Do not declare the
full operational plan complete on the strength of the minimal CLI or fake-room tests alone.
