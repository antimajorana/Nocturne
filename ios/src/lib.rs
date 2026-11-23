#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
//! Nocturne for iPadOS / iOS.
//!
//! Native `eframe` + `wgpu` (Metal) shell for iPadOS and iOS, sharing Nocturne's
//! engine, store, formats, and `egui` UI while bridging UIKit's `UIPasteboard`,
//! sandbox storage paths, and incoming `Documents/Inbox` files.

#[cfg(target_os = "ios")]
pub mod app;
#[cfg(target_os = "ios")]
pub mod clipboard;
pub mod diagnostics;
pub mod edit_menu;
pub mod gestures;
pub mod http;
pub mod inbox;
pub mod keyboard;
pub mod keychain;
pub mod pasteboard_codec;
pub mod pencil;
pub mod storage;
pub mod text_input;
pub mod vision_ocr;

/// Starts Nocturne inside the iOS / iPadOS UIKit event loop.
#[cfg(target_os = "ios")]
pub fn main_entry() -> Result<(), String> {
    let result = app::run();
    if let Err(ref error) = result {
        log::error!("Nocturne could not start: {error}");
        app::export_current_diagnostics_log();
    }
    result
}
