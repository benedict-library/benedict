# Changelog

All notable changes to this project are written here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Filing accepts nonblank answers of any length in both the local and SQL-backed writers.

- courier: output over `COURIER_SHOW` with 60 lines or fewer came back whole. Head and tail now fit within the
  character limit (head a third, tail the rest, at least one line each) and the hidden range is always named.

- `bin/task.py get` reads a result slot as well as a material; `list` showed result slots but `get` answered
  "키 없음" for them. A material wins when both have the same key, and a missing key lists both.
- `bin/read.py` sends the text of an absolute path that is a file on this machine and on no library's shelf (a
  session's own notes, a repository file) with the read code, because the reading room's machine cannot open it;
  the text lands in the same `reading_<topic>_<date>` variable. Box libraries still read inside their own box.
- Filing-cabinet pages from `bin/library.py save` are named and dated by the local day, like the legacy writer and
  existing wikis; the UTC day put an early-morning page under yesterday.
- `bin/save_query.py` refuses a `wiki_root` that does not exist on this machine instead of creating it, so a library
  whose wiki lives on another machine never grows a stray copy here.
- `bin/web.py` understands the local CLI writer's JSON line (`{"saved": true, "path": ...}`) as well as
  `저장: <path>`, so `bin/library.py save` can be a library's `web_save_cmd`; a refused default save is reported, not raised.
- `bin/search.py` uses a box library's own registry `box_cmd` when it has one, so more than one box library works;
  the single `LIBRARY_BOX_CMD` remains the default.
- `bin/embed_sync.py --prune` removes the library's vector rows (and pieces) whose document is gone from disk; before,
  a deleted page kept its vector and vector search could still point at it. Explicit, because a row's library is
  known only by its path prefix.

### Added

- `LIBRARY_CENTRAL_HITS`: print only the first N of the central library's hits. Heading and record lines stay, one
  line names how many more there are, and `--all` prints them. Unset prints every hit, as before.
- `courier --write <path> [--replace]` / `courier --edit <path>`: file work through the courier — content or
  old/new blocks on stdin, all-or-nothing edits, one line back, the reason in the ledger.
- `courier/`: one door for work outside the library. `courier --why <reason> [--timeout s] -- <command>` runs the
  command, keeps the full output in a private file, shows short output whole and long output as head and tail with a
  line naming the hidden range and file, keeps the exit code (124 on the time limit, whole process group stopped) and
  writes one ledger line per run or refusal. Optional `COURIER_DENY` (accident guard, not a security boundary) and
  `COURIER_SANDBOX` (command prefix for isolation).
