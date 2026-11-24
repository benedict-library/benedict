# Working in a library
Start with the shared [Benedict library skill](docs/skills/benedict-library/SKILL.md). Docker activation adds it to the searchable wiki as `entities/skill_benedict-library.md`.

When a task depends on project knowledge, start with the project's library. Use the central library to find the relevant project or subject when its location is unclear. Open the original, check why the earlier decision was made, and apply that reasoning to the task in front of you. If the conditions have changed, explain which part of the decision needs reconsideration.

When local sources do not answer a public question, use the web search capability already available to your agent. Search with public terms only; keep private project details local. Choose the original source and archive it with `benedict web-source --config REGISTRY --library KEY "public question" "source URL"`. Read the returned `read_path`, verify the evidence, then file the answer with its source. No provider registration or shared search quota is needed from the library. If the agent lacks web search, report that limitation.

At the start of a task, search for the skill that covers the work as well as the sources and earlier decisions. The skill is a wiki document linking the procedure to its sources on the shelf. Open its catalog card, then read the referenced SKILL.md and any companion files you need in the reading room. Apply the procedure to the current task and check the result against the sources.

Work alone in the reading room. For shared work, put the sources and the open question in a named meeting-room task so colleagues can work from the same evidence. File the outcome, reason and source in the library that owns the work. Keep unresolved questions visible so a new session can pick them up.

Read the [library method](docs/method.md) or [한국어 운영 방법](docs/method.ko.md). The [agent guide](docs/agent-usage.md) shows the commands.

For a new installation, use the start source the user requested. If unspecified, ask the start question before running any installation command. Use --wiki DIR for an existing wiki or --start example|empty for the requested starting point. For Docker installations, follow [install.md](install.md): ask only the start question, let the installer choose and check the rest, and ask the user again only when the installer stops with a reason. For an unattended interview with recorded answers, see the [installation interview](docs/docker-setup.md#installation-interview). Pass the original user question separately from the agent’s query when recording a search.

When installing for a user, list existing skill and wiki candidates by name. Use the README import preview and show its resolved sources, included files and destination. Wait for user confirmation for that selection before importing. If the source cannot be read, report the path and stop that import.

For each agent the user wants to use a skill, check the prerequisites in that agent’s actual environment. Use a read-only check or a dry run when available. If a prerequisite blocks use, tell the user which agent and step failed, what would change, and where. Ask whether to apply that specific change or leave the skill as imported. Import approval covers copying only. Wait for a separate answer before editing the skill or its environment. After an approved edit, rerun the same check as the affected agent and report what it verified. If you could not test, say so.
