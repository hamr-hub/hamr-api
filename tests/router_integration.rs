//! Router-level integration tests using `tower::ServiceExt::oneshot`.
//!
//! Round-5 (iter-skill 2026-09-01) — the previous `proxy_integration.rs`
//! exercised `proxy::proxy_request` in isolation. That proved the
//! proxy hop works, but left the router itself untested: middleware
//! ordering, route matching, JWT auth, rate-limit boundary, and the
//! `/health` / `/readyz` / `/metrics` admin routes were only verified
//! by manual smoke-testing the running container.
//!
//! These tests build the real `routes::build_router` and drive it via
//! `tower::ServiceExt::oneshot`, which is the minimum-dependency way
//! to exercise a full axum middleware chain. They cover:
//!   * `/health` returns 200 with the expected JSON body,
//!   * `/metrics` returns 200 with Prometheus text format,
//!   * protected routes return 401 without a JWT,
//!   * protected routes return 200 with a real `jsonwebtoken`-signed JWT
//!     (proves the round-2 HS256 path is wired up end-to-end),
//!   * rate limit returns 429 after `RATE_LIMIT_PER_MINUTE + 1` requests
//!     against a public auth endpoint.
//!
//! Prometheus recorder caveat: `install_recorder` calls
//! `set_global_recorder`, which can only succeed once per process.
//! We cache a single handle in a `OnceLock` so the integration tests
//! don't panic on the second `install_recorder` invocation.

use std::sync::OnceLock;

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use chrono::Utc;
use hamr_api_gateway::{config::Config, routes};
use httpmock::prelude::*;
use jsonwebtoken::{encode, EncodingKey, Header};
use metrics_exporter_prometheus::PrometheusHandle;
use tower::util::ServiceExt;

/// 32-byte minimum to satisfy the round-2 fail-fast check in
/// `Config::from_env`. Mirrors the value CI uses.
const TEST_JWT_SECRET: &str = "router-test-secret-with-at-least-32-bytes-of-entropy";
const UNREACHABLE: &str = "http://127.0.0.1:1";

/// Process-wide Prometheus handle. `install_recorder` is global and
/// can only be installed once; without this cache the second test
/// panics on "Failed to install Prometheus recorder".
fn shared_prom_handle() -> PrometheusHandle {
    static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
    HANDLE
        .get_or_init(hamr_api_gateway::metrics::install_recorder)
        .clone()
}

/// Build a Config with sensible test defaults:
///   * JWT secret: 32+ bytes (passes round-2 fail-fast),
///   * downstreams: `http://127.0.0.1:1` (unreachable, so any
///     accidental proxy call returns 502 — useful for distinguishing
///     "auth/rate-limit did its job" from "downstream succeeded"),
///   * rate limit: 3/minute, low enough to hit cheaply in tests.
fn test_config(account_url: &str, app_url: &str, jiabu_url: &str) -> Config {
    Config {
        port: 0,
        jwt_secret: TEST_JWT_SECRET.to_string(),
        account_service_url: account_url.to_string(),
        app_service_url: app_url.to_string(),
        jiabu_service_url: jiabu_url.to_string(),
        rate_limit_per_minute: 3,
        cors_allowed_origins: vec!["http://localhost:3000".to_string()],
    }
}

/// Sign a test JWT with the claims issued by hamr-account.
fn sign_token() -> String {
    let now = Utc::now().timestamp();
    let claims = serde_json::json!({
        "sub": "00000000-0000-0000-0000-000000000001",
        "email": "user@test.local",
        "username": "user",
        "exp": now + 3600,
        "iat": now,
    });
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(TEST_JWT_SECRET.as_bytes()),
    )
    .expect("token must sign with a 32-byte HS256 key")
}

