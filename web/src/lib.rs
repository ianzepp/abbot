// Abbot web frontend - Leptos/WASM.
//
// This crate provides the web UI for Abbot, built with Leptos and compiled to
// WebAssembly. It connects to the backend via WebSocket for real-time updates
// and REST API for data fetching.
//
// To build: trunk build
// To serve with hot reload: trunk serve

mod api;
mod app;
mod bus;
mod components;
mod state;

pub use app::App;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();
    let root = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document")
        .get_element_by_id("root")
        .expect("no #root element");
    leptos::mount::mount_to(root.unchecked_into(), App).forget();
}
