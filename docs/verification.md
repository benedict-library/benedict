# Verification

## Current Rust distribution

Run `cargo test --locked --manifest-path core/Cargo.toml` on Linux with Git and the ACL utilities installed. Run `bash tests/docker-handoff.sh` on a Docker Compose host with jq. It builds a fresh image, activates an isolated project, reads a shelf original, files a decision, checks the room memory and retrieves the evidence after restarting the room service. Run `cargo test --locked --manifest-path verification/compat/Cargo.toml` for the compatibility suite. It uses synthetic records and calls the shipped Python adapters. Set `BENEDICT_TEST_PYTHON` to select an interpreter.

### Local release checks, 2026-11-21

| Check | Observed result on Linux |
|---|---|
| Rust library | 23 tests passed, including selection of the shelf belonging to the active library, missing references, and promotion of filed knowledge |
| Host courier | 19 tests passed. In a fresh Compose project a meeting request received the host result; a new client retrieved that reply using the original task ID. See [courier handoff](courier.md). |
| Plaza | 14 Rust tests passed. A non-root process wrote a question to a fresh volume, restarted the server and retrieved the same question. Run `sh plaza/tests/fresh-volume.sh IMAGE` against the built image. |
| Python compatibility | 234 tests passed with the native search library built. |
| Fresh Docker library | Separate CLI containers opened a shelf original, recorded a question with its source, filed and found the page, and retrieved the record after restarting the room service. |

These results cover the Linux installation. The hosted workflow reports its platform-specific results separately.

## Historical Python installation

Verification on 2026-11-02 (KST) of the one-library-one-container layout: shelf originals read-only, the wiki
(encyclopedia, skills, filing cabinet), indexes and room snapshots writable, network for the search room's web step.

| Check | Result |
|---|---|
| Python 3.12.3, `python3 -m unittest discover -s tests` | 162 tests, OK with 1 skipped, 41 seconds |
| Compose, fresh project and named volume | `init --demo` → `save` wrote a filing-cabinet page; a write to `/shelf` failed with "Read-only file system"; an HTTPS request from the container returned 200; `down -v` removed the volume |
| Operator deployment, one library in one container with four rooms and an optional vector table | network bridge, root writable, no memory limit; a write to the mounted shelf failed read-only, a write to the wiki succeeded; `room_sync.py` started the four rooms; one `library.py save` filed a page, refreshed the reading/search rooms (1 body, 1 index entry) and wrote one vector row |

The earlier verification (2026-10-30) ran containers with no network, a read-only root and dropped capabilities.
That layout kept the search room from the web and the library from writing its own wiki, so it was replaced; its
record remains in the Git history of this file.

## Historical commands

```bash
python3 -m unittest discover -s tests
docker compose -p library-verify build
docker compose -p library-verify run --rm library init --config /data/library.json --demo
docker compose -p library-verify run --rm library save --config /data/library.json --library main \
  "Compose write test" "The data volume is writable for the wiki and its filing cabinet; the shelf is read-only."
docker compose -p library-verify run --rm --entrypoint sh library -c 'touch /shelf/x'      # must fail
docker compose -p library-verify down -v
```

## Failures retained in the result

- In the operator deployment the first filed page was named by the UTC day (`261101_…`) at 02:00 local time on
  2026-11-02. `bin/library.py save` now uses the local day; the test runs under a +9 hour zone.
- After `bin/save_query.py` began refusing a missing `wiki_root`, 18 web tests errored: their fixture had relied on
  the writer creating the wiki folder. The fixture now creates it; the refusal stays.
- `init` could only open the example `main` library; the operator deployment wrote its registry by hand. `init --key
  --name` was added and is tested.

## Limits

This verifies synthetic documents and one operator library, not semantic retrieval quality or multi-host rollout.
No PDF/media ingestion or hosted authentication is provided. The librarian runs bounded, operator-started cycles.
Skill publishing and wiki backup need Git, which the current Docker image includes; backup is local only. Page replacement is
atomic, but page/log writes are not a transaction and concurrent writers require external coordination. See
[portability boundaries](portability.md).
