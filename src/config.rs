use anyhow::Result;

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
        Ok(Self {
            port: std::env::var("PORT")
                .unwrap_or_else(|_| "8090".to_string())
                .parse()?,
            jwt_secret: std::env::var("JWT_SECRET")
                .unwrap_or_else(|_| "dev-secret-change-in-production".to_string()),
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

    #[test]
    fn parse_origins_trims_and_filters_empty() {
        let out = parse_origins(" http://a , , https://b ,");
        assert_eq!(out, vec!["http://a", "https://b"]);
    }

    #[test]
    fn parse_origins_keeps_single_entry() {
        assert_eq!(parse_origins("https://only.example"), vec!["https://only.example"]);
    }
}
