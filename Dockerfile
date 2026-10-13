# Relay Nova Live (live.agcunae.com). TLS fica no proxy do Coolify.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p nova-live-relay

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends curl && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --shell /usr/sbin/nologin novalive
COPY --from=build /src/target/release/nova-live-relay /usr/local/bin/nova-live-relay
USER novalive
EXPOSE 47910
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=5s CMD curl -fsS http://127.0.0.1:47910/healthz || exit 1
ENTRYPOINT ["/usr/local/bin/nova-live-relay", "--listen", "0.0.0.0:47910"]
