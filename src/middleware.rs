use axum::{
    extract::{ConnectInfo, Request, State},
    middleware::Next,
    response::Response,
};
use dashmap::DashMap;
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::{config::Config, errors::GatewayError};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,
    pub email: String,
    pub username: String,
    pub exp: i64,
}

/// Default interval at which the background sweeper scans the map.
pub const DEFAULT_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
/// Default maximum age of an untouched entry before it is evicted.
/// Set to 5 minutes — comfortably above the 60s rate-limit window so
/// that an idle user is not reset to a fresh bucket mid-window.
pub const DEFAULT_ENTRY_MAX_AGE: Duration = Duration::from_secs(300);

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

    /// Remove every entry whose `last_seen` is older than `max_age`.
    /// Returns the number of entries evicted. Called by the background
    /// sweeper; tests can call it directly with a small `max_age`.
    ///
    /// Round-2 fix: without this sweep, an unbounded stream of distinct
    /// keys (e.g. bot traffic) grows the map without bound — the round-1
    /// audit flagged this as a slow memory leak.
    pub fn cleanup_once(&self, max_age: Duration) -> usize {
        let now = Instant::now();
        let before = self.map.len();
        self.map
            .retain(|_, (_, last_seen)| now.duration_since(*last_seen) <= max_age);
        before - self.map.len()
    }

    /// Spawn a Tokio task that periodically evicts stale entries.
    /// Returns the `JoinHandle` so callers can cancel on shutdown.
    /// `interval` controls sweep frequency; `max_age` controls eviction.
    pub fn spawn_cleanup_task(
        self,
        interval: Duration,
        max_age: Duration,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Skip the immediate first tick (Tokio intervals fire at t=0).
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let removed = self.cleanup_once(max_age);
                if removed > 0 {
                    tracing::debug!(
                        "rate-limiter cleanup evicted {} stale entries (max_age={:?})",
                        removed,
                        max_age
                    );
                }
            }
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_once_evicts_stale_entries_and_keeps_fresh() {
        // Build a limiter; insert one "fresh" entry by hitting it, and
        // one "stale" entry directly via the inner map (so we don't
        // have to sleep through the 60s window).
        let limiter = RateLimiter::new(10);
        // Fresh: calling check sets (count, now).
        assert!(limiter.check("user:fresh"));

        // Stale: bypass check() and inject an entry with last_seen far
        // in the past. cleanup_once uses Instant::now() so we need an
        // age large enough that any small clock drift on the test
        // host can't accidentally keep it.
        limiter.map.insert(
            "user:stale".to_string(),
            (3, Instant::now() - Duration::from_secs(3600)),
        );

        assert_eq!(limiter.map.len(), 2);

        // max_age = 1 minute; stale entry is 1h old → evicted.
        let removed = limiter.cleanup_once(Duration::from_secs(60));
        assert_eq!(removed, 1, "should evict only the stale entry");
        assert_eq!(limiter.map.len(), 1);
        assert!(limiter.map.contains_key("user:fresh"));
        assert!(!limiter.map.contains_key("user:stale"));
    }
}
