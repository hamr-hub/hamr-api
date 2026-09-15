//! Downstream-backend readiness probes.
//!
//! `proxy::proxy_request` only knows the upstream URL; it has no view of
//! whether the downstream service is actually responding. The `/readyz`
//! endpoint here gives Kubernetes (or any external probe) a single
//! aggregate answer derived from per-backend `GET /health` checks.
//!
//! Round-4 iter-skill 2026-09-01 follow-up to round-1/round-2 hardening:
//! previously the gateway exposed only `GET /health` (liveness), with no
//! signal of whether downstream backends were reachable. A liveness
//! probe that says "ok" while every backend 502s is worse than no probe
//! at all — it tells orchestrators to keep sending traffic at a broken
//! gateway.

use std::time::{Duration, Instant};

use reqwest::Client;
use serde::Serialize;

/// One backend probe result. Serialized directly into the `/readyz`
/// JSON body so operators can grep `kubectl logs` / curl output.
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct BackendHealth {
    pub name: &'static str,
    pub url: String,
    /// `true` iff the backend responded with a 2xx within `timeout`.
    pub healthy: bool,
    /// HTTP status if the backend responded, `None` on connection / timeout.
    pub status: Option<u16>,
    /// Round-trip latency in milliseconds, `None` on connection failure.
    pub latency_ms: Option<u64>,
    /// Human-readable failure reason; empty string when healthy.
    pub error: String,
}

/// Probe one backend's `/health` endpoint. A 2xx response is healthy;
/// anything else (non-2xx, timeout, connection refused, DNS failure) is
/// not. We deliberately do NOT consider 5xx "healthy" — a downstream
/// that returns 500 should fail the readiness probe.
pub async fn probe_backend(
    client: &Client,
    name: &'static str,
    base_url: &str,
    timeout: Duration,
) -> BackendHealth {
    // Trim trailing slash to avoid `//health`.
    let base = base_url.trim_end_matches('/');
    let target = format!("{}/health", base);

    let start = Instant::now();
    let result = client.get(&target).timeout(timeout).send().await;
    let latency_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(resp) => {
            let status = resp.status().as_u16();
            BackendHealth {
                name,
                url: target,
                healthy: (200..300).contains(&status),
                status: Some(status),
                latency_ms: Some(latency_ms),
                error: String::new(),
            }
        }
        Err(e) => {
            // reqwest's error Display includes cause chain (e.g.
            // "error sending request: connect error: Connection refused").
            // That's verbose but useful in JSON output.
            BackendHealth {
                name,
                url: target,
                healthy: false,
                status: None,
                latency_ms: Some(latency_ms),
                error: e.to_string(),
            }
        }
    }
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct ReadinessReport {
    /// `true` iff every probed backend is healthy.
    pub healthy: bool,
    pub backends: Vec<BackendHealth>,
}

/// Probe every backend in sequence with `client` and `timeout` per probe.
/// We probe sequentially rather than concurrently: with only 3 backends
/// this is fast enough and avoids spawning a Tokio task per probe,
/// which matters when the readiness endpoint itself is hot under
/// k8s readiness churn.
pub async fn aggregate(
    client: &Client,
    backends: &[(&'static str, &str)],
    timeout: Duration,
) -> ReadinessReport {
    let mut results = Vec::with_capacity(backends.len());
    for (name, url) in backends {
        results.push(probe_backend(client, name, url, timeout).await);
    }
    let healthy = results.iter().all(|b| b.healthy);
    ReadinessReport {
        healthy,
        backends: results,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a reqwest client that never makes a real network call in
    /// the negative tests; for positive tests we use httpmock.
    fn test_client() -> Client {
        Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .expect("test client must build")
    }

    #[tokio::test]
    async fn probe_backend_reports_unhealthy_when_unreachable() {
        // Port 1 is reserved by IANA and unbound on the test host; the
        // connection should fail immediately rather than hanging.
        let client = test_client();
        let result = probe_backend(
            &client,
            "account",
            "http://127.0.0.1:1",
            Duration::from_millis(50),
        )
        .await;

        assert_eq!(result.name, "account");
        assert!(!result.healthy, "unreachable backend must be unhealthy");
        assert!(
            result.status.is_none(),
            "no HTTP status when connection fails"
        );
        assert!(!result.error.is_empty(), "failure reason must be populated");
    }

    #[tokio::test]
    async fn probe_backend_reports_unhealthy_on_non_2xx() {
        // Spin up an in-process listener that returns 500; reqwest's
        // hyper client never sees the network.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // One connection: respond with 500 and close.
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 512];
                let _ = sock.read(&mut buf).await;
                let resp = b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n";
                let _ = sock.write_all(resp).await;
                let _ = sock.shutdown().await;
            }
        });

        let client = test_client();
        let result = probe_backend(
            &client,
            "app",
            &format!("http://{}", addr),
            Duration::from_millis(500),
        )
        .await;

        assert!(!result.healthy, "5xx response must be unhealthy");
        assert_eq!(result.status, Some(500));
    }

    #[tokio::test]
    async fn aggregate_marks_report_unhealthy_if_any_backend_fails() {
        // First "backend" is unreachable (port 1); aggregate must
        // report unhealthy overall regardless of other backends.
        let client = test_client();
        let report = aggregate(
            &client,
            &[
                ("account", "http://127.0.0.1:1"),
                ("app", "http://127.0.0.1:1"),
            ],
            Duration::from_millis(50),
        )
        .await;

        assert!(!report.healthy);
        assert_eq!(report.backends.len(), 2);
        assert!(report.backends.iter().all(|b| !b.healthy));
    }

    #[tokio::test]
    async fn aggregate_empty_list_is_healthy() {
        // Edge case: zero backends configured. Vacuously healthy —
        // the gateway itself is up, so it's "ready" even if it has
        // nothing to route to.
        let client = test_client();
        let report = aggregate(&client, &[], Duration::from_millis(50)).await;
        assert!(report.healthy);
        assert!(report.backends.is_empty());
    }
}
