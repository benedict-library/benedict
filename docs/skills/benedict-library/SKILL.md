---
name: benedict-library
description: Use Benedict to find original sources, check a previous decision, and leave a handoff another agent can verify. 도서관 검색·열람·회의·인계에 사용합니다.
license: MIT
compatibility: Requires the installed doseogwan CLI for the assigned library.
---

# Working in Benedict

Start inside the library assigned to this task. Use the same procedure across agents.
Run the following commands through the installation's library entry point.

1. Search for earlier decisions, relevant skills and originals with
   `doseogwan search --question "original task" "search terms"`.
   Open relevant hits with `doseogwan read "relative/path.md"`. For a shelf hit, keep its `shelf:` prefix. Record the source identifier and hash when the revision matters.
   For a new session, follow the sources in the handoff and check what still applies.
2. Work alone in the reading room. For a joint decision use `doseogwan room task`
   with the question and source. Give collaborators in this library the task ID.
   Use `doseogwan room --help` for opening and replying to a task.
3. Record the decision, the reason and anything still open:
   `doseogwan save --refs "original path or identifier" "question" "decision, reason and open work"`.
   Use `doseogwan librarian --help` when the task calls for indexing or filing.
4. Cite only evidence the recipient may access. If sources are missing, state what
   is unknown. Keep search terms, filenames, citations and replies within the
   recipient's information boundary. For an external check, use the authorized
   host courier and dispatch a request with `doseogwan room dispatch --room ROOM --question QUESTION`. Read its replies with `doseogwan room responses --id ID` in the same library. The host operator selects and runs the command; the result is filed back into that room. See the bundled courier guide.

For a fresh Docker installation run the documented setup interview. Installation
and host administration belong to the installer. Routine library work uses the
assigned entry point without host credentials or the Docker socket.
