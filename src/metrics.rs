//! Prometheus metrics + per-request middleware.
//!
//! Round-5 (iter-skill 2026-09-01) — upgraded the metrics middleware to
//! emit **per-route** labels so Grafana can slice 4xx/5xx by backend
//! (account vs app vs jiabu) and by endpoint class (auth vs protected)
//! without grepping raw paths. The previous implementation normalised
//! the `path` label into a small set of wildcard templates, which
//! collapsed every account route into one bucket and made it impossible
//! to tell "auth login has more 401s than account protected reads".
//!
//! Label design (cardinality-bounded):
//!   * `route`   — `axum::extract::MatchedPath` value (e.g.
//!                 `/api/v1/account/*path`). Bounded by the route table
//!                 so adding new routes is the only way to grow this
//!                 dimension. Falls back to `unmatched` for 404s so a
//!                 scanner hitting random URLs cannot blow up the
//!                 series count.
//!   * `service` — coarse bucket for Grafana slicing. Fixed enum:
//!                 `account-auth`, `account`, `app`, `jiabu`, `admin`
//!                 (health/readyz/metrics), `other`.
//!   * `method`  — GET/POST/etc. Bounded by HTTP.
//!   * `status`  — 200/401/502/... Bounded by HTTP.
//!
//! The raw URI path is preserved in `tracing` logs (where it does not
//! affect Prometheus cardinality) so operators can still grep for
//! scanner traffic.

use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::Response,
};
use metrics::{counter, histogram};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::time::Instant;

pub fn install_recorder() -> PrometheusHandle {
    PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Full("http_request_duration_seconds".to_string()),
            &[0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5],
        )
        .expect("failed to set histogram buckets")
        .install_recorder()
        .expect("Failed to install Prometheus recorder")
}

pub async fn metrics_middleware(request: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = request.method().to_string();

    // Prefer the matched route template (bounded by the router). For
    // 404s there's no matched route — fall back to `unmatched` so a
    // scanner hitting /wp-admin.php doesn't add a new label value per
    // request. Raw path goes into tracing logs where cardinality is
    // free.
    let matched = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_string());
    let raw_path = request.uri().path().to_string();

    let client_ip = request
        .headers()
        .get("x-forwarded-for")
        .or_else(|| request.headers().get("x-real-ip"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .split(',')
        .next()
        .unwrap_or("unknown")
        .trim()
        .to_string();

    let response = next.run(request).await;

    let duration = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    let route_label = matched.clone().unwrap_or_else(|| "unmatched".to_string());
    let service = classify_service(matched.as_deref().unwrap_or(raw_path.as_str()));

    counter!("http_requests_total",
        "method" => method.clone(),
        "route" => route_label.clone(),
        "service" => service,
        "status" => status.clone()
    )
    .increment(1);

    histogram!("http_request_duration_seconds",
        "method" => method.clone(),
        "route" => route_label.clone(),
        "service" => service
    )
    .record(duration);

    if response.status().is_server_error() {
        counter!("http_errors_total",
            "method" => method.clone(),
            "route" => route_label.clone(),
            "service" => service,
            "status" => status.clone()
        )
        .increment(1);
    }

    let duration_ms = (duration * 1000.0) as u64;

    tracing::info!(
        method = %method,
        path = %raw_path,
        route = %route_label,
        service = %service,
        status = %status,
        duration_ms = duration_ms,
        client_ip = %client_ip,
        "request"
    );

    response
}

/// Bucket the path into a small, fixed set of `service` labels so the
/// Prometheus time series count stays bounded.
///
/// This function is the single source of truth for Grafana slicing.
/// When new route groups are added to `routes.rs`, also add a branch
/// here; otherwise the new traffic falls into `other` and operators
/// have to drill into raw logs to see it.
///
/// Order matters: `/api/v1/account/auth/...` MUST be checked before
/// `/api/v1/account/...` because both prefixes match account traffic.
fn classify_service(path: &str) -> &'static str {
    if path.starts_with("/api/v1/account/auth/") {
        "account-auth"
    } else if path.starts_with("/api/v1/account/") {
        "account"
    } else if path.starts_with("/api/v1/app/") {
        "app"
    } else if path.starts_with("/api/v1/jiabu/") {
        "jiabu"
    } else if matches!(path, "/health" | "/readyz" | "/metrics") {
        "admin"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of `classify_service` is to bound Prometheus
    /// label cardinality. Any path we don't recognise MUST fall into
    /// `other`; the test exhausts a few thousand random scanner-style
    /// paths and asserts they all collapse to one bucket.
    #[test]
    fn classify_service_buckets_account_auth_separately() {
        assert_eq!(classify_service("/api/v1/account/auth/login"), "account-auth");
        assert_eq!(classify_service("/api/v1/account/auth/register"), "account-auth");
        assert_eq!(classify_service("/api/v1/account/auth/refresh"), "account-auth");
    }

    #[test]
    fn classify_service_buckets_account_protected_routes() {
        // Catch-all wildcard path the router uses for protected account
        // endpoints: anything not under /auth/ after the prefix.
        assert_eq!(classify_service("/api/v1/account/*path"), "account");
        assert_eq!(classify_service("/api/v1/account/users"), "account");
        assert_eq!(classify_service("/api/v1/account/users/123"), "account");
        assert_eq!(classify_service("/api/v1/account/settings/profile"), "account");
    }

    #[test]
    fn classify_service_buckets_app_and_jiabu() {
        assert_eq!(classify_service("/api/v1/app/projects"), "app");
        assert_eq!(classify_service("/api/v1/app/*path"), "app");
        assert_eq!(classify_service("/api/v1/jiabu/tasks"), "jiabu");
        assert_eq!(classify_service("/api/v1/jiabu/*path"), "jiabu");
    }

    #[test]
    fn classify_service_buckets_admin_routes() {
        assert_eq!(classify_service("/health"), "admin");
        assert_eq!(classify_service("/readyz"), "admin");
        assert_eq!(classify_service("/metrics"), "admin");
    }

    #[test]
    fn classify_service_unknown_paths_fall_to_other() {
        assert_eq!(classify_service("/"), "other");
        assert_eq!(classify_service("/typo"), "other");
        assert_eq!(classify_service("/wp-login.php"), "other");
    }

    #[test]
    fn classify_service_does_not_explode_cardinality_for_random_paths() {
        // 1000 distinct scanner-style paths must all collapse to "other".
        // If we accidentally added a per-path bucket, this test would
        // produce 1000 distinct return values and the equality check
        // would still pass — but a real operator would see 1000 series.
        // The test pins the contract: ANY unknown path -> "other".
        for i in 0..1000 {
            let p = format!("/scanner-path-{i}");
            assert_eq!(classify_service(&p), "other", "path {p} should bucket to other");
        }
    }

    #[test]
    fn classify_service_prefers_account_auth_over_account() {
        // Order-of-checks regression guard: /api/v1/account/auth/foo
        // starts with both prefixes. Must bucket to "account-auth",
        // not "account". A future refactor that reorders branches
        // breaks this test loudly instead of silently mixing auth
        // 401s with account 200s in the dashboard.
        assert_eq!(classify_service("/api/v1/account/auth/login"), "account-auth");
    }
}
