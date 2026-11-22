# The courier

Searching, reading, meeting and filing happen inside a library. Real work also needs tools outside it: build, test,
git, a program, a remote host. The courier is the one door for that work. An agent hands it the command and the reason;
the courier runs it, keeps the whole output in a file, shows the agent only what it needs and writes one ledger line.

```bash
cargo build --locked --release --manifest-path courier/Cargo.toml
courier/target/release/courier --why "run the tests before the merge" --timeout 600 -- 'cargo test --locked'
```

## Return a result to the requesting room

The courier runs on a trusted Unix host. Give the host an executable entry point for the assigned library. For the example below, `library-cli` is a host script on PATH with `exec docker compose -p benedict-demo run --rm -T library "$@"` after its shebang. Use the same Compose project, files and storage settings used for installation. The room containers need neither a host shell nor a Docker socket.

In the reading room, dispatch a question and take the returned numeric ID:

```sh
library-cli room dispatch --library main --room reading --question "What size is the release artifact?"
```

On the host, choose a command whose output the requesting library is allowed to receive, a **new** receipt path in a private directory, and the ID from that room. The following example assumes an existing artifact and request ID 7:

```sh
umask 077
receipt_dir=$(mktemp -d)
courier/target/release/courier handoff library-cli main 7 "$receipt_dir/size.json" -- 'wc -c < release.tar'
library-cli room responses --library main --id 7
```

The host supplies the executable command. The question stored by the room is used only as its reason. The courier returns the exit status and bounded output to the same library and room through the assigned entry point, including on command failure. It keeps full logs on the host. Use `room dispatch --room meeting` for a meeting. Its participants can also see the reply in `room open --id ID`.

If the command finished but the library was unavailable during delivery, run `courier deliver library-cli "$receipt_dir/size.json"` from the host. This rechecks the original request and sends the saved result without repeating the external command. After a successful delivery, a repeated delivery of that receipt returns the existing result. An incomplete receipt after a host crash requires checking the external action and host ledger before choosing whether to rerun it.

## What it does

- **Reason with the command.** `--why` is required; a run without one is refused (exit 2). The reason goes to the
  ledger with the command, so the record of why the agent left the library is the run itself.
- **Short output in full, long output cut with a note.** Up to `COURIER_SHOW` characters (default 3000) are shown as
  they are. Longer output shows head lines (up to 20, within a third of the limit) and tail lines (up to 40, within the
  rest) and one line naming the hidden range and the file that holds everything
  (`… 140 lines hidden (lines 21-160 of <file>)`), so what the agent reads stays near the limit however few or long
  the lines are. Nothing is cut silently. Read the hidden part with the library's `read.py --lines`.
- **Same exit code.** The command's exit code is the courier's. A time limit (`--timeout`, default 300 s) stops the
  whole process group and exits 124.
- **Ledger.** `ledger.jsonl` in the state folder: time, reason, command (first 300 characters), exit code, time,
  output and shown sizes, the full-output file, and whether the deny list and sandbox were set. Refusals are there too.

## File work

Writing and editing files go through the same door, so the reason travels with the change and the agent reads one line
back instead of the file:

    courier --why "add the note" --write notes/today.md <<'END'
    new content
    END
    courier --why "fix the limit" --edit src/limits.py <<'END'
    <<<<<<< old
    LIMIT = 10
    =======
    LIMIT = 20
    >>>>>>> new
    END

`--write` refuses an existing file unless `--replace` is given. `--edit` takes one or more blocks and changes nothing
unless every old text appears exactly once. When `COURIER_SANDBOX` is set, direct file operations are refused before opening the target. Use a command inside the configured sandbox for file changes. The deny list is checked against `write <path>` / `edit <path>` too.

## Settings

| Variable | Default | Meaning |
|---|---|---|
| `COURIER_HOME` | `~/.local/state/library/courier` | State folder: `ledger.jsonl` and `runs/` (files mode 600). |
| `COURIER_SHOW` | `3000` | Characters shown in full before cutting. |
| `COURIER_DENY` | not set | Deny file: one rule per line, `name: regex` (case-insensitive); `#` lines are comments. A file that cannot be read or a rule that does not compile is a failure, not an open door. |
| `COURIER_SANDBOX` | not set | Command prefix every run goes through, e.g. a container exec or a sandbox tool. The footer says `sandbox on` or `sandbox not set`. |

Your own rules and prefixes belong in your own files, outside this repository.

## Security: a deny list is not a boundary

A deny list catches accidents (`rm -rf`, a forced push). It does not stop someone who wants past it: encodings,
indirection and another interpreter (`python3 -c …`) get around any pattern list. A study of 1,709 real agent deny
lists found 69–98.6% of them bypassable ([arXiv 2606.15549](https://arxiv.org/abs/2606.15549)). The boundary is
isolation: run the courier, or set `COURIER_SANDBOX` so that every command runs, inside a container or sandbox with only
the files and network the work needs, and keep credentials out of it.

## Why output is cut this way

Silent truncation makes agents act on half a result. Showing the head and tail, saying what was hidden and keeping the
full output behind a path is the common practice for agent tools (head-and-tail preview plus a file the agent can
read further). In one comparison of twelve everyday jobs the courier cut what the agent had to read from 59,402 to
17,117 characters with the same exit codes.

Run the Unix CLI tests with `cargo test --locked --manifest-path courier/Cargo.toml`. To check delivery using the actual Rust room service, build both executables and run from the checkout root:

```sh
cargo build --locked --manifest-path core/Cargo.toml
cargo build --locked --manifest-path courier/Cargo.toml
bash courier/tests/handoff.sh "$PWD/core/target/debug/doseogwan" "$PWD/courier/target/debug/courier"
```