- `bin/librarian.py` registry choice `copies`: after each sync, a file's JSON or a command's printed JSON goes into
  the search room as one variable (refused over the snapshot's per-variable cap); a failing copy keeps the old value
  and fails the cycle, and one journal line names each result. `--clean` now keeps every `ROOM_` setting.
- `bin/librarian.py` registry choices `mail_reap` (export then remove mailbox topics past their keeping time),
  `restart` (stop and start rooms after the clean-up or the cycle so freed memory goes back to the system, keeping
  every variable in the snapshot or a JSON dump, then a full sync) and `journal_db` (journal lines inserted into a
  table, `journal.sql`, before the room copy; a failing insert fails the step). `--reap` and `--restart` run one alone.
- `bin/report.py`: keeps a finished report in a library's filing cabinet, then pipes it to `LIBRARY_REPORT_CMD`
  (title in `LIBRARY_REPORT_TITLE`). The library names no chat service — each user connects their own; not set means
  nothing leaves the machine, and a failing command is reported as a failure.
- The square (`plaza/`, Rust): outside the libraries, agents and people ask, answer with sources, vote, reply and
  accept; private details are refused, nothing is deleted, and accepted answers make the leaders table.
- GitHub Actions workflow runs the unit tests and the README quick start on Linux, macOS and Windows.

- Web search: `bin/search.py --web` adds the web after the inside results. `bin/web.py` asks a configured means
  (`LIBRARY_WEB_SEARCH_CMD`) with the question alone, receives each source URL itself, keeps the bodies on the library's
  shelf (registry `web_shelf`) with address, time and hash, says which were not received, and files the summary and links
  through the library's writer. Questions that look like a secret, an inside address or a pasted document stay inside;
  addresses that resolve inside are not opened, redirects included. Without `--web` nothing goes out.
- `bin/task.py`: task materials in a meeting room under short keys with three-line summaries; `list` shows no
  bodies, `get` reads one key (optionally a line range), `put` fills a result slot. Older `{path: text}` materials
  are listed with the same summary.
- `task.py add` requires `--summary`: the reader writes the lines others choose by; a cut head stays only for older
  materials.
- `task.py get --q WORDS` shows only lines holding any word, one line around; summaries take the title from a
  heading after the lead and end a header at its closing `---` even when a value wraps.
- Standalone `bin/library.py` with explicit versioned JSON registry, empty/synthetic initialization, confined
  Markdown read/save and dependency-free BM25 search. No company config, room host or database is needed.
- Synthetic subprocess regressions cover the complete standalone flow, source changes and information boundaries.
- Initial Docker/Compose example (superseded by the current container layout) with an isolated named volume and non-root user;
  bilingual quick start and an explicit required/optional subsystem and legacy adapter dependency contract.
- Local question filenames add a question hash after the slug to avoid overwriting different questions that share
  a slug. Legacy PostgreSQL-backed filenames are unchanged.
- Rooms: `bin/room.py` runs the bundled persistent room host (`runtime/kernel_room.py`, explicit `LIBRARY_ROOM_HOME`);
  `bin/room_sync.py` fills a library's search, reading, meeting and librarian rooms. A `ROOM.route` file forwards that
  room's commands to the command prefix it holds, so a room can run inside its library's container while callers keep
  the same commands.
- Optional Rust tokenizer (`native/`, standard library only, C ABI through ctypes): the same words as the Python
  tokenizer, about 3x faster. The vector image builds it (`LIBRARY_NATIVE=on`); elsewhere `LIBRARY_NATIVE=auto` uses it
  when `runtime/liblibrary_native.so` (or `LIBRARY_NATIVE_LIB`) exists and `off` keeps Python only.
- Librarian, evaluation, skill intake and backup: `bin/librarian.py` (bounded sync/index/journal cycles),
  `bin/evaluate.py` (golden-question ranks and timing), `bin/publish_skills.py` (committed Git skills into
  provenance-bearing cards) and `bin/wiki_backup.py` (verified local Git bundle snapshot and restore).
- Embedding adapter `runtime/wiki_embed_client.py`: local server over HTTP or a Unix socket, optional Gemini
  fallback, chunk helpers; off unless configured. The room search takes the database address from `LIBRARY_PG_DSN`.
  `docker build --target vector` adds `psycopg2-binary` 2.9.10; the default image stays standard-library only.
- Gate `bin/gate.py`: publishes chosen TCP services as Unix sockets so a library container without a network reaches
  only those services (database, embedding server).
- Catalog-record layer as a choice: registry `cards` (path prefixes) is copied to the rooms as `ROOM_CARD_PREFIXES`;
  without a query vector, the layer falls back to word scores and says so.
- Registry rows may name `dirs`, the folders inside `wiki_root` that hold the library's documents.
- Method and scope documents: `docs/method.md` (Korean `docs/method.ko.md`), `docs/scope-audit.md`,
  `docs/agent-usage.md`, `docs/portability.md` and `docs/verification.md`.

- Read: `bin/read.py --var TOPIC [--library KEY] [--q words] [--lines A-B] PATH…` copies originals into the reading room
  variable `reading_<topic>_<yymmdd>` (the room the library's search room points to with `ROOM_READS`, or the library's own
  room) and shows only the lines holding the question words, a line range, or the headings. Paths under another
  library's wiki folder go to that library. Box libraries read inside their box.
- Search: `bin/search.py [--library KEY] "question"` searches every library the asker may read (the central library's
  search room with its side sections, other libraries' near hits, glossary terms, memory) or one library. Rooms run
  `rooms/room_query.py`; box libraries run their own command; a stale or unreachable room falls back to a grep of the
  registry's `wiki_root` and says so. Machine-specific values are settings (`~/.config/library/search.env`).

### Changed

- The default Linux image now runs the Rust library CLI and room service. See [installation](docs/docker-setup.md).

- Container layout follows one library = one container with the library's own storage: shelf originals are mounted
  read-only, the wiki (encyclopedia, skills, filing cabinet), indexes and room snapshots are written to the data
  volume, and the search room has network access for web search. Compose no longer sets `network_mode: none`, a
  read-only root, dropped capabilities or memory/process limits; `examples/shelf` is the synthetic read-only shelf.
- `init --key K --name N` opens a new library under its own key and name; before, every new library was the
  example `main` "Example library" and had to be renamed by hand.

## [0.1.0] - 2026-10-29

### Added

- Registry: `registry.sql` creates `library.registry`, one row per library (wiki root, rooms, searcher, golden set,
  librarian, save rules); `examples/registry.example.sql` adds two made-up libraries.
- Filing cabinet: `bin/save_query.py <library> "question" "answer" ["refs"]` saves one answered question as
  `queries/<yymmdd>_<slug>.md` and one line in `log.md` under the library's `wiki_root`. Answers under 120 bytes are
  skipped; a question, answer or reference matching one of the library's `save_block` rules is refused (exit 2).
  Without the registry row nothing is saved.
- Project files: license (MIT), contributing, security, third-party notes, badges and an overview diagram (English and
  Korean).
