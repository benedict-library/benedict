# The gate: let agents in other containers use the library

A library runs in its own containers. Agents that work in other containers (for example one container per
person) cannot run `.benedict/library`, because that needs Docker on the host. The gate gives them a narrow
entrance: **search and read only**. Results are masked and recorded exactly as on the command line. Other
commands, `--config`, `--web` and unknown options are refused.

## 1. Start the gate

The gate service is off unless you ask for it. From the Benedict checkout, with the project you installed:

```bash
PROJECT=$(jq -r .name ~/projects/notes/.benedict/profile.json)
LIBRARY_IMAGE=$PROJECT-library docker compose -p "$PROJECT" -f compose.yaml \
  -f ~/projects/notes/.benedict/profile.json --profile gate up -d --no-build gate
```

A project installed before the gate existed has no `gate` entry in its profile; install it again into a new
project folder, or give the gate the same entry as `room` in `profile.json`.

## 2. Put it on the agents' network

The gate listens on port 8740 inside Docker and publishes nothing on the host. Connect it to the network the
agent containers already use, under a name they can reach:

```bash
docker network connect --alias notes-library AGENTS_NETWORK "$PROJECT-gate-1"
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
