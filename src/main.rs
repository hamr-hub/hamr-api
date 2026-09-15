// `main.rs` is the binary crate root. To avoid compiling every module
// twice (once for the library, once for the binary), we pull the
// modules in from the library crate defined in `lib.rs`. This also
// makes the integration tests in `tests/` use the exact same code as
// the running binary.
//
// Round-5 (iter-skill 2026-09-01) — added graceful shutdown on
// SIGTERM/SIGINT via `axum::serve(...).with_graceful_shutdown(...)`.
// Without this, k8s rolling updates drop in-flight requests when the
// pod is replaced. Kubernetes sends SIGTERM, waits
// `terminationGracePeriodSeconds` (default 30s), then SIGKILLs. The
// gateway now stops accepting new connections on signal and lets
// in-flight handlers complete; combine with `terminationGracePeriodSeconds`
// in the Pod spec to size the shutdown budget.
use hamr_api_gateway::{config::Config, metrics, routes};

use std::net::SocketAddr;
use tokio::signal;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hamr_api_gateway=info,tower_http=info".into()),
        )
        .json()
        .init();

    dotenvy::dotenv().ok();

    let prom_handle = metrics::install_recorder();

    let config = Config::from_env()?;

    // CORS allowlist: env-driven. Defaults to local dev origins + hamr.top prod.
    // Replaces previous `CorsLayer::new().allow_origin(Any)` (P0 from
    // iter-skill 2026-07-01 round-2): any origin could hit the API; even
    // without `allow_credentials`, browsers will still let JS from any site
    // call public auth endpoints.
    let allow_origin = if config.cors_allowed_origins.is_empty() {
        // Safety net: never silently re-open to wildcard. Empty config is
        // almost always a misconfiguration; refuse rather than degrade.
        tracing::warn!("CORS_ALLOWED_ORIGINS is empty; defaulting to deny-all origin list");
        AllowOrigin::list([])
    } else {
        let parsed: Vec<_> = config
            .cors_allowed_origins
            .iter()
            .filter_map(|o: &String| o.parse().ok())
            .collect();
        AllowOrigin::list(parsed)
    };

    let cors = CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = routes::build_router(config.clone(), prom_handle).layer(cors);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!(
        "HamR API Gateway listening on {} (cors origins: {:?})",
        addr,
        config.cors_allowed_origins
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("HamR API Gateway shutdown complete");
    Ok(())
}

/// Wait for SIGTERM (orchestrator-driven) or SIGINT (Ctrl-C, dev).
///
/// `axum::serve(...).with_graceful_shutdown(future)` stops accepting
/// new connections as soon as `future` resolves, then waits for
/// in-flight requests to finish. We resolve on either of:
///   * `tokio::signal::ctrl_c()` — Ctrl-C in dev / `docker stop`
///     without `--signal` set,
///   * `tokio::signal::unix::SignalKind::terminate()` — SIGTERM, which
///     is what Kubernetes sends during a rolling update.
///
/// On non-unix targets only Ctrl-C is wired up; `signal::unix` is
/// gated by `#[cfg(unix)]` so this still compiles for Windows dev
/// boxes.
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("Ctrl-C received; draining in-flight requests"),
        _ = terminate => tracing::info!("SIGTERM received; draining in-flight requests"),
    }
}
