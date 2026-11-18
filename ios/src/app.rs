//! Native iOS / iPadOS application entry and `eframe::App` shell for Nocturne.

use crate::{clipboard, diagnostics, edit_menu, gestures, inbox, keyboard, pencil, storage, text_input};
use nocturne_engine::Session;
use nocturne_store::Settings;
use nocturne_ui_egui::clipboard::Board as _;
use nocturne_ui_egui::touch_ux::{TouchLongPress, apply_mobile_touch_input_options, apply_mobile_touch_style};
use nocturne_ui_egui::{NocturneApp, PlatformEditAction};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_foundation::{NSBundle, NSHomeDirectory};
use objc2_ui_kit::UIView;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::collections::{BTreeSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const WELCOME_MISSING_MESSAGE: &str = "This build of Nocturne has no bundled Welcome notebook payload, so the Welcome notebook cannot be installed.";

static LOG_TARGET: OnceLock<Mutex<LogTarget>> = OnceLock::new();
static PANIC_HOOK_ONCE: OnceLock<()> = OnceLock::new();
static FILE_LOGGER: FileLogger = FileLogger;

struct LogTarget {
    active: PathBuf,
    backup: PathBuf,
    notebooks: PathBuf,
}

struct FileLogger;

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            append_log_line(record.level().as_str(), &format!("{}: {}", record.target(), record.args()));
        }
    }

    fn flush(&self) {}
}

pub(crate) fn export_current_diagnostics_log() {
    let target = {
        let Some(lock) = LOG_TARGET.get() else { return };
        let guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        (guard.active.clone(), guard.notebooks.clone())
    };
    if let Err(err) = diagnostics::export_diagnostics_log(&target.0, &target.1) {
        log::warn!("diagnostics export failed: {err}");
    }
}

fn init_logging(app_data: &Path, notebooks: PathBuf) {
    let target = LogTarget { active: app_data.join("nocturne.log"), backup: app_data.join("nocturne.log.old"), notebooks };
    if let Some(lock) = LOG_TARGET.get() {
        let mut guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = target;
    } else {
        let _ = LOG_TARGET.set(Mutex::new(target));
    }
    rotate_if_oversized();
    if log::set_logger(&FILE_LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    PANIC_HOOK_ONCE.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let backtrace = std::backtrace::Backtrace::force_capture();
            append_log_line("PANIC", &format!("{info}\n{backtrace}"));
            export_current_diagnostics_log();
            previous(info);
        }));
    });
}

fn rotate_if_oversized() {
    let Some(lock) = LOG_TARGET.get() else { return };
    let guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    cap_log_file(&guard);
}

fn cap_log_file(target: &LogTarget) {
    if std::fs::metadata(&target.active).is_ok_and(|meta| meta.len() > MAX_LOG_BYTES) && std::fs::rename(&target.active, &target.backup).is_err() {
        let _ = std::fs::OpenOptions::new().create(true).write(true).truncate(true).open(&target.active);
    }
}

fn append_log_line(level: &str, text: &str) {
    let Some(lock) = LOG_TARGET.get() else { return };
    let guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    cap_log_file(&guard);
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&guard.active) {
        let _ = writeln!(file, "{} {level} {text}", utc_now());
    }
}

fn utc_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format_utc(secs)
}

fn format_utc(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Howard Hinnant's civil_from_days algorithm (days since 1970-01-01).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn sandbox_home_dir() -> Option<PathBuf> {
    let home = NSHomeDirectory().to_string();
    if !home.is_empty() {
        return Some(PathBuf::from(home));
    }
    std::env::var_os("HOME").filter(|value| !value.is_empty()).map(PathBuf::from)
}

