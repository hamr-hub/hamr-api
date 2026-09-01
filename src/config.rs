use anyhow::{Context, Result};

/// Minimum acceptable JWT_SECRET length in bytes. HS256 needs at least
/// 256 bits (32 bytes) of entropy; refusing anything shorter prevents
/// trivial brute-force against the HMAC key.
pub const MIN_JWT_SECRET_LEN: usize = 32;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub jwt_secret: String,
    pub account_service_url: String,
    pub app_service_url: String,
    pub jiabu_service_url: String,
    pub rate_limit_per_minute: u32,
    pub cors_allowed_origins: Vec<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        // JWT_SECRET is mandatory and must be long enough. A silently
        // accepted default (e.g. "dev-secret-change-in-production")
        // is exactly what P0 round-2 flagged: a missing env var should
        // refuse to start, not silently boot with a guessable key.
        let jwt_secret = std::env::var("JWT_SECRET")
            .context("JWT_SECRET must be set (HS256 requires >= 32 bytes)")?;
        if jwt_secret.len() < MIN_JWT_SECRET_LEN {
            anyhow::bail!(
                "JWT_SECRET is too short: got {} bytes, need at least {}",
                jwt_secret.len(),
                MIN_JWT_SECRET_LEN
            );
        }

        Ok(Self {
            port: std::env::var("PORT")
                .unwrap_or_else(|_| "8090".to_string())
                .parse()?,
            jwt_secret,
            account_service_url: std::env::var("ACCOUNT_SERVICE_URL")
                .unwrap_or_else(|_| "http://hamr-account-api:8080".to_string()),
            app_service_url: std::env::var("APP_SERVICE_URL")
                .unwrap_or_else(|_| "http://hamr-app-api:8081".to_string()),
            jiabu_service_url: std::env::var("JIABU_SERVICE_URL")
                .unwrap_or_else(|_| "http://hamr-jiabu-api:8082".to_string()),
            rate_limit_per_minute: std::env::var("RATE_LIMIT_PER_MINUTE")
                .unwrap_or_else(|_| "60".to_string())
                .parse()?,
            cors_allowed_origins: parse_origins(
                &std::env::var("CORS_ALLOWED_ORIGINS").unwrap_or_else(|_| {
                    // Dev-friendly defaults. Production must override via env.
                    "http://localhost:3000,http://localhost:5173,https://hamr.top".to_string()
                }),
            ),
        })
    }
}

fn parse_origins(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Process-wide env-var mutex: cargo runs tests in parallel threads
    // within the same binary, and `set_var` / `remove_var` mutate
    // process state. Without this lock, two tests racing to set
    // JWT_SECRET would see each other's values and produce flakes.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Helper: load config with a forced JWT_SECRET value (and
    /// clean any pre-existing JWT_SECRET from the test env).
    fn load_with_jwt(secret: Option<&str>) -> Result<Config> {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Snapshot and clear so each test starts from a known state.
        let prev = std::env::var("JWT_SECRET").ok();
        match secret {
            Some(s) => std::env::set_var("JWT_SECRET", s),
            None => std::env::remove_var("JWT_SECRET"),
        }
        let result = Config::from_env();
        // Restore.
        match prev {
            Some(v) => std::env::set_var("JWT_SECRET", v),
            None => std::env::remove_var("JWT_SECRET"),
        }
        result
    }

    #[test]
    fn parse_origins_trims_and_filters_empty() {
        let out = parse_origins(" http://a , , https://b ,");
        assert_eq!(out, vec!["http://a", "https://b"]);
    }

    #[test]
    fn parse_origins_keeps_single_entry() {
        assert_eq!(parse_origins("https://only.example"), vec!["https://only.example"]);
    }

    #[test]
    fn jwt_secret_missing_returns_err() {
        let err = load_with_jwt(None).expect_err("missing JWT_SECRET must error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("JWT_SECRET must be set"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn jwt_secret_too_short_returns_err() {
        // 31 bytes — one below the minimum.
        let short = "a".repeat(MIN_JWT_SECRET_LEN - 1);
        let err = load_with_jwt(Some(&short)).expect_err("short JWT_SECRET must error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("too short"),
            "unexpected error message: {msg}"
        );
        assert!(
            msg.contains(&MIN_JWT_SECRET_LEN.to_string()),
            "error should mention the minimum length"
        );
    }

    #[test]
    fn jwt_secret_at_minimum_length_is_accepted() {
        let ok = "a".repeat(MIN_JWT_SECRET_LEN);
        let cfg = load_with_jwt(Some(&ok)).expect("32-byte JWT_SECRET must be accepted");
        assert_eq!(cfg.jwt_secret.len(), MIN_JWT_SECRET_LEN);
    }
}
