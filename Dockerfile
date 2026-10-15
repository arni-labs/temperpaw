FROM node:22-bookworm AS dashboard-build
WORKDIR /app/dashboard
COPY dashboard/package*.json ./
RUN npm install
COPY dashboard ./
RUN npm run build

FROM rust:1.94-bookworm AS rust-build
RUN apt-get update && apt-get install -y libclang-dev cmake libz3-dev && rm -rf /var/lib/apt/lists/*
ARG BUILD_VERSION=dev
ARG BUILD_SHA=unknown
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY os-apps ./os-apps
COPY scripts ./scripts
COPY docs ./docs
COPY railway.toml README.md AGENTS.md CLAUDE.md INSTRUCTIONS.md ./
COPY --from=dashboard-build /app/dashboard/build ./dashboard/build
ENV CARGO_BUILD_JOBS=2
ENV BUILD_VERSION=${BUILD_VERSION}
ENV BUILD_SHA=${BUILD_SHA}
RUN cargo build -p temperpaw --release --bin temperpaw-server
# Build WASM modules for os-apps (requires wasm32 targets)
RUN rustup target add wasm32-unknown-unknown wasm32-wasip1
RUN cd os-apps/paw-agent/wasm && bash build.sh \
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
    && cd /app/os-apps/dsf-twin/wasm && bash build.sh
RUN bash scripts/verify_route_message_wasm.sh /app/os-apps/paw-channels/wasm/route_message/route_message.wasm
RUN find os-apps -type d -name target -prune -exec rm -rf {} +

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
COPY --from=rust-build /app/target/release/temperpaw-server ./temperpaw
COPY --from=rust-build /app/dashboard/build ./dashboard/build
COPY --from=rust-build /app/os-apps ./os-apps
COPY scripts/temperpaw-entrypoint.sh ./scripts/temperpaw-entrypoint.sh
COPY scripts/datadog_railway_capability_check.sh ./scripts/datadog_railway_capability_check.sh
RUN chmod +x ./scripts/temperpaw-entrypoint.sh
RUN chmod +x ./scripts/datadog_railway_capability_check.sh
ENV BUILD_VERSION=${BUILD_VERSION}
ENV BUILD_SHA=${BUILD_SHA}
EXPOSE 3467
ENTRYPOINT ["./scripts/temperpaw-entrypoint.sh"]
