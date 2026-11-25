# Install Benedict with an agent

This file is for the agent that installs Benedict for a user. Read it to the end before running any installation command.

OBJECTIVE: Install a Benedict library into the user's project folder while asking the user as little as possible: one question about where to start, and a second one only if the installer finds something the user must decide.

DONE WHEN:

- The installer prints `Benedict is ready in PROJECT` and its summary.
- `PROJECT/.benedict/library room ping` prints `"ready":true`.
- You have shown the user the summary, including how to change a choice.

## TODO

- [ ] Ask where the library starts from
- [ ] Check that Docker and jq work
- [ ] Run the installer
- [ ] If it stops, tell the user the reason and the proposed fix, and run it again after they agree
- [ ] Show the summary

## 1. Ask where the library starts from

Ask one question and wait for the answer: **“어떤 자료로 시작할까요?”** — the bundled example, an empty library, or the user's own wiki folder. If the user names a wiki folder, resolve it to an absolute path and show it. Ask nothing else now: the installer chooses the rest and reports it.

## 2. Check that Docker and jq work

Run `docker info --format '{{.ServerVersion}}'` and `jq --version`. If either fails, tell the user which one and stop. A permission error from Docker means the user must be in the `docker` group or start Docker Desktop. On Windows, run the installer inside WSL 2 (`wsl`), not in PowerShell or cmd.

## 3. Run the installer

From the Benedict checkout. `PROJECT` must already exist.

```bash
bin/install-project --start example PROJECT
bin/install-project --wiki WIKI PROJECT
```

The installer prints one line per step and takes care of these choices:

| Choice | Default | What the installer checks |
| --- | --- | --- |
| Memory limit | 1 GiB per container up to about 50,000 pages, 1 GiB more for each further 50,000, at most two thirds of Docker's memory | If initial indexing runs out of memory, it doubles the limit and tries again |
| Wiki access | — | Writes a test file as UID 10001, the account the library runs as |
| Database | SQLite in the project's data volume | — |
| Shelf | None | — |
| Librarian | Started: rebuilds the index once a day | — |
| After a reboot | The library starts again by itself | — |
| Semantic search | On when a 2 GiB embedding server fits beside the library in two thirds of Docker's memory; off with PostgreSQL | Downloads the embedding model once (558 MB) and checks its SHA-256. The choices and their costs: [docs/embedding.md](docs/embedding.md) |

Pass an option only if the user asked for something else: `--memory SIZE`, `--database postgres --database-secret-file FILE`, `--shelf DIR`, `--autostart off`, `--librarian off`, `--semantic on|off`, `--embed-url http://HOST:PORT` (an embedding server that already runs, reachable from the containers; no embedder service is started). On macOS, Docker runs containers in a virtual machine without the GPU, so the embedder container is slow for wikis of thousands of pages: when the summary shows semantic search on with the embedder container, tell the user that [docs/embedding.md](docs/embedding.md) explains turning it off or running the server on the host, and let them choose. If you want to relay progress yourself, add `--progress json`: standard output then carries one JSON event per step (`step`, `done`, `error` with a `reason`).

## 4. If the installer stops

It prints the step, the reason and the log path (`PROJECT/.benedict/install.log`). Tell the user all three in one message.

- **The library cannot write the wiki.** The installer prints the exact `setfacl` command. It gives UID 10001 read and write access to that folder only. Before proposing it, run `getent passwd 10001`: if an account is printed, that person could also edit the wiki, so say so. No output (exit status 2) means no account uses that UID. Run the command only after the user agrees, then run the installer again with the same options. Run in a terminal, the installer asks this itself.
- **Docker cannot give enough memory.** The installer warns when the recommended limit is larger than two thirds of Docker's memory. Tell the user they can raise Docker Desktop's memory or use a machine with more memory.
- **The bundled skill page was edited.** An upgrade replaces `entities/skill_benedict-library.md` when it is the page an earlier version wrote. If someone changed it, the installer stops. Ask the user before moving their version to another name (for example `entities/skill_benedict-library.local.md`), then run the installer again.
- **Anything else.** Show the reason line and the last lines of the log. Do not change files or permissions without the user's agreement.

To retry, run the installer again with the same options. Once step 3 has written `PROJECT/.benedict/profile.json`, `bin/install-project PROJECT` with no options also retries with the same choices. Semantic search can be turned on or off later with `bin/install-project --semantic on|off PROJECT`; a run with the image that last indexed the project keeps the index instead of indexing the wiki again. To change another choice after that, choose a new project folder.

## 5. Show the summary

When the room service starts, the search room loads the index into memory (about 12 seconds for 31,000 documents); a search in that time fails with `still loading the index` and works when repeated. The summary lists where the library started from, the number of indexed documents, the memory limit and why, the database, the librarian and what happens after a reboot, with the options that change each of them. Show it to the user as it is. Then run `PROJECT/.benedict/library room ping` and report the result.

## Upgrading an installed library

Use this section instead of steps 1 to 3 when the user asks to update or upgrade Benedict and `PROJECT/.benedict/profile.json` already exists. Ask nothing.

From the Benedict checkout:

```bash
git pull
bin/install-project PROJECT
```

The installer builds the new image, starts the room and the librarian again from it and indexes the wiki again when the image changed (it records the image in `PROJECT/.benedict/indexed-image`). The profile, the wiki, the shelf and the data volume stay as they are, so the choices made at installation are kept. Do not pass `--start` or `--wiki`. If the librarian was turned off at installation, pass `--librarian off` again; without it the installer starts the librarian.

On a small server, build the image on another machine from the same commit (`docker build -t benedict-library .`), move it to the server with `docker save` and `docker load`, and run `bin/install-project --image benedict-library PROJECT` there; the server then does not compile.

Do not rebuild or restart the containers with `docker compose` yourself: that can leave the librarian on the old version. If the installer stops, follow step 4. Afterwards do step 5, and read the [changelog](CHANGELOG.md) for anything a version asks you to do by hand.

## EXECUTE NOW

Start with step 1. For an installed library that the user wants updated, go to “Upgrading an installed library” instead.
