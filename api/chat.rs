//! POST /api/chat — SSE proxy to OpenRouter chat completions.
//! Deployed as a Vercel function (Fluid, streaming). maxDuration is set in
//! vercel.json.

use tower::Layer;

use harness_proxy::{chat_router, ProxyConfig};
use vercel_runtime::axum::VercelLayer;
use vercel_runtime::{run, Error};

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Not the loopback dev server: the bearer gate is mandatory.
    let config = ProxyConfig::from_env(false);
    let app = chat_router(config);
    run(VercelLayer::new().layer(app)).await
}
