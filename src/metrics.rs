use std::time::Instant;
use axum::{
    extract::Request,
    middleware::Next,
    response::Response,
};
use metrics::{counter, histogram};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};

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
    let path = request.uri().path().to_string();
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
    let route = normalize_path(&path);

    counter!("http_requests_total",
        "method" => method.clone(),
        "path" => route.clone(),
        "status" => status.clone()
    ).increment(1);

    histogram!("http_request_duration_seconds",
        "method" => method.clone(),
        "path" => route.clone()
    ).record(duration);

    if response.status().is_server_error() {
        counter!("http_errors_total",
            "method" => method.clone(),
            "path" => route.clone(),
            "status" => status.clone()
        ).increment(1);
    }

    let duration_ms = (duration * 1000.0) as u64;

    tracing::info!(
        method = %method,
        path = %path,
        status = %status,
        duration_ms = duration_ms,
        client_ip = %client_ip,
        "request"
    );

    response
}

fn normalize_path(path: &str) -> String {
    if path.starts_with("/api/v1/account") {
        "/api/v1/account/*".to_string()
    } else if path.starts_with("/api/v1/app") {
        "/api/v1/app/*".to_string()
    } else if path.starts_with("/api/v1/jiabu") {
        "/api/v1/jiabu/*".to_string()
    } else {
        path.to_string()
    }
}