#[tokio::test]
async fn router_health_returns_200_with_expected_body() {
    let app = routes::build_router(
        test_config(UNREACHABLE, UNREACHABLE, UNREACHABLE),
        shared_prom_handle(),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("router must serve /health");

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = to_bytes(response.into_body(), 1024).await.unwrap();
    let body = std::str::from_utf8(&body_bytes).unwrap();
    assert!(body.contains("\"status\":\"ok\""), "body: {body}");
    assert!(body.contains("hamr-api-gateway"), "body: {body}");
}

#[tokio::test]
async fn router_metrics_endpoint_returns_prometheus_text() {
    let app = routes::build_router(
        test_config(UNREACHABLE, UNREACHABLE, UNREACHABLE),
        shared_prom_handle(),
    );

    // The Prometheus recorder renders lazily: before any request has been
    // observed, `handle.render()` is an empty string. Generate one sample
    // ourselves so the test is self-contained and does not depend on which
    // other test happened to hit the router first (test order is parallel
    // and unspecified).
    let warmup = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("warmup request must serve /health");
    assert_eq!(warmup.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("router must serve /metrics");

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let body = std::str::from_utf8(&body_bytes).unwrap();
    // Prometheus text format always begins with `# HELP` / `# TYPE`
    // directives; asserting on those proves the recorder was wired
    // through to the handler, not just returning a 200 page.
    assert!(
        body.contains("# HELP") || body.contains("# TYPE"),
        "expected Prometheus text format, got: {body}"
    );
    // And the recorded counter must actually surface under the metric name
    // emitted by metrics_middleware — proving the full wiring, not merely
    // that the exporter printed *some* boilerplate.
    assert!(
        body.contains("http_requests_total"),
        "expected http_requests_total in scrape, got: {body}"
    );
}

#[tokio::test]
async fn router_protected_route_returns_401_without_token() {
    let app = routes::build_router(
        test_config(UNREACHABLE, UNREACHABLE, UNREACHABLE),
        shared_prom_handle(),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/account/auth/me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("router must respond");

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "missing Authorization header must yield 401, not silently proxy"
    );
}

#[tokio::test]
async fn router_protected_route_returns_200_with_valid_jwt() {
    // httpmock gives us a deterministic loopback backend so we can
    // prove the proxy hop actually runs after auth_middleware lets the
    // request through.
    let server = MockServer::start();
    let bearer = format!("Bearer {}", sign_token());
    let mock = server.mock(|when, then| {
        when.method(GET)
            .path("/api/v1/auth/me")
            .header("authorization", bearer.as_str());
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"id":"u-1","email":"x@y.z"}"#);
    });

    let app = routes::build_router(
        test_config(&server.base_url(), UNREACHABLE, UNREACHABLE),
        shared_prom_handle(),
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/account/auth/me")
                .header("authorization", bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("router must respond");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "valid JWT must reach the downstream"
    );
    let body_bytes = to_bytes(response.into_body(), 1024).await.unwrap();
    let body = std::str::from_utf8(&body_bytes).unwrap();
    assert!(
        body.contains("u-1"),
        "downstream body should round-trip: {body}"
    );
    assert_eq!(
        mock.calls(),
        1,
        "downstream must have been hit exactly once"
    );
}

#[tokio::test]
async fn router_account_public_auth_routes_match_downstream_contract() {
    let account = MockServer::start();
    let gateway = routes::build_router(
        test_config(&account.base_url(), UNREACHABLE, UNREACHABLE),
        shared_prom_handle(),
    );

    for (action, body) in [
        (
            "register",
            r#"{"email":"user@test.local","username":"user","password":"test-pass"}"#,
        ),
        (
            "login",
            r#"{"email":"user@test.local","password":"test-pass"}"#,
        ),
        ("refresh", r#"{"refresh_token":"test-refresh-token"}"#),
    ] {
        let mock = account.mock(|when, then| {
            when.method(POST)
                .path(format!("/api/v1/auth/{action}"))
                .query_param("trace", "a+b")
                .header("content-type", "application/json")
                .body(body);
            then.status(200);
        });
        let response = gateway
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/account/auth/{action}?trace=a%2Bb"))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        mock.assert();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn router_jiabu_forwards_encoded_decision_status_request() {
    let jiabu = MockServer::start();
    let gateway = routes::build_router(
        test_config(UNREACHABLE, UNREACHABLE, &jiabu.base_url()),
        shared_prom_handle(),
    );
    let bearer = format!("Bearer {}", sign_token());
    let body = r#"{"status":"decided","rationale":"test decision"}"#;
    // The percent-encoded first digit still represents a valid decision UUID.
    let suffix = "/decisions/%300000000-0000-0000-0000-000000000001/status?trace=a%2Fb&trace=x+y";
    let target = format!("/api/v1{suffix}");
    let mock = jiabu.mock(|when, then| {
        when.method(PUT)
            .is_true(move |req| {
                req.uri().path_and_query().map(|value| value.as_str()) == Some(target.as_str())
            })
            .header("authorization", bearer.as_str())
            .header("content-type", "application/json")
            .body(body);
        then.status(200);
    });

    let response = gateway
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/jiabu{suffix}"))
                .header("authorization", bearer)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    mock.assert();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn router_app_proxy_passes_public_path_through() {
    let app = MockServer::start();
    let forwarded = app.mock(|when, then| {
        when.method(GET).path("/api/v1/app/health");
        then.status(404);
    });
    let gateway = routes::build_router(
        test_config(UNREACHABLE, &app.base_url(), UNREACHABLE),
        shared_prom_handle(),
    );
    let response = gateway
        .oneshot(
            Request::builder()
                .uri("/api/v1/app/health")
                .header("authorization", format!("Bearer {}", sign_token()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    forwarded.assert();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn router_rate_limit_returns_429_after_burst() {
    // Use a fresh RateLimiter per test by building a new router; the
    // limiter's Arc<DashMap> is owned by the router so isolation comes
    // for free. We point the downstream at port 1 (refused) so each
    // request that *passes* the rate limit returns 502 — which lets
    // us distinguish "rate-limited" (429) from "passed through to a
    // broken downstream" (502).
    let app = routes::build_router(
        test_config(UNREACHABLE, UNREACHABLE, UNREACHABLE),
        shared_prom_handle(),
    );

    // rate_limit_per_minute = 3; send 4 requests, expect first 3 to
    // be 502 (passed rate-limit, downstream refused) and the 4th to
    // be 429.
    let mut statuses = Vec::new();
    for _ in 0..4 {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/account/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"email":"a@b.com","password":"x"}"#))
                    .unwrap(),
            )
            .await
            .expect("router must respond");
        statuses.push(resp.status());
    }

    assert_eq!(
        statuses[0],
        StatusCode::BAD_GATEWAY,
        "1st request: rate-limit pass-through, downstream refused"
    );
    assert_eq!(statuses[1], StatusCode::BAD_GATEWAY);
    assert_eq!(statuses[2], StatusCode::BAD_GATEWAY);
    assert_eq!(
        statuses[3],
        StatusCode::TOO_MANY_REQUESTS,
        "4th request must be rate-limited (limit=3/min)"
    );
}

#[tokio::test]
async fn router_readyz_uses_each_downstream_probe_contract() {
    let account = MockServer::start();
    let app = MockServer::start();
    let jiabu = MockServer::start();
    let account_ready = account.mock(|when, then| {
        when.method(GET).path("/readyz");
        then.status(200);
    });
    let app_health = app.mock(|when, then| {
        when.method(GET).path("/api/v1/health");
        then.status(200);
    });
    let jiabu_live = jiabu.mock(|when, then| {
        when.method(GET).path("/health");
        then.status(200);
    });
    let gateway = routes::build_router(
        test_config(&account.base_url(), &app.base_url(), &jiabu.base_url()),
        shared_prom_handle(),
    );

    for status in [StatusCode::OK, StatusCode::SERVICE_UNAVAILABLE] {
        let mut jiabu_ready = jiabu.mock(|when, then| {
            when.method(GET).path("/readyz");
            then.status(status.as_u16());
        });
        let response = gateway
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        jiabu_ready.assert();
        assert_eq!(response.status(), status);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let report: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(report["healthy"], status.is_success());
        jiabu_ready.delete();
    }

    assert_eq!(account_ready.calls(), 2);
    assert_eq!(app_health.calls(), 2);
    assert_eq!(jiabu_live.calls(), 0);
}

#[tokio::test]
async fn router_readyz_returns_503_when_downstream_unreachable() {
    // /readyz probes each downstream. With all backends pointing at
    // port 1 (refused), every probe fails and the aggregate must
    // return 503 — this is the round-4 contract that lets k8s drain
    // a broken pod from the service.
    let app = routes::build_router(
        test_config(UNREACHABLE, UNREACHABLE, UNREACHABLE),
        shared_prom_handle(),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("router must serve /readyz");

    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "readyz must report 503 when all backends unreachable"
    );
    let body_bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let body = std::str::from_utf8(&body_bytes).unwrap();
    eprintln!("readyz body: {body}");
    assert!(body.contains("\"healthy\":false"), "body: {body}");
    assert!(body.contains("account"), "body must list backends: {body}");
    assert!(body.contains("app"), "body must list backends: {body}");
    assert!(body.contains("jiabu"), "body must list backends: {body}");
}
