use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum GatewayError {
    #[error("Unauthorized")]
    Unauthorized,
    #[error("Rate limit exceeded")]
    RateLimitExceeded,
    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),
    #[error("Bad gateway")]
    BadGateway,
    #[error("Not found")]
    NotFound,
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            GatewayError::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized"),
            GatewayError::RateLimitExceeded => (StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded"),
            GatewayError::ServiceUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "Service unavailable"),
            GatewayError::BadGateway => (StatusCode::BAD_GATEWAY, "Bad gateway"),
            GatewayError::NotFound => (StatusCode::NOT_FOUND, "Not found"),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub type GatewayResult<T> = Result<T, GatewayError>;
