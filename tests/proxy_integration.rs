//! HTTP-level integration tests for `proxy::proxy_request`.
//!
//! Round-3 (iter-skill 2026-09-01) introduced these to fill a gap the
//! unit tests couldn't: the proxy forwards HTTP requests to downstreams,
//! but the unit tests only exercised the in-process config + rate-limit
//! logic. Without a mocked downstream we couldn't prove that:
//!   1. method / path / query string survive the hop
//!   2. request body bytes are forwarded intact
//!   3. response status + headers + body come back unchanged
//!   4. non-2xx responses are NOT silently turned into 502s by reqwest
//!
//! `httpmock` provides a real loopback HTTP server so we exercise the
//! actual reqwest client — no real backend required.
//!
//! IMPORTANT: these run against `proxy::proxy_request` directly (the
//! inner building block), not through the full axum router. The router
//! integration is covered by manual smoke-testing the running container;
//! `axum::Router` testing requires either `tower::ServiceExt::oneshot`
//! or a TestServer, both of which would need additional dev-deps.

use axum::body::to_bytes;
use axum::http::{HeaderMap, HeaderValue, Method};
use hamr_api_gateway::errors::GatewayError;
use httpmock::prelude::*;

#[tokio::test]
async fn proxy_request_forwards_method_path_query_body_and_headers() {
    let server = MockServer::start();

    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/api/v1/account/auth/login")
            .query_param("trace", "abc123")
            .header("content-type", "application/json")
            .header("x-tenant", "tenant-42")
            .body(r#"{"email":"a@b.com","password":"hunter2"}"#);
        then.status(201)
            .header("x-backend", "account-api")
            .header("content-type", "application/json")
            .body(r#"{"token":"jwt-xyz","expires_in":3600}"#);
    });

    // Build a request that the proxy would forward. In production the
    // upstream builds these from axum::Request, but `proxy_request`
    // only cares about the parts it copies across.
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("x-tenant", HeaderValue::from_static("tenant-42"));
    // `host` is intentionally included — the proxy MUST strip it before
    // copying headers to the downstream, otherwise reqwest refuses the
    // request. The forwarder already does this in src/proxy.rs.
    headers.insert("host", HeaderValue::from_static("evil.example.com"));

    let response = hamr_api_gateway::proxy::proxy_request(
        &format!(
            "{}/api/v1/account/auth/login?trace=abc123",
            server.base_url()
        ),
        Method::POST,
        headers,
        br#"{"email":"a@b.com","password":"hunter2"}"#.to_vec(),
    )
    .await
    .expect("proxy must succeed against mocked backend");

    // Status + body round-trip.
    assert_eq!(response.status().as_u16(), 201);
    let body_bytes = to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body collection should succeed");
    assert_eq!(
        std::str::from_utf8(&body_bytes).unwrap(),
        r#"{"token":"jwt-xyz","expires_in":3600}"#
    );

    // The mock asserted on the exact body + headers it received; if any
    // of those were dropped/mangled by the proxy, this `hits` count
    // would be 0.
    assert_eq!(mock.calls(), 1, "backend must have been hit exactly once");
}

#[tokio::test]
async fn proxy_request_propagates_non_2xx_response_unchanged() {
    // A 404 from the downstream must NOT be turned into a 502 by the
    // gateway. The whole point of the proxy layer is pass-through.
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path("/api/v1/app/does-not-exist");
        then.status(404)
            .header("content-type", "application/json")
            .body(r#"{"error":"resource not found"}"#);
    });

    let response = hamr_api_gateway::proxy::proxy_request(
        &format!("{}/api/v1/app/does-not-exist", server.base_url()),
        Method::GET,
        HeaderMap::new(),
        vec![],
    )
    .await
    .expect("proxy must succeed even when downstream returns 404");

    assert_eq!(
        response.status().as_u16(),
        404,
        "downstream status must be passed through unmodified"
    );
    let body_bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    assert!(std::str::from_utf8(&body_bytes)
        .unwrap()
        .contains("resource not found"));
    assert_eq!(mock.calls(), 1);
}

#[tokio::test]
async fn proxy_request_returns_bad_gateway_on_connection_refused() {
    // Port 1 is reserved/unbound on the test host — connection must
    // fail fast. The proxy should map this to `BadGateway` so callers
    // see a meaningful error.
    let result = hamr_api_gateway::proxy::proxy_request(
        "http://127.0.0.1:1/api/v1/account/health",
        Method::GET,
        HeaderMap::new(),
        vec![],
    )
    .await;

    assert!(result.is_err(), "refused connection must yield an error");
    match result.unwrap_err() {
        GatewayError::BadGateway => {}
        other => panic!("expected BadGateway, got {:?}", other),
    }
}
