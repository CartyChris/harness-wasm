//! Local dev server: the same handlers as the Vercel functions, bound to
//! loopback ONLY (never a public interface).
//!
//! If HARNESS_ACCESS_TOKEN is empty, requests are allowed because the bind
//! address is 127.0.0.1 (loopback exception, DESIGN.md §5). Set the token to
//! exercise the bearer gate locally.

use harness_proxy::{full_router, ProxyConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = std::env::var("HARNESS_DEV_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8788);

    // Loopback only.
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let config = ProxyConfig::from_env(true);
    if config.access_token.is_none() {
        eprintln!(
            "[harness-dev] HARNESS_ACCESS_TOKEN empty: bearer gate open for loopback only ({addr})"
        );
    }

    let app = full_router(config);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("[harness-dev] listening on http://{addr}/ (loopback only)");
    axum::serve(listener, app).await?;
    Ok(())
}
