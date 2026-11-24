# Install a library with an agent

An agent can build the Docker image, create an installation profile and activate it.
The host needs Docker, Compose and jq. From a source checkout, resolve the start choice before building the image with `docker compose build library`. When a release supplies a prebuilt image, use its pinned image. Installer and runtime commands execute inside that image.

Before activation, show the workspace, storage and access modes, memory limit, and project instruction files to the user. Obtain approval for these changes.

## Install directly from a terminal

Run `bin/install-project /absolute/workspace` to choose a wiki directory, the example, or an empty library (SQLite, bundled read-only shelf, 1 GiB per container). For unattended installation, pass the user’s selection as --wiki DIR or --start example|empty before the workspace. A confirmed profile already supplies this choice. The installer builds and activates the library, then adds `.benedict/library` and a project-local instruction block to AGENTS.md, CLAUDE.md and GEMINI.md. Existing instructions are preserved. Start agents in that workspace. The host launcher needs Docker access and the installation checkout. An agent already running inside the corresponding library container uses its native client. The workspace should contain only instructions suitable for these agents; resolve any conflicting project instructions before work begins.

## Installation interview

Begin with **“에이전트와 페르소나를 위한 나의 도서관을 만듭니다.”** and **“어떤 자료로 시작할까요?”**. Use the host agent’s clickable question tool, one question at a time. Read its wording and options from the Rust CLI:

```bash
benedict setup question start --wiki-found
benedict setup question database
benedict setup question memory
benedict setup question shelf
```

Use `--wiki-found` only when a candidate wiki has actually been found in the approved scope. This recommends **내 위키 연결하기** first; otherwise the first choice is **예제 자료로 시작하기**. An undiscovered wiki can still be selected by providing its path. The recommendation is a suggestion, never consent.

Show the recommendation first with its reason. Count **기타 (직접 입력)** as one of at most three choices. When the host adds a built-in free-text choice, use it for `other` and omit the duplicate. If buttons are unavailable, show the same numbered choices and accept a number or the user’s words. Keep the displayed wording and ordering from the question output so the stored prompt matches the user’s choice. Resolve free text before applying it. Never silently map an unrecognized answer to the recommendation.

Use one installation profile as the authority for the selected project, database type, storage mounts and memory limit. Suggest a project name from the selected workspace and show it for confirmation together with the resolved paths. The 512 MiB starting limit applies to each container. Where PostgreSQL needs credentials, request only a reference to an existing protected secret file; keep the connection string out of chat, the interview record, and wiki pages.

### Record each answer as it happens

Use a private settings directory for this installation and one stable project name. Run the interview commands in the same image version used to activate the project. Mount only the settings directory while asking questions. In the shell example below, `bootstrap` runs on the host; the user answers through their agent.

```bash
library_settings="${XDG_CONFIG_HOME:-$HOME/.config}/benedict/research"
install -d -m 700 "$library_settings"
project=research
image="${LIBRARY_IMAGE:-benedict-local}"
bootstrap() {
  docker run --rm -i --user "$(id -u):$(id -g)" \
    --volume "$library_settings:/settings" "$image" setup "$@"
}
bootstrap question start
```

Immediately after the actual response, pass a JSON object to `bootstrap journal append --project "$project" --buffer /settings/interview.json` on standard input. For example, **only if** the agent found no candidate and the user chose the sample, the object is `{"step":"start","wiki_found":false,"choice":"example"}`. Repeat for database, memory and shelf using the IDs in the displayed question. A typed answer uses `"choice":"other"` with the actual nonsecret `answer` and `"nonsecret":true` after checking it. Keep credentials out of this input. Use structured JSON serialization to preserve user text.

The buffer holds the prompt, choices, recommendation and actual selection while the library is being created. Apply the confirmed choices to the profile. After activation, replay the buffer with the command below; verify its receipts before deleting the temporary buffer. Retrying the same buffer returns the original decision IDs. The profile owns the current values; the librarian's database records and wiki pages explain their history.

Show the resolved host paths and access modes before connecting an existing wiki or shelf.
For a bound wiki, run bootstrap as the host user who will open the wiki. The generated profile records that numeric user ID. New filing pages grant that user read/write access through a POSIX access control list. The wiki filesystem must support access control lists when the accounts differ. The container runs as UID/GID 10001. It needs read/write access to the wiki and read access to the shelf.
For an upgrade, regenerate the profile as the host installer. Verify existing filing receipts too. If legacy pages deny access, after approval grant only that user read/write access to the identified pages, as their owner, then repeat the check. The wiki and shelf must be separate directories. Report any missing permission and ask before changing it.

## Install

Run from the repository root using the same bootstrap function and project name. Set `workspace` to the adopter’s confirmed project directory. Create the profile from the answers already collected. For a custom path or an explicitly chosen memory cap, supply the resolved value together with the answers file.

