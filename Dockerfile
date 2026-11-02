FROM node:22-bookworm AS dashboard-build
WORKDIR /app/dashboard
COPY dashboard/package*.json ./
RUN npm install
COPY dashboard ./
RUN npm run build

FROM rust:1.94-bookworm AS rust-base
RUN apt-get update && apt-get install -y libclang-dev cmake libz3-dev && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown wasm32-wasip1
WORKDIR /app

# The server and the os-app modules build in stages of their own, so a change to one reuses the other's layers. Each
# keeps its compiled dependencies in a cache mount (docker.yml saves the mounts between runs), so a change recompiles
# its own crates and the dependencies it moved, not everything. Sources get a new mtime each checkout, so cargo always
# rebuilds the crates it is given, and never serves a stale one.
FROM rust-base AS server-build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# The one os-app the server links crates of (crates/temperpaw/Cargo.toml's dsf-resource-collect and its siblings).
# The server depends on crates in os-apps by path; they come along whichever those are.
COPY os-apps ./os-apps
# Each stage has its own registry and git mounts: the stages run at once, and cargo's lock lives in
# CARGO_HOME outside the mounts, so sharing them lets two cargos write one checkout.
RUN --mount=type=cache,id=server-target,target=/cache/target \
    --mount=type=cache,id=server-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=server-git,target=/usr/local/cargo/git \
    cargo build -p temperpaw --release --bin temperpaw-server --target-dir /cache/target \
    && cp /cache/target/release/temperpaw-server /app/temperpaw-server

FROM rust-base AS wasm-build
COPY os-apps ./os-apps
COPY scripts/verify_route_message_wasm.sh ./scripts/verify_route_message_wasm.sh
RUN --mount=type=cache,id=wasm-target,target=/cache/target \
    --mount=type=cache,id=wasm-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=wasm-git,target=/usr/local/cargo/git \
    export CARGO_TARGET_DIR=/cache/target \
    && cd /app/os-apps/paw-agent/wasm && bash build.sh \
    && cd /app/os-apps/paw-channels/wasm && bash build.sh \
    && cd /app/os-apps/paw-fs/wasm/artifact_batch_apply && bash build.sh \
    && cd /app/os-apps/paw-fs/wasm/blob_adapter && bash build.sh \
    && cd /app/os-apps/paw-fs/wasm/workspace_fs && bash build.sh \
    && cd /app/os-apps/paw-ingest/wasm && bash build.sh \
    && cd /app/os-apps/paw-managed-agents/wasm && bash build.sh \
    && cd /app/os-apps/paw-media/wasm && bash build.sh \
    && cd /app/os-apps/paw-foresight/wasm && bash build.sh \
    && cd /app/os-apps/paw-skills/wasm && bash build.sh \
    && cd /app/os-apps/paw-research/wasm && bash build.sh \
    && cd /app/os-apps/paw-compute/wasm && bash build.sh \
    && cd /app/os-apps/paw-patrol/wasm && bash build.sh \
    && cd /app/os-apps/dsf-twin/wasm && bash build.sh \
    && du -sm /cache/target /usr/local/cargo/registry /usr/local/cargo/git
RUN bash scripts/verify_route_message_wasm.sh /app/os-apps/paw-channels/wasm/route_message/route_message.wasm

FROM debian:bookworm-slim
ARG TARGETARCH
ARG DDPROF_VERSION=0.26.0
RUN apt-get update \
    && apt-get install -y ca-certificates curl libz3-4 git xz-utils \
    && rm -rf /var/lib/apt/lists/* \
    && case "${TARGETARCH:-amd64}" in \
        amd64|arm64) ddprof_arch="${TARGETARCH:-amd64}" ;; \
        *) echo "unsupported ddprof architecture: ${TARGETARCH}" >&2; exit 1 ;; \
    esac \
    && curl -fsSL "https://github.com/DataDog/ddprof/releases/download/v${DDPROF_VERSION}/ddprof-${DDPROF_VERSION}-${ddprof_arch}-linux.tar.xz" -o /tmp/ddprof.tar.xz \
    && tar -xJf /tmp/ddprof.tar.xz -C /usr/local/bin --strip-components=2 ddprof/bin/ddprof \
    && chmod +x /usr/local/bin/ddprof \
    && rm -f /tmp/ddprof.tar.xz
ARG BUILD_VERSION=dev
ARG BUILD_SHA=unknown
WORKDIR /app
COPY --from=server-build /app/temperpaw-server ./temperpaw
COPY --from=dashboard-build /app/dashboard/build ./dashboard/build
COPY --from=wasm-build /app/os-apps ./os-apps
COPY scripts/temperpaw-entrypoint.sh ./scripts/temperpaw-entrypoint.sh
COPY scripts/datadog_railway_capability_check.sh ./scripts/datadog_railway_capability_check.sh
RUN chmod +x ./scripts/temperpaw-entrypoint.sh
RUN chmod +x ./scripts/datadog_railway_capability_check.sh
ENV BUILD_VERSION=${BUILD_VERSION}
ENV BUILD_SHA=${BUILD_SHA}
EXPOSE 3467
ENTRYPOINT ["./scripts/temperpaw-entrypoint.sh"]
