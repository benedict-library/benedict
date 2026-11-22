### Working in this project

For a project task, make your first command from the project root
`./.benedict/library work open "the user's task" "my first hypothesis or the condition I need to check"`.
This records the starting judgment in the project's reading room. Keep the returned work ID.
If continuing earlier work, find its title in `recent_work` and run
`./.benedict/library work show ID` before deciding. For older tasks use
`./.benedict/library recall --query work/open` to find the ID.
Use `./.benedict/library search "terms"` and `./.benedict/library read "path"` for sources.
As evidence changes your judgment, append it to the same work ID with
`./.benedict/library work note ID "decision and reason" "source path or earlier work ID"`.
Before replying, save the exact nonsecret choices, the reason and any unresolved question. Read back the entry with `./.benedict/library work show ID` and check that someone can recover the choices without seeing your reply. Keep credentials out of the record.
Another session must be able to continue from `work show ID` alone.