```bash
bootstrap --project "$project" --answers /settings/interview.json --output /settings/compose.json
export LIBRARY_IMAGE="$image"
library_compose=(docker compose -f compose.yaml -f "$library_settings/compose.json")
bin/install-project "$workspace" "$library_settings/compose.json"
"${library_compose[@]}" run --rm -T library setup journal replay < "$library_settings/interview.json" > "$library_settings/receipts.json" || exit 1
# For a bound wiki, verify each saved decision from the actual host account.
bin/verify-wiki-access "$library_settings/compose.json" "$library_settings/receipts.json" || exit 1
```

**예제 자료로 시작하기** seeds the bundled example and its example shelf; it may skip the remaining questions (SQLite, 1GiB, starts again with the host). Any other start must answer the database, memory, shelf and autostart questions: setup stops at the first missing answer and prints that question for the agent to ask. For **내 위키 연결하기**, show the resolved host path and supply `--wiki "/absolute/path/to/wiki"`. **내 원본 폴더 연결하기** requires `--shelf "/absolute/path/to/shelf"` and is mounted read-only. **서가 없이 시작하기** (`--shelf none`) mounts an empty read-only volume; your own library never receives the example shelf. **시동 때 다시 켜기** (`--autostart on`) gives the room and librarian containers `restart: unless-stopped`, so they come back after a host restart; **직접 켤 때만** (`--autostart off`) sets `restart: "no"` for a trial library. Without an installed profile, `LIBRARY_RESTART` sets the same policy in `compose.yaml`. `bin/install-project` accepts the same `--database`, `--database-secret-file`, `--memory`, `--shelf` and `--autostart` answers.
Resolve both paths on the Docker host before generating the profile. Docker rejects missing directories.
The generated profile contains the mount settings and the project name. Running setup again with the
same options reuses the profile. To change a setting such as the memory limit, generate a second profile
file, review the change and select that file in subsequent Compose commands. Keep the same project name
to reuse the existing data volume.

For an existing PostgreSQL database, pass `--database postgres --database-secret-file /absolute/path/to/protected-file`. The operator's secret file contains the connection URI; the generated settings contain the file reference. Verify the runtime account can read that file. The library and room service receive the secret through Docker Compose. For existing environment-based deployments, the previous `LIBRARY_DB_URL` integration still works.

Activation verifies wiki access, shelf readability and database initialization before publishing
`/data/library.json`. An existing registry is preserved. Activation builds the first index so a fresh agent can immediately search the wiki. The default wiki, registry, SQLite database and
generated indexes persist on the project's data volume. A connected wiki remains on its own host mount. The running room service retains active observations in memory and uses the selected database; the library commands connect over the project volume’s Unix socket.

## Verify a fresh container
Activation installs the bundled Benedict library skill in the wiki. Run `benedict search benedict-library` or `benedict read entities/skill_benedict-library.md` through the assigned library entry point. Each information boundary needs its own registry, volume, and room service. Only attach sources the recipient may read.

```bash
"${library_compose[@]}" run --rm library save "Which dispatch day did we agree on?" \
  "Friday" --refs "Approved dispatch memo, section 2" --query "dispatch weekday"
"${library_compose[@]}" run --rm library search "dispatch weekday"
```

Capture the record ID returned by the save command and use `recall --id ID` in a fresh
container to read the original question, search terms, answer and source. Each invocation of
`run --rm` uses a new container while retaining the selected storage. To inspect the assigned limit,
run `cat /sys/fs/cgroup/memory.max` inside a runtime container on a host using cgroup v2.

## Snapshot and restore the wiki

The Rust command packages committed Markdown and its Git history in a tar archive with a
SHA-256 manifest. Use a new archive path outside the wiki and restore into a new directory.

```bash
"${library_compose[@]}" run --rm library backup snapshot --config /data/library.json --out /data/wiki-backup.tar
"${library_compose[@]}" run --rm library backup restore --archive /data/wiki-backup.tar --target /data/recovered-wiki
```

Both commands verify the Git bundle. Restore also verifies the commit and each Markdown file
before the recovered wiki becomes visible. Preserve the database and shelf separately.

## Run a librarian cycle

After activation, run the Rust librarian against the selected library. A golden file contains
questions paired with expected wiki documents. Supply the path visible inside the container.

```bash
"${library_compose[@]}" run --rm library librarian --library main --golden /data/golden.jsonl
"${library_compose[@]}" run --rm library room memory --room librarian --library main
```

The cycle rebuilds the index, opens the retrieved sources during evaluation and writes its
actual start, end and result to the librarian room and the selected database. Use the returned
`record_id` with `recall --id` after replacing the container. A missing golden configuration
or a retrieval miss returns 1. An execution or journal failure returns 2 and stops the loop.
For a finite schedule add `--iterations N --interval SECONDS`. To create a verified wiki snapshot
on each cycle, also supply `--backup-dir /data/backups`.
