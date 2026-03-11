use axum::{
    middleware as axum_mw,
    routing::{any, get},
    Extension, Json, Router,
};
use metrics_exporter_prometheus::PrometheusHandle;
use serde_json::json;

use crate::{
    config::Config,
    metrics::metrics_middleware,
    middleware::{auth_middleware, rate_limit_middleware, RateLimiter},
    proxy,
};

pub fn build_router(config: Config, prom_handle: PrometheusHandle) -> Router {
    let limiter = RateLimiter::new(config.rate_limit_per_minute);

    let health = Router::new()
        .route("/health", get(health_check))
        .route("/metrics", get(prometheus_metrics));

    let protected = Router::new()
        .route("/api/v1/account/*path", any(proxy::forward_to_account))
        .route("/api/v1/app/*path", any(proxy::forward_to_app))
        .route("/api/v1/jiabu/*path", any(proxy::forward_to_jiabu))
        .route_layer(axum_mw::from_fn_with_state(config.clone(), auth_middleware))
        .route_layer(axum_mw::from_fn_with_state(limiter, rate_limit_middleware));

    Router::new()
        .merge(health)
        .merge(protected)
        .layer(axum_mw::from_fn(metrics_middleware))
        .layer(Extension(prom_handle))
        .with_state(config)
}

async fn health_check() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "service": "hamr-api-gateway",
        "version": "0.1.0"
    }))
}

async fn prometheus_metrics(
    Extension(handle): Extension<PrometheusHandle>,
) -> String {
    handle.render()
}
