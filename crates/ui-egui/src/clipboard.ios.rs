//! Fallback clipboard on iOS, under the name `arboard` that `clipboard.rs` reads.
//!
//! `arboard` has no iOS backend: the `ios/` app shell injects a real `UIPasteboard`-backed
//! `Board` via `NocturneApp::with_clipboard` at startup. Keeping this stand-in inside
//! `crates/ui-egui` keeps the UI crate self-contained without `unsafe` code; if anything
//! reaches the fallback `clipboard::System` or `clipboard::open` path directly on iOS, it
//! returns `ClipboardNotSupported` rather than letting operations fail silently.

use std::borrow::Cow;
use std::fmt;

/// Why the clipboard cannot be used. Only the cases `clipboard.rs` names are needed.
#[derive(Debug)]
pub enum Error {
    /// Never returned here; `clipboard.rs` retries on it on the desktop.
    #[allow(dead_code)]
    ClipboardOccupied,
    /// The `UIPasteboard` bridge was not injected by an iOS app shell.
    ClipboardNotSupported,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::ClipboardOccupied => f.write_str("the clipboard is in use by another app"),
            Error::ClipboardNotSupported => f.write_str("the iOS clipboard requires the app shell's UIPasteboard bridge"),
        }
    }
}

/// A picture as `arboard` hands it over: unmultiplied RGBA, row by row.
pub struct ImageData<'a> {
    pub width: usize,
    pub height: usize,
    pub bytes: Cow<'a, [u8]>,
}

/// Fallback clipboard when no `UIPasteboard` bridge is attached.
pub struct Clipboard;

impl Clipboard {
    pub fn new() -> Result<Clipboard, Error> {
        Err(Error::ClipboardNotSupported)
    }
    pub fn get_image(&mut self) -> Result<ImageData<'static>, Error> {
        Err(Error::ClipboardNotSupported)
    }
    pub fn get_text(&mut self) -> Result<String, Error> {
        Err(Error::ClipboardNotSupported)
    }
    pub fn set_text(&mut self, _text: String) -> Result<(), Error> {
        Err(Error::ClipboardNotSupported)
    }
    pub fn set(&mut self) -> Set {
        Set
    }
    pub fn get(&mut self) -> Get {
        Get
    }
}

/// `Clipboard::set()`: the HTML flavour with its plain-text alternative.
pub struct Set;

impl Set {
    pub fn html(self, _html: String, _alt_text: Option<String>) -> Result<(), Error> {
        Err(Error::ClipboardNotSupported)
    }
}

/// `Clipboard::get()`: the HTML flavour.
pub struct Get;

impl Get {
    pub fn html(self) -> Result<String, Error> {
        Err(Error::ClipboardNotSupported)
    }
}
