FROM rust:1.83-slim AS builder
WORKDIR /app
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release && rm -rf src
COPY . .
RUN touch src/main.rs && cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libssl3 ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/hamr-api-gateway .

# Graceful shutdown (iter-skill 2026-09-01 round-5): the binary
# handles SIGTERM via axum::serve(...).with_graceful_shutdown. When
# the orchestrator (k8s, docker-compose) sends SIGTERM, the gateway
# stops accepting new connections and drains in-flight requests.
#
# Kubernetes: set `terminationGracePeriodSeconds: 30` (default is
# already 30) in the Pod spec so k8s does not SIGKILL before the
# drain finishes. The 30s budget is comfortably above any plausible
# in-flight request budget for this proxy (reqwest timeout is 30s).
#
# docker-compose: `stop_grace_period: 30s` in docker-compose.yml
# matches the same window.
#
# If you need to force-quit (SIGKILL) immediately, the operator
# should send a second signal after the grace period; the gateway
# has no second-stage cleanup because all state lives in
# process-memory (RateLimiter DashMap + Prometheus recorder) and is
# discarded on exit.
EXPOSE 8090
CMD ["./hamr-api-gateway"]
