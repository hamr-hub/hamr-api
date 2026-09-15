use std::time::Duration;

use axum::{
    http::StatusCode,
    middleware as axum_mw,
    response::{IntoResponse, Response},
    routing::{any, get},
    Extension, Json, Router,
};
use metrics_exporter_prometheus::PrometheusHandle;
use serde_json::json;

use crate::{
    config::Config,
    health,
    metrics::metrics_middleware,
    middleware::{
        auth_middleware, rate_limit_middleware, RateLimiter, DEFAULT_CLEANUP_INTERVAL,
        DEFAULT_ENTRY_MAX_AGE,
    },
    proxy,
};

pub fn build_router(config: Config, prom_handle: PrometheusHandle) -> Router {
    let limiter = RateLimiter::new(config.rate_limit_per_minute);

    // Background sweep: remove entries that have not been seen for
    // > DEFAULT_ENTRY_MAX_AGE. Without this, an unbounded stream of
    // distinct keys (bot traffic, login storms) leaks memory. Round-2
    // iter-skill 2026-09-01 follow-up to round-1's allowlist fix.
    let _cleanup = limiter
        .clone()
        .spawn_cleanup_task(DEFAULT_CLEANUP_INTERVAL, DEFAULT_ENTRY_MAX_AGE);

    // readiness_check 以 `Extension<Config>` 读取下游地址；`.with_state(config)`
    // 只注入 State（供 auth/限流中间件用），不会顺带塞进 Extension。少了这层
    // Extension 时 axum 注入失败，/readyz 会退化成 500 而非契约要求的 503。
    let health = Router::new()
        .route("/health", get(health_check))
        .route("/readyz", get(readiness_check))
        .route("/metrics", get(prometheus_metrics))
        .layer(Extension(config.clone()));

    let public = Router::new()
        .route(
            "/api/v1/account/auth/register",
            any(proxy::forward_to_account),
        )
        .route("/api/v1/account/auth/login", any(proxy::forward_to_account))
        .route(
            "/api/v1/account/auth/refresh",
            any(proxy::forward_to_account),
        )
        .route_layer(axum_mw::from_fn_with_state(
            limiter.clone(),
            rate_limit_middleware,
        ));

    let protected = Router::new()
        .route("/api/v1/account/*path", any(proxy::forward_to_account))
        .route("/api/v1/app/*path", any(proxy::forward_to_app))
        .route("/api/v1/jiabu/*path", any(proxy::forward_to_jiabu))
        .route_layer(axum_mw::from_fn_with_state(config.clone(), auth_middleware))
        .route_layer(axum_mw::from_fn_with_state(limiter, rate_limit_middleware));

    Router::new()
        .merge(health)
        .merge(public)
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

/// Liveness vs readiness:
///   * `/health`  → liveness.  "The gateway process is up and serving."
///     Always 200; orchestrators should NOT restart the pod based on
///     this alone.
///   * `/readyz`  → readiness. "The gateway can actually forward
///     traffic to its downstreams." Returns 503 if any backend is
///     unreachable so orchestrators drain traffic from this pod.
///     Round-4 (iter-skill 2026-09-01) introduces this distinction;
///     before this round, the only probe was liveness.
async fn readiness_check(Extension(config): Extension<Config>) -> Response {
    // Probe each downstream with a 2s timeout. Build a fresh client
    // here (rather than reusing the proxy's `OnceLock`) so that:
    //   1. The proxy's 30s timeout doesn't bleed into a probe that
    //      should fail fast.
    //   2. Tests that build the router don't accidentally share a
    //      client with production traffic.
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "readyz: failed to build HTTP client");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "healthy": false,
                    "error": format!("client build failed: {e}"),
                })),
            )
                .into_response();
        }
    };

    let backends: &[(&'static str, &str)] = &[
        ("account", config.account_service_url.as_str()),
        ("app", config.app_service_url.as_str()),
        ("jiabu", config.jiabu_service_url.as_str()),
    ];

    let report = health::aggregate(&client, backends, Duration::from_secs(2)).await;

    if report.healthy {
        (StatusCode::OK, Json(report)).into_response()
    } else {
        let unhealthy: Vec<&str> = report
            .backends
            .iter()
            .filter(|b| !b.healthy)
            .map(|b| b.name)
            .collect();
        tracing::warn!(
            unhealthy = ?unhealthy,
            "readyz: one or more downstreams unreachable; reporting 503"
        );
        (StatusCode::SERVICE_UNAVAILABLE, Json(report)).into_response()
    }
}

async fn prometheus_metrics(Extension(handle): Extension<PrometheusHandle>) -> String {
    handle.render()
}
