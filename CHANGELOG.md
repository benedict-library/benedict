# Changelog

All notable changes to this project are written here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `read` and `search` mask personal data before a result is recorded or printed, on the native path and
  through the compatibility adapter. Korean resident and foreign registration numbers keep the birth date
  and one digit (`900101-1******`), mobile numbers keep the first and last groups (`010-****-5678`),
  sole-proprietor business numbers keep three leading and two trailing digits, card, account, passport and
  driver licence numbers and personal e-mail become labels, and a person's name next to a title or label
  keeps its first and last syllable (`홍*동`). Corporate business numbers, landlines and role mailboxes such
  as `info@` stay. Source fields (`path`, `library`, `sha256`) are not changed, so the original can still be
  opened. A result that masked anything carries `"redacted": N`.
- `read shelf:PATH --pages 3-5` converts up to five pages of a non-text original (PDF, HWP, HWPX, DOCX,
  XLSX, image) to Markdown with [kordoc](https://github.com/chrisryugj/kordoc); pages without a text layer
  are read with kordoc's built-in OCR. Without `--pages` the result describes the original and says how to
  read part of it. The image pins kordoc and its parts in `readers/package-lock.json` and copies the Node
  24 LTS binary from the pinned `node` image.
- `benedict room append` keeps room conversation as it happens. The room service writes each event to
  `/data/shelf/transcripts/YYMMDD/LIBRARY-ROOM.jsonl` first, then to that room's memory, and returns its file and
  line. A repeated request ID from today or yesterday returns the earlier line. Without the room service the event
  is refused (`room service not running`); a transcript write failure refuses the event.
- Each room has its own memory window. Together they use half of the container's memory limit (half of 512 MiB
  when the limit cannot be read), and a full room drops its oldest events first. An event larger than its share
  keeps an excerpt and its transcript line. `room ping` reports each room's use and dropped count.
- Meeting tasks, replies, dispatches, `room open` and `room responses` also leave their originals in the transcript
  and need the room service.
- `save --session ID --turns DAY/FILE:A-B` (or `DAY/FILE:A..DAY/FILE:B` across midnight) links a record to its
  transcript lines. Records gain `session`, `turns`, `at`, `kind` and `filed_at`; older rows keep empty values.
- `install-project --transcript-days N|keep` (default keep) sets how long transcripts stay; the summary shows it.
- The librarian removes transcript days past `LIBRARY_TRANSCRIPT_DAYS`, counts saved records not yet filed and
  reads `golden/LIBRARY.jsonl` by default. A missing golden file is `not_configured`; a bad line is `invalid` with
  its line number. Neither stops the cycle.

- Setup asks whether the library starts again when the host restarts (`--autostart on|off`). `on`, the
  recommended answer and the example start's default, gives the room and librarian containers
  `restart: unless-stopped`; `off` sets `restart: "no"` for a trial library. `compose.yaml` uses
  `${LIBRARY_RESTART:-unless-stopped}` for both services, so an installed project without the answer starts
  again with the host after `docker compose up -d`. Before, the room container had no restart policy, so it
  did not come back after a host restart.

### Changed

- `save` keeps one database record (`kind: record`) and no longer writes a wiki page. `file --id` writes the filing,
  adds one golden line per question with wiki sources (another wiki source for the same question extends that line)
  and sets `filed_at`. Running it again fills only what is missing.
- The compose librarian no longer passes the example golden file; only the example start points
  `LIBRARY_GOLDEN` at it.
- An upgrade replaces the bundled skill page (`entities/skill_benedict-library.md`) when it is the page an
  earlier version wrote. An edited page stops the installer with a message that says how to keep it.
