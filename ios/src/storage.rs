//! Where Nocturne keeps its files on iPadOS / iOS.
//!
//! Inside an iOS app sandbox (`NSHomeDirectory()`):
//! - `Documents/Nocturne`: the user's notebooks, visible in the iPadOS Files app.
//! - `Documents/Inbox`: the staging drop-box where files opened via Open-in or the Share
//!   sheet arrive before `inbox::ingest_inbox` imports them.
//! - `Library/Application Support/Nocturne`: private app data (`settings.toml`, logs).
//! - `<bundle_resources>/WelcomeNotebook`: optional read-only Welcome notebook payload with
//!   `html/` and `cmgfm/` editions inside.

use std::path::{Path, PathBuf};

/// The folder name the notebooks live in (`Documents/Nocturne`).
pub const NOTEBOOKS_FOLDER: &str = "Nocturne";

/// The folder name iOS uses for incoming shared or opened files (`Documents/Inbox`).
pub const INBOX_FOLDER: &str = "Inbox";

/// The app's folders inside an iOS sandbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub settings: PathBuf,
    pub app_data: PathBuf,
    pub notebooks: PathBuf,
    pub inbox: PathBuf,
    pub welcome_payload: Option<PathBuf>,
}

/// Computes the iOS sandbox layout from `sandbox_home` and optional `bundle_resources`.
/// Returns `None` when `sandbox_home` is `None`.
pub fn layout(sandbox_home: Option<&Path>, bundle_resources: Option<&Path>) -> Option<Layout> {
    let home = sandbox_home?;
    let documents = home.join("Documents");
    let app_data = home.join("Library").join("Application Support").join(NOTEBOOKS_FOLDER);
    let settings = app_data.join("settings.toml");
    let notebooks = documents.join(NOTEBOOKS_FOLDER);
    let inbox = documents.join(INBOX_FOLDER);
    let welcome_payload = bundle_resources
        .map(|bundle| bundle.join("WelcomeNotebook"))
        .filter(|payload| payload.join("html").is_dir() && payload.join("cmgfm").is_dir());

    Some(Layout { settings, app_data, notebooks, inbox, welcome_payload })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_store::testing::TempDir;
    use std::fs;

    #[test]
    fn without_sandbox_home_there_is_no_layout() {
        let temp = TempDir::new("ios-storage-no-home");
        assert_eq!(layout(None, None), None);
        assert_eq!(layout(None, Some(temp.path())), None);
    }

    #[test]
    fn layout_places_notebooks_and_inbox_in_documents_and_settings_in_application_support() {
        let home = Path::new("/var/mobile/Containers/Data/Application/1234");
        let resolved = layout(Some(home), None).expect("layout");

        assert_eq!(resolved.notebooks, home.join("Documents").join("Nocturne"));
        assert_eq!(resolved.inbox, home.join("Documents").join("Inbox"));
        assert_eq!(resolved.app_data, home.join("Library").join("Application Support").join("Nocturne"));
        assert_eq!(resolved.settings, resolved.app_data.join("settings.toml"));
        assert_eq!(resolved.welcome_payload, None);
    }

    #[test]
    fn welcome_payload_requires_both_html_and_cmgfm_directories() {
        let temp = TempDir::new("ios-storage-welcome");
        let home = temp.path().join("sandbox");
        let resources = temp.path().join("Resources");
        fs::create_dir_all(&resources).expect("resources dir");

        assert_eq!(layout(Some(&home), Some(&resources)).expect("layout").welcome_payload, None);

        let welcome = resources.join("WelcomeNotebook");
        fs::create_dir_all(welcome.join("html")).expect("html dir");
        assert_eq!(layout(Some(&home), Some(&resources)).expect("layout").welcome_payload, None);

        // A regular file named `cmgfm` is not an edition directory.
        fs::write(welcome.join("cmgfm"), b"not a directory").expect("cmgfm file");
        assert_eq!(layout(Some(&home), Some(&resources)).expect("layout").welcome_payload, None);

        fs::remove_file(welcome.join("cmgfm")).expect("remove file");
        fs::create_dir_all(welcome.join("cmgfm")).expect("cmgfm dir");
        assert_eq!(layout(Some(&home), Some(&resources)).expect("layout").welcome_payload, Some(welcome));
    }
}
