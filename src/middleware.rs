use std::sync::Arc;
use std::time::{Duration, Instant};
use axum::{
    extract::{ConnectInfo, Request, State},
    middleware::Next,
    response::Response,
};
use dashmap::DashMap;
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

use crate::{config::Config, errors::GatewayError};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,
    pub email: String,
    pub username: String,
    pub exp: i64,
}

#[derive(Clone)]
pub struct RateLimiter {
    map: Arc<DashMap<String, (u32, Instant)>>,
    limit: u32,
}

impl RateLimiter {
    pub fn new(limit: u32) -> Self {
        Self {
            map: Arc::new(DashMap::new()),
            limit,
        }
    }

    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let window = Duration::from_secs(60);

        let mut entry = self.map.entry(key.to_string()).or_insert((0, now));
        if now.duration_since(entry.1) > window {
            *entry = (1, now);
            return true;
        }
        if entry.0 >= self.limit {
            return false;
        }
        entry.0 += 1;
        true
    }
}

pub async fn auth_middleware(
    State(config): State<Config>,
    mut request: Request,
    next: Next,
) -> Result<Response, GatewayError> {
    let token = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match token {
        Some(t) => {
            let claims = decode::<Claims>(
                t,
                &DecodingKey::from_secret(config.jwt_secret.as_bytes()),
                &Validation::default(),
            )
            .map_err(|_| GatewayError::Unauthorized)?
            .claims;
            request.extensions_mut().insert(claims);
            Ok(next.run(request).await)
        }
        None => Err(GatewayError::Unauthorized),
    }
}

pub async fn rate_limit_middleware(
    State(limiter): State<RateLimiter>,
    request: Request,
    next: Next,
) -> Result<Response, GatewayError> {
    // Key priority:
    //   1. JWT sub from auth_middleware (most accurate per-user)
    //   2. ConnectInfo peer addr (real socket addr, not spoofable)
    //   3. X-Forwarded-For last segment (only useful when behind a
    //      trusted proxy that overwrites the header; caller must verify
    //      proxy chain at deploy time)
    //
    // Previous implementation used X-Forwarded-For directly, which is
    // trivially spoofable by clients to bypass rate limits (P0 from
    // iter-skill 2026-07-01).
    let key = request
        .extensions()
        .get::<Claims>()
        .map(|c| format!("user:{}", c.sub))
        .or_else(|| {
            request
                .extensions()
                .get::<ConnectInfo<std::net::SocketAddr>>()
                .map(|ci| format!("ip:{}", ci.0.ip()))
        })
        .or_else(|| {
            request
                .headers()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(',').next())
                .map(|s| format!("xff:{}", s.trim()))
        })
        .unwrap_or_else(|| "unknown".to_string());

    if !limiter.check(&key) {
        tracing::warn!("hamr-api rate limit exceeded for key={}", key);
        return Err(GatewayError::RateLimitExceeded);
    }
    Ok(next.run(request).await)
}
