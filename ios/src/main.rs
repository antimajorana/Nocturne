#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::process::ExitCode;

#[cfg(target_os = "ios")]
fn main() -> ExitCode {
    if let Err(error) = nocturne_ios::main_entry() {
        eprintln!("Nocturne could not start: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(not(target_os = "ios"))]
fn main() -> ExitCode {
    eprintln!("nocturne-ios is only supported on iOS / iPadOS (target_os = \"ios\").");
    ExitCode::FAILURE
}
