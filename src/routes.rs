use axum::{
    middleware,
    routing::{any, get},
    Json, Router,
};
use serde_json::json;

use crate::{
    config::Config,
    middleware::{auth_middleware, rate_limit_middleware, RateLimiter},
    proxy,
};

pub fn build_router(config: Config) -> Router {
    let limiter = RateLimiter::new(config.rate_limit_per_minute);

    let health = Router::new().route("/health", get(health_check));

    let protected = Router::new()
        .route("/api/v1/account/*path", any(proxy::forward_to_account))
        .route("/api/v1/app/*path", any(proxy::forward_to_app))
        .route("/api/v1/jiabu/*path", any(proxy::forward_to_jiabu))
        .route_layer(middleware::from_fn_with_state(config.clone(), auth_middleware))
        .route_layer(middleware::from_fn_with_state(limiter, rate_limit_middleware));

    Router::new()
        .merge(health)
        .merge(protected)
        .with_state(config)
}

async fn health_check() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "service": "hamr-api-gateway",
        "version": "0.1.0"
    }))
}
