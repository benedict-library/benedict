# Python compatibility commands

Try the library with Python 3.10 or later and no Docker. The commands below create the bundled example in a new
folder, read the sample policy, file an answer and search for it.

```bash
python3 bin/library.py init --config /tmp/library-demo/library.json --demo
library() { python3 bin/library.py "$1" --config /tmp/library-demo/library.json "${@:2}"; }
library read --library main entities/refund.md
library save --library main "When can an unused product be returned?" \
  "The sample refund policy allows unused products to be returned within 14 days. For a used product, the cited page does not specify a return window." \
  --refs entities/refund.md
library search "refund"
```

The last search returns both the sample policy and the answer you just filed. Give a fresh session the registry path
and ask it to find the return window and its source. The handoff works when it can follow the filed answer back to
the original policy.

When a built Rust CLI is available (`BENEDICT_BIN`, `benedict` on PATH, or `core/target` in this checkout) and its
room service is running (`LIBRARY_ROOM_SOCKET`), search runs through the search room, so both clients rank results
and explain an empty result the same way. Without the room service these commands search their own index. The four
rooms, filing through the librarian and the courier need the Docker installation in the [README](../README.md#install).
