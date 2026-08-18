# syntax=docker/dockerfile:1

FROM rust:1.89.0-bookworm AS builder
WORKDIR /src

ARG TARGETPLATFORM

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=xo-syncd-target-${TARGETPLATFORM},target=/src/target,sharing=locked \
    cargo build --locked --release -p xo-syncd && \
    cp target/release/xo-syncd /usr/local/bin/xo-syncd

FROM debian:bookworm-slim AS runtime-base

RUN apt-get update && \
    apt-get install --yes --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/* && \
    groupadd --gid 10001 xo && \
    useradd --uid 10001 --gid xo --home-dir /data --shell /usr/sbin/nologin xo && \
    install --directory --owner xo --group xo --mode 0700 /data

USER xo:xo
VOLUME ["/data"]
EXPOSE 9464
STOPSIGNAL SIGINT

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["curl", "--fail", "--silent", "--show-error", "http://127.0.0.1:9464/healthz"]

ENTRYPOINT ["/usr/local/bin/xo-syncd"]
CMD ["--state-dir", "/data", "--bind", "0.0.0.0:9464"]

# CI reuses the release binaries built natively by the platform matrix instead
# of compiling the ARM binary under QEMU.
FROM runtime-base AS runtime-prebuilt
ARG TARGETARCH
COPY --chown=xo:xo container-dist/${TARGETARCH}/xo-syncd /usr/local/bin/xo-syncd

# Keep local `docker build` self-contained and independent of CI artifacts.
FROM runtime-base AS runtime
COPY --from=builder /usr/local/bin/xo-syncd /usr/local/bin/xo-syncd
