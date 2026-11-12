//! GET /api/health — public liveness, no secrets.

use tower::Layer;

use harness_proxy::{health_router, ProxyConfig};
use vercel_runtime::axum::VercelLayer;
use vercel_runtime::{run, Error};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let config = ProxyConfig::from_env(false);
    let app = health_router(config);
    run(VercelLayer::new().layer(app)).await
}
