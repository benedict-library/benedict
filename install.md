# Install Benedict with an agent

This file is for the agent that installs Benedict for a user. Read it to the end before running any installation command.

OBJECTIVE: Install a Benedict library into the user's project folder after checking the environment, telling the user about any problem the check predicts, and applying only the choices the user confirmed.

DONE WHEN:

- `PROJECT/.benedict/library room ping` prints `"ready":true` and the installed version.
- `PROJECT/.benedict/library search "a word from the user's wiki"` returns at least one hit.
- The user has seen a short report that lists what was checked, what was chosen, and whether the librarian service is running.

## TODO

- [ ] Ask where the library starts from (example, empty, or the user's wiki)
- [ ] Run the preflight checks and record each result
- [ ] Tell the user about every check that failed or warned, with the proposed change
- [ ] Agree on the memory limit and on the librarian service
- [ ] Install
- [ ] Start the librarian service if the user chose it
- [ ] Verify and report

## 1. Ask where the library starts from

Ask one question and wait for the answer: **“어떤 자료로 시작할까요?”** — the bundled example, an empty library, or the user's own wiki folder. If the user names a wiki folder, resolve it to an absolute path and show it. The example needs no further questions; skip to step 2 and use 512 MiB.

## 2. Preflight checks

Run each check yourself before proposing anything. A check can fail because of permissions; if a command is refused, report which command and why, and do not guess the value. Keep every result: you will show them in step 3.

| Check | Command | Why it matters |
| --- | --- | --- |
| Docker access | `docker info --format '{{.ServerVersion}}'` | The installer builds an image and starts containers. A permission error here means the user must be in the `docker` group or start Docker Desktop. |
| Compose and jq | `docker compose version` and `jq --version` | The installer needs both. |
| Memory Docker can use | `docker info --format '{{.MemTotal}}'` (bytes) | On Docker Desktop this is the VM size, not the machine's RAM. The library limit must fit inside it. |
| Free disk for Docker | `df -h "$(docker info --format '{{.DockerRootDir}}')"` | The image, the index and the database live there. |
| Wiki size (own wiki only) | `find "$WIKI" -type f -name '*.md' \| wc -l` and `du -sh "$WIKI"` | Initial indexing reads the whole wiki inside one container under the memory limit. |
| Wiki access for you (own wiki only) | `test -r "$WIKI" && test -w "$WIKI" && echo ok` | You open and edit the wiki as this account. |
| Wiki access for the container (own wiki only) | `stat -c '%u %a' "$WIKI"` and `getfacl -p "$WIKI"` | The library runs as UID 10001 and writes new pages into the wiki. It can write only if UID 10001 owns the folder or an ACL entry `user:10001:rwx` exists. Otherwise the installer stops with `wiki is not writable`. A passing check for you does not mean the container can write. |
| Shelf access (if any) | `test -r "$SHELF" && echo ok` | The shelf is mounted read-only. |

If `docker info` itself is refused, stop here: the remaining checks and the installation all need Docker.

## 3. Tell the user what the checks predict

Before any change, show the results in one short message and ask for the decisions below. Name each finding with what you measured, what it may cause, and what you propose.

### Memory limit

Each library container starts with a 512 MiB limit. That is verified for the bundled example. A wiki of about 42,000 Markdown files (about 540 MB) was killed during initial indexing at 512 MiB and indexed at 3 GiB ([issue #2](https://github.com/benedict-library/benedict/issues/2)). No measurement exists yet between those two sizes, so say so.

Propose a limit from the wiki size you measured, with margin, and keep it at or below two thirds of the Docker memory from step 2 so the host stays responsive while indexing ([Meilisearch guidance on indexing memory](https://www.meilisearch.com/docs/resources/self_hosting/performance/ram_multithreading)). If the wiki needs more than Docker can give, tell the user before installing: they can raise Docker Desktop's memory, use a machine with more memory, or start with a smaller part of the wiki. Choose the limit now — changing it after installation requires a new profile.

### Librarian service

The installer starts only the room service. The librarian service rebuilds the index and runs the golden measurement once a day; without it the index is refreshed only when someone runs a librarian cycle ([issue #3](https://github.com/benedict-library/benedict/issues/3)). Ask whether to start it: **“도서관을 매일 사서가 정리하게 할까요?”**

### Wiki access for the container

If UID 10001 cannot write the wiki, check two things before proposing a change:

- Who else is UID 10001 on this machine: `getent passwd 10001`. If an account is printed, that person would also be able to edit the wiki; tell the user and let them decide.
- That `$WIKI` is the wiki folder itself: show the absolute path and its page count from step 2. Never apply the change to a home folder, `/`, or a folder that holds more than the wiki.

Then propose this change and run it only after the user agrees. It gives UID 10001 read and write access to the existing pages and to pages created later, and nobody else:

```bash
setfacl -R -m u:10001:rwX -m d:u:10001:rwX "$WIKI"
# to undo: setfacl -R -x u:10001 -x d:u:10001 "$WIKI"
```

The filesystem must support ACLs and the account running the command must own the files. If either is missing, tell the user which one; do not open the folder to all users with `chmod o+w`. More on mixed accounts: [Record each answer as it happens](docs/docker-setup.md#record-each-answer-as-it-happens).

### Anything that failed

For each failed check, say which command failed, the message, and the change you would make. Make a change only after the user agrees to that specific change.

## 4. Install

Use the answers, not defaults. From the Benedict checkout:

```bash
# Example
bin/install-project --start example PROJECT
# Own wiki
bin/install-project --wiki WIKI --database sqlite --memory LIMIT --shelf SHELF_OR_none PROJECT
```

`PROJECT` must already exist. Write `LIMIT` as a number with `m` or `g`, for example `512m` or `3g`. The full interview with PostgreSQL and shelf options is in [docs/docker-setup.md](docs/docker-setup.md).

If installation stops:

- `wiki is not writable` — the container check in step 2 was missed; go back to step 3.
- `initial wiki indexing failed` — check for an out-of-memory kill (`dmesg | grep -i -E 'oom|killed process'` on Linux) and tell the user the limit was too small, with the value you would try next.

A stopped installation keeps `PROJECT/.benedict/profile.json`, a running room container and a Docker network. To retry with the same choices, run `bin/install-project PROJECT` with no options; the installer refuses options when a profile exists. To change a choice such as the memory limit, ask the user, then stop the old project and keep its profile as a backup before installing again:

```bash
name=$(jq -r .name PROJECT/.benedict/profile.json)
docker compose -p "$name" -f compose.yaml -f PROJECT/.benedict/profile.json down
mv PROJECT/.benedict/profile.json PROJECT/.benedict/profile.json.bak
```

`down` without `-v` keeps the data volume.

## 5. Start the librarian service (if chosen)

The installer stores the project name in `PROJECT/.benedict/profile.json`. From the Benedict checkout:

```bash
name=$(jq -r .name PROJECT/.benedict/profile.json)
LIBRARY_IMAGE="$name-library" docker compose -p "$name" \
  -f compose.yaml -f PROJECT/.benedict/profile.json \
  --profile rooms up -d --no-build librarian
docker compose -p "$name" -f compose.yaml -f PROJECT/.benedict/profile.json ps librarian
```

Report the state shown by `ps`. If it is restarting or exited, show the last lines of `docker compose -p "$name" -f compose.yaml -f PROJECT/.benedict/profile.json logs librarian`.

## 6. Verify and report

```bash
PROJECT/.benedict/library room ping
PROJECT/.benedict/library search "a word from the user's wiki"
```

Report in a few lines: the checks and their results, the memory limit and why, whether the librarian is running, and the two verification results. If a step could not be tested, say which one.

## EXECUTE NOW

Start with step 1. Do not run `bin/install-project` before step 3 is answered.
