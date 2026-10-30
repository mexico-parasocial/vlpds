# syntax=docker/dockerfile:1.7
FROM rust:1.99.0-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends git cmake clang && rm -rf /var/lib/apt/lists/*
COPY source.env prepare.sh /overlay/
COPY patches/ /overlay/patches/
RUN bash /overlay/prepare.sh /src
WORKDIR /src
ENV RUSTUP_TOOLCHAIN=1.99.0
ENV CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo test --locked -p rsky-relay --no-default-features && \
    cargo build --locked --release -p rsky-relay --bin rsky-relay && \
    cp target/release/rsky-relay /rsky-relay

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl dumb-init && \
    rm -rf /var/lib/apt/lists/* && \
    groupadd --gid 10001 relay && useradd --uid 10001 --gid relay --no-create-home relay && \
    mkdir -p /data/relay && chown relay:relay /data/relay
COPY --from=build /rsky-relay /usr/local/bin/rsky-relay
COPY source.env /usr/share/rsky-relay/source.env
COPY --from=build /src/LICENSE /usr/share/doc/rsky-relay/LICENSE
LABEL org.opencontainers.image.source="https://github.com/blacksky-algorithms/rsky" \
      org.opencontainers.image.revision="e96634e18d3731d4f4a0380f620f2e1ea52f9a9f" \
      para.rsky.patch="para2"
WORKDIR /data/relay
USER 10001:10001
ENV RELAY_LISTEN=0.0.0.0:2470 RELAY_LENIENT_VALIDATION=false RUST_LOG=rsky_relay=info
EXPOSE 2470
HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -fsS http://127.0.0.1:2470/xrpc/_health || exit 1
ENTRYPOINT ["dumb-init", "--"]
CMD ["/usr/local/bin/rsky-relay", "--no-plc-export"]
