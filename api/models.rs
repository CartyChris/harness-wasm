//! GET /api/models — relay the OpenRouter model list.

use tower::Layer;

use harness_proxy::{models_router, ProxyConfig};
use vercel_runtime::axum::VercelLayer;
use vercel_runtime::{run, Error};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let config = ProxyConfig::from_env(false);
    let app = models_router(config);
    run(VercelLayer::new().layer(app)).await
}
