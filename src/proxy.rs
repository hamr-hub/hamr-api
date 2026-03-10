use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use reqwest::Client;

use crate::{config::Config, errors::GatewayError};

static CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();

fn get_client() -> &'static Client {
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client")
    })
}

pub async fn proxy_request(
    target_url: &str,
    method: reqwest::Method,
    headers: HeaderMap,
    body: Vec<u8>,
) -> Result<Response, GatewayError> {
    let client = get_client();

    let mut req = client.request(method, target_url);

    for (key, value) in headers.iter() {
        if key != "host" {
            req = req.header(key, value);
        }
    }

    let resp = req
        .body(body)
        .send()
        .await
        .map_err(|_| GatewayError::BadGateway)?;

    let status = StatusCode::from_u16(resp.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let resp_headers = resp.headers().clone();
    let body = resp.bytes().await.map_err(|_| GatewayError::BadGateway)?;

    let mut response = Response::builder().status(status);
    for (key, value) in resp_headers.iter() {
        if key != "transfer-encoding" {
            response = response.header(key, value);
        }
    }

    Ok(response
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response()))
}

pub async fn forward_to_account(
    State(config): State<Config>,
    request: Request,
) -> Result<Response, GatewayError> {
    forward(request, &config.account_service_url).await
}

pub async fn forward_to_app(
    State(config): State<Config>,
    request: Request,
) -> Result<Response, GatewayError> {
    forward(request, &config.app_service_url).await
}

pub async fn forward_to_jiabu(
    State(config): State<Config>,
    request: Request,
) -> Result<Response, GatewayError> {
    forward(request, &config.jiabu_service_url).await
}

async fn forward(request: Request, base_url: &str) -> Result<Response, GatewayError> {
    let method_str = request.method().as_str();
    let method = reqwest::Method::from_bytes(method_str.as_bytes())
        .map_err(|_| GatewayError::BadGateway)?;

    let path = request.uri().path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");

    let target = format!("{}{}", base_url, path);

    let headers = request.headers().clone();
    let body = axum::body::to_bytes(request.into_body(), 10 * 1024 * 1024)
        .await
        .map_err(|_| GatewayError::BadGateway)?
        .to_vec();

    proxy_request(&target, method, headers, body).await
}
