//! Slice in the browser.
//!
//! A Leptos client-side application over [`slice_core`]. Everything runs locally: the
//! font is read by the browser, sliced by WebAssembly, and handed back as a download.
//! No part of it is uploaded anywhere.

mod app;
mod files;
mod recent;
mod settings;
mod state;
mod ui;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() {
    // Turns a Rust panic into a readable stack trace in the browser console instead of
    // the bare "unreachable executed" that WebAssembly would otherwise report.
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
