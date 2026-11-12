//! Harness client — Leptos CSR, compiled to wasm32, shipped as one HTML file.

pub mod agent;
pub mod js;
pub mod net;
pub mod state;
pub mod storage;
pub mod ui;

use wasm_bindgen::prelude::*;

/// Entry point called by the packed loader (or Trunk's `main`).
#[wasm_bindgen]
pub fn run_app() {
    console_error_panic_hook::set_once();
    ui::app::mount();
}
