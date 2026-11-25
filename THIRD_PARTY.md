# Third-party software

library's own code is MIT. Standalone source execution uses only Python 3.10+ and its standard library. No package
installation is required. The Docker distribution builds the Rust CLI and uses the separate `python:3.11-slim` image; its runtime and OS are
distributed under their respective licenses, not this repository's MIT license.

| Software | License | How library uses it |
|---|---|---|
| Python 3.10+ and its standard library | PSF-2.0 | Runs standalone CLI, existing clients and tests. |
| PostgreSQL server and `psql` | PostgreSQL License | Optional durable PostgreSQL records, registry and deployment integration. |
| Rust 1.93.1 toolchain (`rust:1.93.1-slim-trixie`, build stage) | MIT OR Apache-2.0 | Compiles the CLI in `core/` and the optional tokenizer in `native/`. Dependency versions are pinned in their Cargo lockfiles. |
| psycopg2-binary 2.9.10 (vector image only) | LGPL-3.0-or-later with exceptions; bundles libpq (PostgreSQL License) | Installed by `docker build --target vector`; connects the room to the vector table. |
| regex 1.13.1 (Rust crate, courier only) | MIT OR Apache-2.0 | Matches the optional deny list in `courier/`. |
| serde_json 1.0.151 (Rust crate, courier only) | MIT OR Apache-2.0 | Writes the courier ledger lines. |
| axum 0.8.9 and tokio 1.54.1 | MIT | Plaza HTTP server, async runtime and network transport. |
| reqwest 0.13.5, serde 1.0.229 and serde_json 1.0.154 | MIT OR Apache-2.0 | Plaza HTTPS client, JSON API and event storage. |
| Git | GPL-2.0 | Used for skill provenance and wiki backups; installed from Debian in the Docker image. |
| llama.cpp server image (`ghcr.io/ggml-org/llama.cpp:server-b12464`, pinned by digest) | MIT | Optional `embedder` service of the `semantic` compose profile; serves `/v1/embeddings`. Not part of the library image. |
| EmbeddingGemma 2 GGUF (`ggml-org/embeddinggemma-2-GGUF`, BF16, pinned revision and SHA-256) | Apache-2.0 (model card) | Embedding model the installer downloads into the `library_models` volume when semantic search is on. Not redistributed by this repository. |

Plaza pins its dependency tree in [plaza/Cargo.lock](plaza/Cargo.lock). Its container uses the Rust toolchain and Alpine Linux, whose packaged components carry their respective licenses.

The Rust CLI links bundled SQLite through rusqlite. Its direct dependencies and pinned dependency tree are in [core/Cargo.toml](core/Cargo.toml) and [core/Cargo.lock](core/Cargo.lock).

The lexical room search module `runtime/wiki_room_fuse.py` and the embedding adapter `runtime/wiki_embed_client.py`
are part of this repository. Optional vector search also needs `psycopg2` (vector image), an embedding server or
provider, and a compatible vector schema; their licenses and terms are the operator's to check. These modules and deployment
services are not included or installed by the standalone package. Operators must verify the licenses and pinned
versions of their own room deployment before redistribution. See [portability boundaries](docs/portability.md).
