# plaza: tested and built in a pinned Rust image, run as one static binary in a pinned Alpine.
FROM rust:1.99.1-alpine3.24@sha256:27cd9befd4a05d6f6eb407c3e28365f5092f008bfc5946207cca76ae5179ef23 AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tests ./tests
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target,sharing=locked \
    cargo clean --release -p plaza && cargo test --release --locked && cargo build --release --locked && cp target/release/plaza /plaza

FROM alpine:3.24.3@sha256:be2656061b536c1883c4034790e21d0a09c5aaaf51968b89368a3c0371ea2c9a
COPY --from=build /plaza /usr/local/bin/plaza
ENV PLAZA_DATA=/data PLAZA_ACCOUNTS=/run/plaza/accounts.json
RUN mkdir -p /data && chown 1001:1001 /data && chmod 700 /data
USER 1001:1001
VOLUME /data
EXPOSE 43190
ENTRYPOINT ["/usr/local/bin/plaza"]
CMD ["serve"]
