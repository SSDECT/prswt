# PRSWT server image (OPS-01). Build context: repository root.
# Serves sign-in, the token page, the HTTP API and remote MCP; no web UI (ADR-0009).

FROM rust:1.99.1-slim-trixie AS build
WORKDIR /src
COPY engine/ ./
RUN cargo build --release --locked -p prswt-server

FROM debian:trixie-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --no-create-home prswt
COPY --from=build /src/target/release/prswt-server /usr/local/bin/prswt-server
USER 10001
ENV PRSWT_BIND=0.0.0.0:8080
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --retries=3 CMD ["/usr/local/bin/prswt-server", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/prswt-server"]
