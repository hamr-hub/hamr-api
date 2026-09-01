// `main.rs` is the binary crate root. To avoid compiling every module
// twice (once for the library, once for the binary), we pull the
// modules in from the library crate defined in `lib.rs`. This also
// makes the integration tests in `tests/` use the exact same code as
// the running binary.
use hamr_api_gateway::{config::Config, metrics, routes};

use std::net::SocketAddr;
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
    axum::serve(listener, app).await?;

    Ok(())
}
