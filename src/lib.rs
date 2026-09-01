//! Library crate re-exports for the integration-test target.
//!
//! `main.rs` remains the binary entry point that wires modules into the
//! running gateway. `lib.rs` exists so that `tests/proxy_integration.rs`
//! (and any future integration tests) can `use hamr_api_gateway::proxy`
//! without spinning up the binary. Every public module is re-exported
//! here exactly once — keep this list in sync with `mod ...;` in
//! `main.rs`.
//!
//! Round-4 (iter-skill 2026-09-01) — added so httpmock integration
//! tests can drive `proxy_request` end-to-end without going through
//! axum.

pub mod config;
pub mod errors;
pub mod health;
pub mod metrics;
pub mod middleware;
pub mod proxy;
pub mod routes;
