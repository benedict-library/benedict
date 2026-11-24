# Agent use: search, originals, filing and mailbox

Start with the [library method](method.md). In an activated installation, run `benedict method` (Korean: `--lang ko`) to read it before selecting a registry.

Use a dedicated library/container and synthetic examples first. The paths below refer only to a checkout and a new
local data directory. Never substitute a production root while testing.

## Working from Docker

Start with the [setup interview](docker-setup.md). In the activated project use `benedict search` to find documents, `benedict read shelf:river-field-note.md` to open the sample original, and `benedict file --id RECORD` to file an answered observation with its evidence. A librarian agent compares sources and records the synthesis with `benedict record --room librarian --question TITLE --answer SYNTHESIS --source REFERENCES`. Publish its encyclopedia page with `benedict file --id RECORD --as concept`. Use the same library for both commands. The maintenance command refreshes the index and reports retrieval checks. Without `--library`, `benedict search` also lists under `near` the top hits of each library the central library `reads`. A library whose index is missing or damaged is searched in its files directly, and `fallback` says so. `--web` adds the deployment's web step after the inside results ([portability](portability.md)). [External work and the courier](courier.md) · [The public square](plaza.md)

## File/index compatibility path

```bash
python3 bin/library.py init --config /tmp/example-library/library.json --demo
python3 bin/library.py search --config /tmp/example-library/library.json --library main refund
python3 bin/library.py read --config /tmp/example-library/library.json --library main entities/refund.md
python3 bin/library.py save --config /tmp/example-library/library.json --library main \
  "When may a fictional product be returned?" \
  "The synthetic refund policy allows an unused fictional product to be returned within 14 days. This answer refers only to the example document and makes no real merchant claim." \
  --refs entities/refund.md
```

Inspect the returned path/text and cite originals. `save` refreshes the index; after edits made by another editor,
run `python3 bin/library.py index --config /tmp/example-library/library.json --library main` or the librarian.

When local sources do not answer a public question, use the web search capability already available to your agent. Search with public terms only; keep private project details local. Choose the original source and archive it with `python3 bin/library.py web-source --config REGISTRY --library KEY "public question" "source URL"`. Read the returned `read_path`, verify the evidence, then file the answer with its source. No provider registration or shared search quota is needed from the library. If the agent lacks web search, report that limitation.

With the Docker Rust CLI, `web-source --question "exact user wording" "public search terms" "source URL"` keeps the user's wording in the local search-room record when database recording is enabled. The fetch adapter receives the public terms and source URL. The receipt includes the archived original's hash and can be found from a new session.


## Persistent rooms and reading variables

Set a new, dedicated room-state directory, then synchronize the selected library. These local commands start only
the example's four rooms. A room executes trusted Python; do not pass untrusted prompts as code.

```bash
export LIBRARY_ROOM_HOME=/tmp/example-library/rooms
python3 bin/room_sync.py --config /tmp/example-library/library.json --library main
python3 bin/room.py vars main-reading
python3 bin/room.py exec main-reading "reading_refund = {p: t for p, t in docs.items() if p.endswith('/entities/refund.md')}; print(reading_refund)"
echo "Check the synthetic refund answer against entities/refund.md" | python3 bin/mail.py --room main-meeting put refund-review
echo "The original says 14 days" | python3 bin/mail.py --room main-meeting --from reviewer put refund-review
python3 bin/mail.py --room main-meeting list
python3 bin/mail.py --room main-meeting get refund-review --clear
python3 bin/task.py --room main-meeting add refund-review /tmp/example-library/wiki/entities/refund.md --key refund \
  --summary "Refund policy: refunds within 14 days for unused products."
python3 bin/task.py --room main-meeting list refund-review
python3 bin/task.py --room main-meeting get refund-review refund --lines 1-20
echo "Policy says 14 days" | python3 bin/task.py --room main-meeting put refund-review check
python3 bin/task.py --room main-meeting get refund-review check
python3 bin/room.py vars main-meeting
```

The original remains in `docs`; the named variable retains the subset actually read. To hand work to another agent,
read each original first, put it up with `task.py add` and the three-line summary you wrote, and give only the task
name and result slot: the agent lists keys and
summaries first and gets only the keys it needs, so no one prints every original at once. The same `get` reads a
result slot once the agent has put its reply there. Meeting replies are task state,
not proof that a human approved a change. Keep question/input/status/reply in the shared room instead of relying on
private conversation memory. Stop each example room with `bin/room.py stop <room>` when its work ends; subsequent
`start` restores trusted snapshot state. Do not stop rooms belonging to another task.

For the reading client, configure the same local runtime explicitly (replace `/path/to/library` with the
checkout location; no external host is implied):

```bash
export LIBRARY_ROOM_CMD='python3 /path/to/library/bin/room.py exec'
export LIBRARY_REGISTRY_ROOM=main-search
export LIBRARY_SEARCH_ENV=/tmp/example-library/no-deployment-settings.env
python3 bin/read.py --library main --var refund --q "14 days" entities/refund.md
```

`read.py` retains originals under `reading_<topic>_<date>` and prints the requested lines. Use `--attach` with a resolved absolute path to explicitly send an additional host file into that room. Its contents then enter the shared reading variable and may persist in snapshots. This local lexical setup
does not configure a vector database; any hybrid fallback notice remains visible. No silent production config is
used in this example.

## Librarian, evaluation, publishing and backup

Run one explicit librarian cycle, or a finite interval loop. Provide a golden file so the absence of quality
evidence is not treated as success:

```bash
python3 bin/librarian.py --config /tmp/example-library/library.json --library main \
  --golden examples/golden.jsonl --iterations 1
```

`--iterations 2 --interval 60` is a bounded two-cycle example, not a production scheduler. The librarian refreshes
index and rooms, evaluates, and journals actual outcomes. A missing golden file is warning; cycle failures are
failures. `--backup-dir` explicitly enables a local backup step and requires Git.

```bash
python3 bin/evaluate.py --config /tmp/example-library/library.json --library main --golden examples/golden.jsonl
python3 bin/publish_skills.py --config /tmp/example-library/library.json --library main \
  --repo /path/to/committed-synthetic-skills --dry-run
```

The Rust import command inventories approved files, copies them, preserves the Git original and writes the catalog card. For native registries Rust checks the committed skill, scans for secret-like content, checks prior provenance and refreshes the index. Registries with advanced policies still use the compatibility adapter.

The publisher requires a local committed `skills/<name>/SKILL.md` tree, rejects skill drafts and never fetches a
remote. Copy the fictional sample from `examples/skills/` into a separate test source repository and commit it there.
`--dry-run` validates without writing; omit it only after selecting the intended library. Skill frontmatter target
metadata is preserved but never silently redirects the destination. Published cards are indexed for search.

For a synthetic wiki in a Git-capable runtime, snapshot and restore to a new directory:

```bash
python3 bin/wiki_backup.py snapshot --config /tmp/example-library/library.json --library main \
  --out /tmp/example-library/wiki-snapshot.tar
python3 bin/wiki_backup.py restore --archive /tmp/example-library/wiki-snapshot.tar \
  --target /tmp/example-library/restored-wiki
```

The snapshot initializes/commits the selected wiki's Markdown if needed and verifies its local bundle. It does not
push or rewrite a source remote. The restored destination must not exist. The archive includes Git history: do
not distribute it without history/privacy review. A wiki bundle does not back up room snapshots or databases.

Read actual failure results: missing evidence, refused saves, failed evaluation and uncertain publication must not
be presented as success. [Method](method.md), [scope audit](scope-audit.md) and [verification](verification.md) define
what this package does and what rollout work remains.