- `install-project` asks only where the library starts. For your own wiki it uses SQLite without a
  shelf, starts the librarian, turns autostart on and sets the memory limit from the wiki size with
  `setup --recommend-memory` (measured: about 20,000 pages fit in 2 GiB, about 40,000 in 3 GiB; at most
  two thirds of Docker's memory). If initial indexing still runs out of memory, the installer doubles the
  limit and tries again. A summary shows every choice and the option that changes it.
- `install-project` checks that the library (UID 10001) can write the wiki. In a terminal it asks once
  before granting that UID access with `setfacl`; otherwise it stops with the exact command.
- `install-project` prints one line per step with a running clock and keeps Docker's output in
  `.benedict/install.log`; a failure shows its reason and the log path. `--progress json` prints one JSON
  event per step instead. `install.md` is the guide for installing agents.
- `install.md` tells installing agents how to upgrade an installed library: `git pull`, then
  `bin/install-project PROJECT` again. The upgrade was described only at the end of the README.

- The project entry point (`.benedict/library`) sends each command into the running room container with
  `docker exec` instead of starting a new container for every command. A search no longer waits for a
  container to start. To upgrade an installed project, delete its `.benedict/library` and run
  `bin/install-project` again with the same workspace.
- Search is one command, `benedict search`. Without `--library` it also returns, under `near`, the top hits of
  each library the central library `reads`. A library whose prebuilt index is missing, damaged or made for other
  files is searched in its wiki files directly, and `fallback` says why. `--web` runs the deployment's
  `LIBRARY_WEB_CMD` with the library key and search terms after the inside results and puts its output under
  `web`; without it nothing leaves the library. The Python `bin/search.py` and its room code
  `rooms/room_query.py` are removed, and room-side search (side sections, box libraries, vector hits) is not
  carried over. `bin/common.py` keeps the settings, room calls and use records the remaining Python tools share.

### Fixed

- Korean shelf filenames stored decomposed (as macOS and some network stores keep them) are indexed in
  composed form, and `read shelf:<path>` finds them by the composed name. An original the index cannot
  open is skipped and counted instead of stopping the whole index; when a skipped name contains `?`, the
  message says to mount the network share with `iocharset=utf8`.
- `read shelf:<path>` no longer refuses an original that is not plain text. A PDF, office document
  or image on the shelf is now described by its size and SHA-256 with `"binary": true`, so an agent
  can open the file itself instead of being told the original is unreadable.
- A Docker library bound to your own wiki now gives you read and write access to every page it
  files there. Answers saved through the room and the originals kept by `web-source` were written
  for the container's account only, so the wiki's owner could not open, back up or commit them.
  Pages filed before this change keep their old access; grant it once as described in the Docker
  setup guide.
- Installing your own wiki or an empty library now requires every setup answer. Setup used to finish
  without the database, memory and shelf answers and then mounted the bundled example shelf into a library
  built from your own material. It now stops at the first missing answer and prints that question. The
  shelf question recommends connecting your originals folder or starting with no shelf (`--shelf none`,
  an empty read-only volume); the example shelf comes only with the example start, which keeps its
  one-line installation. `bin/install-project` passes `--database`, `--database-secret-file`, `--memory`
  and `--shelf` to setup.
- The room container now mounts the same wiki and shelf as the library container. Commands sent into it
  could search the index built at installation but could not open the wiki page or shelf original it
  returned. New profiles include the mounts. For an installed project, give the room service the library
  service's mounts in `.benedict/profile.json`
  (`jq '.services.room.volumes = .services.library.volumes'`), then run `bin/install-project` again with
  the same workspace so the room container is recreated.

## [0.2.0] - 2026-11-22

The first versioned public release. Check what is installed with `.benedict/library version`; `room ping` also
reports the running version.

### Changed

- One name: the command is `benedict` (was `doseogwan`), the Rust package is `benedict`, the default image is
  `benedict-local` and the environment variables are `BENEDICT_APP`, `BENEDICT_CLI` and `BENEDICT_BIN`. The record
  table is `benedict_room_records`; the first write after the upgrade renames the earlier table, so its records stay.
  To upgrade an installed project, run `bin/install-project` again with the same workspace so its launcher calls the
  new command.
- The README's install section is four numbered steps: get Benedict, install into a project, check the connection,
  try it with an agent. The Compose sample and the Python commands moved out of the README; the Python commands are
  in `docs/python-compat.md`, which the quickstart test now runs.

### Fixed

- Search finds a word written with its ending: `반품` matches `반품하려면` and `반품은`, and `return` matches
  `returned`. A query word of two or more Hangul syllables matches indexed words that begin with its longest part
  (two syllables or more) found in the library; another word of four or more characters matches indexed words that
  begin with it. Shorter words still match whole words. The index format is unchanged, so existing indexes keep
  working.
- The example refund policy has a Korean sentence as well, so the README's first question finds it in either
  language.
- A search with no hits says why in a `hint`: when the query is Korean and the library holds no Korean words, it
  asks for the documents' language; otherwise it suggests the documents' own or fewer words. Agents no longer read
  an empty result as "no source".
- The Python compatibility search (`bin/library.py search`) uses the built Rust CLI (`BENEDICT_BIN`, `benedict`
  on PATH, or this checkout's `core/target` build) for registries the Rust CLI handles, so both clients rank and
  explain results the same way. Registries with advanced policies keep the policy-aware Python search.
- The project launcher written by `bin/install-project` runs Compose with `--progress quiet`, so an agent reads the
  command's JSON without container progress lines.
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

- `doseogwan version` (also `--version`) prints `{"version": "..."}`, and `room ping` reports the running version.
  Every crate, this changelog's newest release and the CLI name one version; a test keeps them equal.
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
