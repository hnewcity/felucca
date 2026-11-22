# syntax=docker/dockerfile:1
ARG RUST_IMAGE=rust:1.98.1-bookworm@sha256:9a73a5088750b4c95158ab26629c854c3d6fc4b173cb7bc8079ad252d8ed7bfa
FROM node:24-bookworm-slim AS admin-builder
WORKDIR /ui
COPY apps/admin/package.json apps/admin/package-lock.json ./
RUN npm ci
COPY apps/admin ./
RUN npm run build

FROM ${RUST_IMAGE} AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY vendor ./vendor
COPY apps/admin ./apps/admin
COPY --from=admin-builder /ui/dist ./apps/admin/dist
COPY scripts/cargo-external.sh ./scripts/cargo-external.sh

FROM builder AS server-builder
RUN --mount=type=cache,id=felucca-cargo,target=/src/target,sharing=locked \
    sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bins \
    && cp target/release/felucca-server /usr/local/bin/felucca-server \
    && felucca_info=$(target/release/felucca-agent --build-info) \
    && felucca_target=$(printf '%s' "$felucca_info" | sed -n 's/.*"target":"\([^"]*\)".*/\1/p') \
    && test -n "$felucca_target" \
    && mkdir -p "/usr/local/bin/agents/$felucca_target" \
    && cp target/release/felucca-agent "/usr/local/bin/agents/$felucca_target/felucca-agent" \
    && felucca_hash=$(sha256sum target/release/felucca-agent | cut -d ' ' -f 1) \
    && felucca_size=$(stat -c %s target/release/felucca-agent) \
    && printf '%s,"filename":"felucca-agent","bytes":%s,"sha256":"%s"}\n' \
       "${felucca_info%\}}" "$felucca_size" "$felucca_hash" \
       > "/usr/local/bin/agents/$felucca_target/manifest.json"

FROM builder AS agent-builder
RUN --mount=type=cache,id=felucca-cargo,target=/src/target,sharing=locked \
    sh scripts/cargo-external.sh build --release --locked -p felucca-cli --bin felucca-agent \
    && cp target/release/felucca-agent /usr/local/bin/felucca-agent

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libgcc-s1 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 felucca \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin felucca \
    && install -d -o 10001 -g 10001 -m 0750 /data /etc/felucca
WORKDIR /data
USER 10001:10001
STOPSIGNAL SIGTERM

FROM runtime AS server
USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl \
    && rm -rf /var/lib/apt/lists/*
USER 10001:10001
COPY --from=server-builder /usr/local/bin/felucca-server /usr/local/bin/felucca-server
COPY --from=server-builder /usr/local/bin/agents /usr/local/bin/agents
ENTRYPOINT ["/usr/local/bin/felucca-server"]
CMD ["--config", "/etc/felucca/server.toml"]

FROM runtime AS agent
COPY --from=agent-builder /usr/local/bin/felucca-agent /usr/local/bin/felucca-agent
ENTRYPOINT ["/usr/local/bin/felucca-agent"]
CMD ["--config", "/etc/felucca/agent.toml"]
