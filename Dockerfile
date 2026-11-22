# Rust CLI with SQLite and PostgreSQL support. Compatibility adapters use the bundled Python runtime.
# Optional Rust tokenizer (native/, standard library only): the same words as the Python one.
FROM rust:1.93.1-slim-trixie@sha256:c0a38f5662afdb298898da1d70b909af4bda4e0acff2dc52aea6360a9b9c6956 AS native
WORKDIR /src
COPY native /src
RUN cargo build --release --locked --offline

FROM rust:1.93.1-slim-trixie@sha256:c0a38f5662afdb298898da1d70b909af4bda4e0acff2dc52aea6360a9b9c6956 AS core
WORKDIR /src/core
COPY core /src/core
COPY docs /src/docs
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/work-target,sharing=locked \
    touch src/main.rs && cargo build --locked --release --target-dir /work-target && cp /work-target/release/benedict /usr/local/bin/

FROM python:3.11-slim-trixie@sha256:07d4824036af339fa1b6ef769cc99d9c10a141769538b52b3b597c84f7a7b535 AS lexical
ENV BENEDICT_CLI=/usr/local/bin/benedict PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 LIBRARY_CONFIG=/data/library.json LIBRARY_DB=/data/library.sqlite3 PYTHONPATH=/app/runtime
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends postgresql-client git acl && rm -r /var/lib/apt/lists/*
COPY --from=core /usr/local/bin/benedict /usr/local/bin/benedict
COPY bin /app/bin
COPY library_local.py skill_publish.py AGENTS.md /app/
COPY docs /app/docs
COPY runtime /app/runtime
COPY rooms /app/rooms
COPY examples /app/examples
RUN mkdir /data && chown 10001:10001 /data
USER 10001:10001
ENTRYPOINT ["benedict"]
CMD ["--help"]

# Vector image: adds the PostgreSQL client module for hybrid (pgvector) search. Building it downloads that package.
FROM lexical AS vector
USER root
RUN pip install --no-cache-dir psycopg2-binary==2.9.10
COPY --from=native /src/target/release/liblibrary_native.so /app/runtime/
ENV LIBRARY_NATIVE=on
USER 10001:10001

FROM lexical
