# syntax=docker/dockerfile:1.7
FROM rust:1.100-bookworm AS tools
WORKDIR /src
RUN rustup component add rustfmt clippy
ENV CARGO_TARGET_DIR=/build

FROM tools AS build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY apps ./apps
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build \
    cargo build --locked --release --workspace && \
    install -D /build/release/datavault-server /out/datavault-server && \
    install -D /build/release/datavault /out/datavault

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/* && useradd --uid 10001 --create-home datavault && \
    install -d -m 0700 -o datavault -g datavault /var/lib/datavault
COPY --from=build /out/ /usr/local/bin/
USER datavault
EXPOSE 9000 9001
ENTRYPOINT ["datavault-server"]

FROM runtime AS cli
USER root
RUN apt-get update && apt-get install -y --no-install-recommends awscli && rm -rf /var/lib/apt/lists/*
USER datavault
ENTRYPOINT ["datavault"]

FROM tools AS audit
RUN cargo install --locked cargo-audit --version 0.22.1
ENTRYPOINT ["cargo", "audit"]
