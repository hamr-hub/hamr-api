mod config;
mod errors;
mod metrics;
mod middleware;
mod proxy;
mod routes;

use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};

pub use config::Config;

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

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = routes::build_router(config.clone(), prom_handle).layer(cors);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!("HamR API Gateway listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