fn bundle_resources_dir() -> Option<PathBuf> {
    let path = NSBundle::mainBundle().resourcePath()?.to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

fn apply_ingest_report(app: &mut NocturneApp, report: inbox::IngestReport) {
    if let Some(path) = report.last_opened {
        app.open_path(&path.to_string_lossy());
    }
    if !report.notices.is_empty() {
        for notice in &report.notices {
            log::info!("inbox: {notice}");
        }
        app.say(report.notices.join(" · "));
    }
}

fn native_options() -> eframe::NativeOptions {
    let mut options = eframe::NativeOptions::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(ref mut create_new) = options.wgpu_options.wgpu_setup {
        create_new.device_descriptor = Arc::new(|adapter| {
            let base_limits = if adapter.get_info().backend == eframe::wgpu::Backend::Gl {
                eframe::wgpu::Limits::downlevel_webgl2_defaults()
            } else {
                eframe::wgpu::Limits::default()
            };
            let desired = eframe::wgpu::Limits { max_texture_dimension_2d: 8192, ..base_limits };
            eframe::wgpu::DeviceDescriptor {
                label: Some("nocturne ios wgpu device"),
                required_limits: desired.or_worse_values_from(&adapter.limits()),
                ..Default::default()
            }
        });
    }
    options
}

fn extract_uikit_view(cc: &eframe::CreationContext<'_>) -> Option<(MainThreadMarker, Retained<UIView>)> {
    let mtm = MainThreadMarker::new()?;
    let handle = cc.window_handle().ok()?;
    let RawWindowHandle::UiKit(uikit) = handle.as_raw() else {
        return None;
    };
    #[allow(unsafe_code)]
    // SAFETY: `MainThreadMarker::new()` guarantees main-thread execution, and `eframe`'s
    // `CreationContext::window_handle` yields a live `WinitView` (`UIView` subclass) pointer
    // for the lifetime of the `winit` window; `Retained::retain` increments its retain count.
    let view = unsafe { Retained::retain(uikit.ui_view.as_ptr().cast::<UIView>()) }?;
    Some((mtm, view))
}

struct IosNativeBridge {
    view: Retained<UIView>,
    gestures: gestures::GestureBridge,
    pencil: pencil::ios::PencilBridge,
    text_input: text_input::ios::TextInputBridge,
    edit_menu: edit_menu::ios::EditMenuBridge,
}

impl IosNativeBridge {
    fn attach(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        let (mtm, view) = extract_uikit_view(cc)?;
        // eframe only runs a frame when winit delivers an event or a repaint is requested, and these
        // UIKit callbacks arrive without any winit event, so each bridge gets the context to wake it.
        let gestures = gestures::GestureBridge::attach(&view, mtm, &cc.egui_ctx);
        let pencil = pencil::ios::PencilBridge::attach(&view, mtm, &cc.egui_ctx);
        let text_input = text_input::ios::TextInputBridge::attach(&view, mtm, &cc.egui_ctx);
        let edit_menu = edit_menu::ios::EditMenuBridge::attach(&view, mtm, &cc.egui_ctx);
        Some(Self { view, gestures, pencil, text_input, edit_menu })
    }
}

fn map_edit_menu_action(action: edit_menu::EditMenuAction) -> PlatformEditAction {
    match action {
        edit_menu::EditMenuAction::Cut => PlatformEditAction::Cut,
        edit_menu::EditMenuAction::Copy => PlatformEditAction::Copy,
        edit_menu::EditMenuAction::Paste => PlatformEditAction::Paste,
        edit_menu::EditMenuAction::SelectAll => PlatformEditAction::SelectAll,
        edit_menu::EditMenuAction::Delete => PlatformEditAction::Delete,
    }
}

/// Initializes iOS sandbox directories, logging, inbox ingestion, and enters `eframe::run_native`.
pub(crate) fn run() -> Result<(), String> {
    if let Ok(offset) = time::UtcOffset::current_local_offset() {
        nocturne_model::time::set_local_offset(offset.whole_seconds());
    }
    let home = sandbox_home_dir();
    let resources = bundle_resources_dir();
    let layout =
        storage::layout(home.as_deref(), resources.as_deref()).ok_or_else(|| "iOS reported no sandbox home directory for Nocturne".to_string())?;

    std::fs::create_dir_all(&layout.app_data).map_err(|e| format!("cannot create {}: {e}", layout.app_data.display()))?;
    init_logging(&layout.app_data, layout.notebooks.clone());
    log::info!(
        "start: Nocturne {} notebooks={} inbox={} settings={}",
        env!("CARGO_PKG_VERSION"),
        layout.notebooks.display(),
        layout.inbox.display(),
        layout.settings.display()
    );

    std::fs::create_dir_all(&layout.notebooks).map_err(|e| format!("cannot create {}: {e}", layout.notebooks.display()))?;

    let settings = Settings::at(&layout.settings);
    let root = nocturne_store::settings::configured_notebooks_dir(&settings).filter(|p| p.is_dir()).unwrap_or_else(|| layout.notebooks.clone());
    let mut session = Session::new(&root, settings).map_err(|e| format!("cannot open the notebooks folder {}: {e}", root.display()))?;

    let initial_ingest = inbox::ingest_inbox(&layout.inbox, &mut session);
    let app_data = layout.app_data.clone();
    let inbox_dir = layout.inbox.clone();
    let welcome_payload = layout.welcome_payload.clone();
    let control_port = diagnostics::requested_control_port_from_env();

    eframe::run_native(
        "Nocturne",
        native_options(),
        Box::new(move |cc| {
            cc.egui_ctx.all_styles_mut(apply_mobile_touch_style);
            cc.egui_ctx.options_mut(apply_mobile_touch_input_options);
            let mut app = NocturneApp::new(session).with_clipboard(Box::new(clipboard::IosPasteboard::new()));
            apply_ingest_report(&mut app, initial_ingest);

            if let Some(welcome_dir) = welcome_payload {
                app = app.with_welcome_notebook_setup(move |notebooks_root, format| {
                    nocturne_setup::install_edition(&welcome_dir, notebooks_root, format).map_err(|error| error.to_string())
                });
            } else {
                log::warn!("bundled Welcome notebook payload missing from app bundle");
                app.alert("welcome-payload", "Nocturne", WELCOME_MISSING_MESSAGE);
            }

            if let Some(port) = control_port {
                match nocturne_ui_egui::control::start(port, &app_data, cc.egui_ctx.clone()) {
                    Ok((server, rx)) => {
                        log::info!("iOS control channel on 127.0.0.1:{}; key in {}", server.port, server.key_file.display());
                        app = app.with_control(server, rx);
                    }
                    Err(err) => log::warn!("iOS control channel failed to start: {err}"),
                }
            }

            let keyboard = keyboard::KeyboardObserver::new();
            let bridge = IosNativeBridge::attach(cc);
            if bridge.is_none() {
                log::warn!("UIKit view handle unavailable; native gesture, Pencil, text input, and edit menu bridges disabled");
            }
            log::info!("window ready");
            export_current_diagnostics_log();
            Ok(Box::new(Shell {
                app,
                focused: true,
                inbox: inbox_dir,
                keyboard,
                bridge,
                long_press: TouchLongPress::default(),
                egui_wants_ime: false,
                egui_ime_preedit: false,
                pending_text_commands: VecDeque::new(),
                move_cursor_focus_wait: 0,
                active_stylus_touches: BTreeSet::new(),
                last_geometry: None,
            }))
        }),
    )
    .map_err(|e| format!("the window failed: {e}"))
}

/// Snapshot of the window and layout geometry fields logged by `Shell`, so each change logs once.
#[derive(Clone, Copy, PartialEq, Eq)]
struct GeometrySnapshot {
    layout_bits: [u32; 4],
    safe_window_bits: Option<[u32; 4]>,
    full_window_bits: Option<[u32; 4]>,
    native_ppp_bits: Option<u32>,
    zoom_bits: u32,
}

fn rect_bits(rect: egui::Rect) -> [u32; 4] {
    [rect.min.x.to_bits(), rect.min.y.to_bits(), rect.max.x.to_bits(), rect.max.y.to_bits()]
}

/// The egui application hosted inside the iPadOS / iOS window.
struct Shell {
    app: NocturneApp,
    focused: bool,
    inbox: PathBuf,
    keyboard: Option<keyboard::KeyboardObserver>,
    bridge: Option<IosNativeBridge>,
    long_press: TouchLongPress,
    egui_wants_ime: bool,
    egui_ime_preedit: bool,
    pending_text_commands: VecDeque<text_input::TextEditCommand>,
    move_cursor_focus_wait: u8,
    active_stylus_touches: BTreeSet<egui::TouchId>,
    last_geometry: Option<GeometrySnapshot>,
}

impl Shell {
    fn prepare_stylus_ids_for_frame(&mut self, events: &[egui::Event]) -> BTreeSet<egui::TouchId> {
        for ev in events {
            if let egui::Event::Touch { id, phase: egui::TouchPhase::Start | egui::TouchPhase::Move, force, .. } = ev
                && force.is_some_and(f32::is_finite)
            {
                self.active_stylus_touches.insert(*id);
            }
        }
        let frame_stylus_ids = self.active_stylus_touches.clone();
        for ev in events {
            if let egui::Event::Touch { id, phase: egui::TouchPhase::End | egui::TouchPhase::Cancel, .. } = ev {
                self.active_stylus_touches.remove(id);
            }
        }
        frame_stylus_ids
    }
}

impl eframe::App for Shell {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // `egui-winit 0.37` populates iOS `safe_area_insets` and Apple Pencil `Touch { force }`.
        let raw_safe_bottom = raw_input.safe_area_insets.map_or(0.0, |insets| insets.0.bottom);
        if let Some(kb) = self.keyboard.as_ref()
            && let Some(screen_h) = raw_input.screen_rect.map(|r| r.height()).or_else(keyboard::current_screen_height_pts)
        {
            let (visible, inset_bottom) = kb.soft_keyboard_state(screen_h, raw_safe_bottom);
            self.app.set_soft_keyboard_visible(visible, inset_bottom);
            kb.apply_to_raw_input(screen_h, raw_input);
        }

        let frame_stylus_ids = self.prepare_stylus_ids_for_frame(&raw_input.events);
        let pointer_was_down = ctx.input(|i| i.pointer.primary_down());

        if self.app.keyboard_caret_rect().is_some() || self.egui_wants_ime {
            let _ = text_input::lock_arrow_focus_if_ready(ctx);
        }

        if let Some(bridge) = self.bridge.as_ref() {
            for _ in 0..bridge.pencil.take_toggle_eraser_count() {
                self.app.toggle_eraser_tool();
            }

            self.pending_text_commands.extend(bridge.text_input.drain_commands());
            if text_input::dispatch_pending_text_commands_with_context(
                Some(ctx),
                &mut self.move_cursor_focus_wait,
                &mut self.pending_text_commands,
                self.egui_wants_ime,
                &mut self.egui_ime_preedit,
                &mut self.app,
                raw_input,
                || clipboard::IosPasteboard::new().text(),
            ) {
                ctx.request_repaint();
            }

            for action in bridge.edit_menu.drain_actions() {
                let mapped = map_edit_menu_action(action);
                if self.egui_wants_ime {
                    let clip = (mapped == PlatformEditAction::Paste).then(|| clipboard::IosPasteboard::new().text()).flatten();
                    text_input::push_edit_action_to_egui(&mut raw_input.events, mapped, clip);
                } else {
                    self.app.apply_platform_edit_action(mapped);
                }
            }

            let touch_pos = gestures::latest_pointer_pos_in_events(&raw_input.events);
            let egui_blocked = gestures::egui_overlay_blocks_native_gesture(ctx, self.app.show_source(), touch_pos);
            bridge.gestures.sync_from_app(self.app.is_finger_drawing(), self.app.page_rect(), egui_blocked);
            if let Some(delta) = bridge.gestures.drain_pan_delta() {
                self.app.apply_external_pan(delta);
            }
            if let Some((scale, focal)) = bridge.gestures.drain_pinch() {
                self.app.apply_external_pinch_zoom(scale, focal);
            }
            bridge.gestures.filter_raw_input(raw_input, pointer_was_down, &frame_stylus_ids);
        }

        let mut keyboard_paste = false;
        if !self.egui_wants_ime {
            raw_input.events.retain(|event| {
                let is_paste = matches!(
                    event,
                    egui::Event::Key { key: egui::Key::V, pressed: true, modifiers, .. }
                        if modifiers.ctrl || modifiers.command
                ) || matches!(event, egui::Event::PasteImage(_));
                keyboard_paste |= is_paste;
                !is_paste
            });
            if keyboard_paste {
                log::debug!("raw keyboard paste captured before egui clipboard handling");
                self.app.queue_keyboard_paste();
            }
        }

        let now_secs = raw_input.time.unwrap_or_else(|| ctx.input(|i| i.time));
        if let Some(long_press_pos) = self.long_press.update(now_secs, &raw_input.events) {
            let caret_rect = self.app.keyboard_caret_rect();
            let page_rect = self.app.page_rect();
            let caret_arr = caret_rect.map(|r| [r.min.x, r.min.y, r.width(), r.height()]);
            let page_arr = page_rect.map(|r| [r.min.x, r.min.y, r.width(), r.height()]);
            let near_caret = edit_menu::is_long_press_near_caret([long_press_pos.x, long_press_pos.y], caret_arr, page_arr);
            let is_writable = caret_rect.is_some() && !self.app.show_source();
            if let Some(bridge) = self.bridge.as_ref()
                && edit_menu::should_present_native_edit_menu(caret_rect.is_some() && !self.egui_wants_ime && near_caret, true)
            {
                let target_arr = edit_menu::compute_menu_target_rect(caret_arr, Some([long_press_pos.x, long_press_pos.y]));
                let target_rect = egui::Rect::from_min_size(egui::pos2(target_arr[0], target_arr[1]), egui::vec2(target_arr[2], target_arr[3]));
                let has_selection = !self.app.selected_text_for_platform().is_empty();
                let can_paste = is_writable && {
                    let mut board = clipboard::IosPasteboard::new();
                    board.text().is_some() || board.has_picture() || board.html().is_some()
                };
                bridge.edit_menu.present(&bridge.view, target_rect, has_selection, can_paste, is_writable);
            } else {
                raw_input.events.push(egui::Event::PointerMoved(long_press_pos));
                raw_input.events.push(egui::Event::PointerButton {
                    pos: long_press_pos,
                    button: egui::PointerButton::Secondary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                });
                raw_input.events.push(egui::Event::PointerButton {
                    pos: long_press_pos,
                    button: egui::PointerButton::Secondary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            ctx.request_repaint();
        }
        if let Some(delay) = self.long_press.next_wakeup_delay(now_secs) {
            ctx.request_repaint_after(delay);
        }

        raw_input.events.extend(self.app.take_events());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let focused = ctx.input(|i| i.focused);
        let zoom = ctx.zoom_factor();
        let (layout, safe_window, full_window, native_ppp) = ctx.input(|i| {
            let viewport = i.viewport();
            (i.viewport_rect(), viewport.inner_rect, viewport.outer_rect, viewport.native_pixels_per_point)
        });
        let snapshot = GeometrySnapshot {
            layout_bits: rect_bits(layout),
            safe_window_bits: safe_window.map(rect_bits),
            full_window_bits: full_window.map(rect_bits),
            native_ppp_bits: native_ppp.map(f32::to_bits),
            zoom_bits: zoom.to_bits(),
        };
        if self.last_geometry != Some(snapshot) {
            self.last_geometry = Some(snapshot);
            log::info!(
                "geometry: layout={layout:?} safe_area_window={safe_window:?} full_window={full_window:?} native_ppp={native_ppp:?} zoom={zoom}"
            );
        }
        if self.focused && !focused {
            self.app.save_page_now();
            export_current_diagnostics_log();
        } else if !self.focused && focused {
            let report = inbox::ingest_inbox(&self.inbox, self.app.session_mut());
            apply_ingest_report(&mut self.app, report);
            export_current_diagnostics_log();
        }
        self.focused = focused;
        self.app.show(ui);

        // Consume `PlatformOutput.ime` so `egui-winit` never calls `WinitView.becomeFirstResponder()`
        // and steals focus from `NocturneTextInputView`.
        let ime_cursor_rect = ctx.output_mut(|o| o.ime.take().map(|ime| ime.cursor_rect));
        let caret_rect = self.app.keyboard_caret_rect();
        self.egui_wants_ime = text_input::should_route_ime_to_egui(ime_cursor_rect, caret_rect, self.app.page_rect(), &ctx);
        if !self.egui_wants_ime {
            self.egui_ime_preedit = false;
        } else {
            let copied_texts: Vec<String> = ctx.output(|o| {
                o.commands
                    .iter()
                    .filter_map(|cmd| match cmd {
                        egui::OutputCommand::CopyText(text) => Some(text.clone()),
                        _ => None,
                    })
                    .collect()
            });
            if !copied_texts.is_empty() {
                let mut board = clipboard::IosPasteboard::new();
                for text in copied_texts {
                    if let Err(err) = board.set_text(&text) {
                        log::warn!("failed to copy egui text to iOS pasteboard: {err}");
                    }
                }
            }
        }
        if let Some(bridge) = self.bridge.as_ref() {
            let should_focus = caret_rect.is_some() || self.egui_wants_ime;
            let effective_caret = if self.egui_wants_ime { ime_cursor_rect.or(caret_rect) } else { caret_rect };
            let selected_text = if self.egui_wants_ime { String::new() } else { self.app.selected_text_for_platform() };
            let pointer_is_write = caret_rect.is_some() || self.egui_wants_ime;
            let is_writable = should_focus;
            bridge.text_input.sync_state(should_focus, effective_caret, &selected_text, pointer_is_write, is_writable);
        }
    }

    fn on_exit(&mut self) {
        self.app.save_page_now();
        export_current_diagnostics_log();
    }
}
