# Library method

In a Docker installation, records move through four layers:

| Layer | What it holds | Who writes it |
| --- | --- | --- |
| Room memory | Everything said in each room, in order, up to that room's share of half the container's memory limit | The room service, after the transcript line is written |
| Shelf transcript | Every event as it happened, one file per day and room under `/data/shelf/transcripts/YYMMDD/`, kept until `--transcript-days` removes it (default: keep) | The room service (`benedict room append`, and every stored record) |
| Database | The summary an agent saves when work is done: question, conclusion, sources and the transcript lines it covers | The agent (`benedict save --session --turns`) |
| Filing cabinet | Saved records the librarian chose to publish | The librarian (`benedict file --id RECORD`) |

Pass the user’s exact wording with `--question` and the agent’s search terms separately. Filing a record with wiki sources also adds its question to `/data/golden/LIBRARY.jsonl`, so the librarian's retrieval check grows from real questions. Original shelf files stay on the selected storage.

## How agents work in the library

Agents work here to make decisions from original sources and to leave their reasoning where the next agent can use it. An agent reads, judges and acts. A persona is a digital identity through which a person expresses themselves.

When you take on a task, look for earlier project decisions and their reasons. If you do not know where to look, use the central library to locate a library you may read. When working alone, keep the source and your working question together in the reading room. Check whether the earlier reasoning applies to the new task. Carry the decision forward if it does. If conditions have changed, explain what changed. Ask the owner about the judgment that still needs their input.

If local evidence is missing or outdated, use your own web search on public terms. Archive the original with the local `web-source` command, read the saved source, and file the verified answer. The library does not supply a shared search account.

Look up the relevant skill when choosing how to do the work. A skill is a wiki document that organizes a reusable procedure and links to the original research and source material on the shelf. The librarian catalogs the committed original and its revision; the search room finds the card, and the agent reads the procedure and any necessary companion files in the reading room. Use it to carry out the task, then file the result and its evidence.

When working with others, put the sources and the open question in a named meeting-room task once. Colleagues can then open the same material and contribute their judgments. At the end, file the conclusion, its reason, the original source and any remaining question in the relevant library.

Suppose the next agent has to order markers for a field survey. The project chose violet so that the crew could distinguish its markers from existing orange ones. The agent can use that reason to order violet markers for the same site. If the new site already uses violet, the agent can explain why the choice needs another look. The handoff succeeds when the reasoning informs the next piece of work.

## Operating the library

1. **Give each library an information boundary.** One container/identity owns one wiki and its rooms. Mount original
   shelf material read-only. Keep writable wiki, indexes and snapshots on dedicated data storage. A registry entry
   selects a boundary; it does not grant access the OS/container does not already allow.
2. **Find before concluding.** Ask the selected search room/index. Keep retrieval failure, access failure and a
   genuine empty result distinct. Lexical search is a valid configured mode; never disguise it as vector retrieval.
3. **Bring originals into the reading room.** Read the retrieved document, preserve its path/revision and retain a
   named reading variable when collaborating. Search snippets point to evidence; they are not evidence substitutes.
4. **Share bounded work through a meeting room.** Use named tasks and mailboxes. Store the question, inputs,
   completion state and reply; another worker can continue without an undocumented private conversation.
5. **Save the answered question, then let the librarian file it.** Save question, answer, sources and the transcript
   lines in the database. The librarian files the ones worth keeping. Keep quality/rule refusals visible.
   The filing cabinet grows from answers actually grounded in originals, not self-referential summaries.
6. **Let the librarian keep layers in step.** After external edits, refresh lexical indexes and room documents.
   Record actual start/end/outcome in the journal. Failed sync or evaluation must remain failure, not a fresh mtime.
7. **Measure known questions.** Golden questions name expected original documents. Report rank/hit and timing;
   if answer keys are measured, call that key coverage, not semantic truth. Use synthetic data for distribution.
8. **Publish committed skills with provenance.** Import immutable Git content, commit and source/tree hashes into
   the explicitly chosen catalog. Drafts do not replace shared originals. Preserve unmanaged or newer pages.
9. **Protect continuity and prove restoration.** Snapshot the wiki in Git, preserve a verifiable backup, and restore
   to a new location before trusting it. Room state and database backups are separate responsibilities.

Use [agent usage](agent-usage.md) for executable examples and [scope audit](scope-audit.md) for missing rollout work.
When a source changes, update derived records with the new evidence; keep the prior revision in history rather than
letting contradictory records masquerade as current truth.
