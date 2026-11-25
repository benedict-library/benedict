# The gate: let agents in other containers use the library

A library runs in its own container. Agents that work in other containers (for example one container per
person) cannot run `.benedict/library`, because that needs Docker on the host. The gate gives them a narrow
entrance: **search and read only**. Results are masked and recorded exactly as on the command line. Other
commands, `--config`, `--web` and unknown options are refused.

The gate opens inside the library's `room` container, so a library stays one container. It is closed unless
you name an address.

## 1. Open the gate

Name the address in the `room` service of the project's profile, then recreate the room. From the Benedict
checkout, with the project you installed:

```bash
profile=~/projects/notes/.benedict/profile.json
jq '.services.room.environment.LIBRARY_GATE = "0.0.0.0:8740"' "$profile" > "$profile.new" && mv "$profile.new" "$profile"
PROJECT=$(jq -r .name "$profile")
LIBRARY_IMAGE=$PROJECT-library docker compose -p "$PROJECT" -f compose.yaml -f "$profile" up -d --no-build room
```

Later runs of `bin/install-project` keep the profile, so the gate stays open. To close it, delete the line
(`jq 'del(.services.room.environment.LIBRARY_GATE)'`) and recreate the room the same way.

If the address cannot be opened (for example the port is taken), the room keeps serving and tries again every five
seconds; `docker logs "$PROJECT-room-1"` says why.

## 2. Put the library on the agents' network

The gate listens on port 8740 inside Docker and publishes nothing on the host. Connect the library's container to
the network the agent containers already use, under a name they can reach:

```bash
docker network connect --alias notes-library AGENTS_NETWORK "$PROJECT-room-1"
```

Every container on that network can then search and read this library. Use a separate network when only
some containers may.

## 3. Give the agents the client and the instruction

Copy `bin/library-client` (Python standard library only) into each agent container, for example to
`~/.local/bin/library-client`, and set `BENEDICT_GATE=notes-library:8740` in its environment.

```bash
library-client search "refund window"
library-client read entities/refund.md
library-client read --pages 1-3 shelf:report.pdf
```

Add a short instruction to the agent's global instructions (for Codex `~/.codex/AGENTS.md`):

```markdown
## Library
When a task depends on project knowledge, search the library first with `library-client search "..."`,
read the original with `library-client read PATH`, and say which original your answer rests on.
```

An agent that supports prompt hooks can be reminded on every request; for Codex, a `UserPromptSubmit` hook
that prints the same line works. Hooks only remind; the instruction is what the agent follows.

A library that runs without Docker can open the same gate with `benedict gate --listen HOST:PORT`.
