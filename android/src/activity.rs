//! The native activity: Android's entry point into Nocturne.

use crate::{insets, storage, welcome};
use nocturne_engine::Session;
use nocturne_store::Settings;
use nocturne_ui_egui::NocturneApp;
use std::ffi::CString;
use std::path::PathBuf;
use winit::platform::android::activity::AndroidApp;

/// Android's loader calls this, by name, on the activity's own thread once the APK's
/// `libnocturne_android.so` is loaded. The name must not be mangled; that attribute is the
/// only unsafe code in Nocturne (see android/Cargo.toml).
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("Nocturne"));
    std::panic::set_hook(Box::new(|info| log::error!("Nocturne stopped: {info}")));
    if let Err(error) = run(app) {
        log::error!("Nocturne could not start: {error}");
    }
}

fn run(app: AndroidApp) -> Result<(), String> {
    if let Ok(offset) = time::UtcOffset::current_local_offset() {
        nocturne_model::time::set_local_offset(offset.whole_seconds());
    }
    let layout = storage::layout(app.internal_data_path().as_deref(), app.external_data_path().as_deref())
        .ok_or("Android reported no internal storage for the app")?;
    log::info!("start: Nocturne {} notebooks={} settings={}", env!("CARGO_PKG_VERSION"), layout.notebooks.display(), layout.settings.display());
    let mut alerts: Vec<(&str, String)> = Vec::new();

    // The bundled Welcome notebook: unpacked into app storage once per version. A failure is
    // shown in the window; the app runs without it.
    let payload = layout.payload.clone();
    match CString::new(welcome::ASSET).ok().and_then(|name| app.asset_manager().open(&name)) {
        Some(asset) => match welcome::refresh(asset, &payload, env!("CARGO_PKG_VERSION")) {
            Ok(true) => log::info!("Welcome notebook payload unpacked to {}", payload.display()),
            Ok(false) => log::info!("Welcome notebook payload is current"),
            Err(error) => alerts.push(("welcome-payload", format!("The Welcome notebook bundled with Nocturne could not be unpacked: {error}"))),
        },
        None => alerts.push((
            "welcome-payload",
            format!("This build of Nocturne has no {} asset, so the Welcome notebook cannot be installed.", welcome::ASSET),
        )),
    }

    std::fs::create_dir_all(&layout.notebooks).map_err(|e| format!("cannot create {}: {e}", layout.notebooks.display()))?;
    let settings = Settings::at(&layout.settings);
    let root = nocturne_store::settings::configured_notebooks_dir(&settings).filter(|p| p.is_dir()).unwrap_or_else(|| layout.notebooks.clone());
    let session = Session::new(&root, settings).map_err(|e| format!("cannot open the notebooks folder {}: {e}", root.display()))?;

    // arboard has no Android backend, so this says the clipboard is not connected and the
    // window shows that once; copy and paste inside Nocturne still work.
    let clipboard: Box<dyn nocturne_ui_egui::clipboard::Board> = match nocturne_ui_egui::clipboard::open() {
        Ok(board) => board,
        Err(error) => {
            log::info!("clipboard: {error}");
            Box::new(nocturne_ui_egui::clipboard::Unavailable)
        }
    };

    let window = app.clone();
    let options = eframe::NativeOptions { android_app: Some(app), ..Default::default() };
    eframe::run_native(
        "Nocturne",
        options,
        Box::new(move |_cc| {
            let setup_payload: PathBuf = payload.clone();
            let mut app = NocturneApp::new(session)
                .with_welcome_notebook_setup(move |notebooks_root, format| {
                    nocturne_setup::install_edition(&setup_payload, notebooks_root, format).map_err(|error| error.to_string())
                })
                .with_clipboard(clipboard);
            for (key, message) in alerts.drain(..) {
                app.alert(key, "Nocturne", &message);
            }
            log::info!("window ready");
            Ok(Box::new(Shell { app, focused: true, window }))
        }),
    )
    .map_err(|e| format!("the window failed: {e}"))
}

/// The egui app inside Android's window.
struct Shell {
    app: NocturneApp,
    /// Whether the activity had the input focus last frame.
    focused: bool,
    /// The activity, asked each frame where the system bars are.
    window: AndroidApp,
}

impl eframe::App for Shell {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // The status bar and the navigation bar lie over the window's surface: what Android
        // leaves free between them becomes egui's content area, so nothing is drawn under them.
        let pixels_per_point = raw_input.viewport().native_pixels_per_point.unwrap_or_else(|| ctx.pixels_per_point());
        let window = raw_input.screen_rect.map(|r| r.size() * pixels_per_point);
        let content = self.window.content_rect();
        if let Some(size) = window
            && let Some(free) = insets::safe_area((content.left, content.top, content.right, content.bottom), size.x, size.y, pixels_per_point)
        {
            let margin = egui::epaint::MarginF32 { left: free.left, right: free.right, top: free.top, bottom: free.bottom };
            raw_input.safe_area_insets = Some(egui::SafeAreaInsets(margin));
        }
        // Android's Back button: it closes what is open over the page (a dialog, the
        // notebooks drawer, the File screen) before it does anything else.
        let mut back = false;
        raw_input.events.retain(|event| {
            let is_back = matches!(event, egui::Event::Key { key: egui::Key::BrowserBack, .. });
            if is_back && matches!(event, egui::Event::Key { pressed: true, .. }) {
                back = true;
            }
            !is_back
        });
        if back {
            self.app.system_back();
        }
        raw_input.events.extend(self.app.take_events());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Android may stop the app at any time once it leaves the screen: losing the focus
        // writes the page now rather than when the autosave would.
        let focused = ctx.input(|i| i.focused);
        if self.focused && !focused {
            self.app.save_page_now();
        }
        self.focused = focused;
        self.app.show(ui);
        // The page editor reads keys itself rather than through a TextEdit, so it asks for the
        // soft keyboard here while a block has the caret. A TextEdit asks on its own.
        if let Some(cursor) = self.app.keyboard_caret_rect() {
            ctx.output_mut(|o| {
                if o.ime.is_none() {
                    o.ime = Some(egui::output::IMEOutput {
                        purpose: egui::viewport::IMEPurpose::Normal,
                        rect: cursor,
                        cursor_rect: cursor,
                        should_interrupt_composition: false,
                    });
                }
            });
        }
    }

    fn on_exit(&mut self) {
        self.app.save_page_now();
    }
}
