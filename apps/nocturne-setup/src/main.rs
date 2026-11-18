#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! Launcher for the installed Nocturne app. The Welcome notebook is no longer installed here:
//! the app's first-use wizard asks for the logging level, the page format and whether to add the
//! demo notebook, then installs the chosen edition itself.

#[cfg(not(target_os = "macos"))]
use std::path::Path;
#[cfg(not(target_os = "macos"))]
use std::process::Command;

fn main() {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => fail(&format!("Could not locate the Nocturne setup tool: {error}")),
    };

    #[cfg(target_os = "macos")]
    {
        let _ = executable;
        show_notice("Nocturne is ready to use. The Welcome notebook is offered the first time you open Nocturne.");
    }

    #[cfg(not(target_os = "macos"))]
    {
        launch_desktop_app(&executable);
    }
}

#[cfg(not(target_os = "macos"))]
fn sibling_app(executable: &Path) -> Option<std::path::PathBuf> {
    let name = if cfg!(windows) { "nocturne.exe" } else { "nocturne" };
    let path = executable.parent()?.join(name);
    path.is_file().then_some(path)
}

#[cfg(not(target_os = "macos"))]
fn launch_desktop_app(executable: &Path) {
    let app = match sibling_app(executable) {
        Some(path) => path,
        None => fail("The Nocturne app is missing beside its setup tool. Reinstall Nocturne and try again."),
    };
    let mut command = Command::new(app);
    command.args(std::env::args().skip(1));
    match command.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => fail(&format!("Nocturne could not be opened: {error}")),
    }
}

#[cfg(target_os = "macos")]
fn show_notice(message: &str) {
    let _ = rfd::MessageDialog::new().set_level(rfd::MessageLevel::Info).set_title("Nocturne Setup").set_description(message).show();
}

fn fail(message: &str) -> ! {
    let _ = rfd::MessageDialog::new().set_title("Nocturne setup").set_description(message).show();
    eprintln!("{message}");
    std::process::exit(1)
}
