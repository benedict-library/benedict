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
| Memory limit | From the wiki size (512 MiB for small wikis, 2 GiB for about 20,000 pages, 3 GiB for about 40,000), at most two thirds of Docker's memory | If initial indexing runs out of memory, it doubles the limit and tries again |
| Wiki access | — | Writes a test file as UID 10001, the account the library runs as |
| Database | SQLite in the project's data volume | — |
| Shelf | None | — |
| Librarian | Started: rebuilds the index once a day | — |
| After a reboot | The library starts again by itself | — |

Pass an option only if the user asked for something else: `--memory SIZE`, `--database postgres --database-secret-file FILE`, `--shelf DIR`, `--autostart off`, `--librarian off`. If you want to relay progress yourself, add `--progress json`: standard output then carries one JSON event per step (`step`, `done`, `error` with a `reason`).

## 4. If the installer stops

It prints the step, the reason and the log path (`PROJECT/.benedict/install.log`). Tell the user all three in one message.

- **The library cannot write the wiki.** The installer prints the exact `setfacl` command. It gives UID 10001 read and write access to that folder only. Before proposing it, run `getent passwd 10001`: if an account is printed, that person could also edit the wiki, so say so. Run the command only after the user agrees, then run the installer again with the same options. Run in a terminal, the installer asks this itself.
- **Docker cannot give enough memory.** The installer warns when the recommended limit is larger than two thirds of Docker's memory. Tell the user they can raise Docker Desktop's memory or use a machine with more memory.
- **Anything else.** Show the reason line and the last lines of the log. Do not change files or permissions without the user's agreement.

A stopped installation keeps `PROJECT/.benedict/profile.json`. To retry with the same choices, run `bin/install-project PROJECT` with no options. To change a choice, choose a new project folder.

## 5. Show the summary

The summary lists where the library started from, the number of indexed documents, the memory limit and why, the database, the librarian and what happens after a reboot, with the options that change each of them. Show it to the user as it is. Then run `PROJECT/.benedict/library room ping` and report the result.

## EXECUTE NOW

Start with step 1.
