//! Non-blocking background kDrive sync job and Cloud Sync configuration modal.

use crate::theme::Tokens;
use egui::{Context, RichText, vec2};
use nocturne_engine::{
    DEFAULT_KDRIVE_ENDPOINT, DEFAULT_KDRIVE_REMOTE_PATH, KDRIVE_TOKEN_KEY, SecretStore, Session, StagedSync, SyncConfig, SyncError, SyncHttpClient,
    SyncReport, stage_sync,
};
use nocturne_store::Settings;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub struct SyncJob {
    rx: Receiver<Result<StagedSync, SyncError>>,
    cancel: Arc<AtomicBool>,
}

impl SyncJob {
    pub fn start(
        ctx: Context,
        local_root: PathBuf,
        settings: Settings,
        config: SyncConfig,
        http: Arc<dyn SyncHttpClient>,
        secrets: Arc<dyn SecretStore>,
    ) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_bg = Arc::clone(&cancel);
        std::thread::Builder::new()
            .name("nocturne-kdrive-sync".into())
            .spawn(move || {
                let res = stage_sync(&local_root, &settings, &config, http.as_ref(), secrets.as_ref(), Some(&cancel_bg));
                let _ = tx.send(res);
                ctx.request_repaint();
            })
            .map_err(|e| format!("could not start sync worker thread: {e}"))?;
        Ok(Self { rx, cancel })
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn try_recv(&self) -> Option<Result<StagedSync, SyncError>> {
        match self.rx.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(SyncError::Transport("sync worker thread disconnected".into()))),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CloudSyncForm {
    pub token_input: String,
    pub has_saved_token: bool,
    pub drive_id_input: String,
    pub remote_path: String,
    pub endpoint: String,
    pub auto_sync: bool,
    pub markdown_default: bool,
    pub propagate_deletes: bool,
    pub last_message: Option<(String, bool)>,
}

impl CloudSyncForm {
    pub fn from_session(session: &Session) -> Self {
        let cfg = SyncConfig::load(&session.settings);
        let has_saved_token = session.sync_secrets().get_secret(KDRIVE_TOKEN_KEY).ok().flatten().is_some();
        let (_, summary) = SyncConfig::last_sync_info(&session.settings);
        Self {
            token_input: String::new(),
            has_saved_token,
            drive_id_input: cfg.drive_id.map(|id| id.to_string()).unwrap_or_default(),
            remote_path: cfg.remote_path,
            endpoint: cfg.endpoint,
            auto_sync: cfg.auto_sync,
            markdown_default: cfg.markdown_default,
            propagate_deletes: cfg.propagate_deletes,
            last_message: summary.map(|s| (s, false)),
        }
    }
}

#[derive(Default)]
pub struct CloudSyncDialogOut {
    pub save_only: bool,
    pub save_and_sync_now: bool,
    pub clear_token: bool,
    pub convert_existing_to_markdown: bool,
    pub cancel_running: bool,
    pub close: bool,
}

pub fn format_sync_report_summary(report: &SyncReport) -> String {
    crate::i18n::format_message_id(
        "ui.kdrive_sync_finished_summary",
        &[
            ("up", &report.uploaded.len().to_string()),
            ("down", &report.downloaded.len().to_string()),
            ("conflicts", &report.conflicts.len().to_string()),
        ],
    )
}

pub fn show_sync_dialog(ctx: &Context, form: &mut CloudSyncForm, supported: bool, sync_busy: bool, tokens: &Tokens) -> CloudSyncDialogOut {
    let mut out = CloudSyncDialogOut::default();
    let mut open = true;
    egui::Window::new(crate::tl_id!("ui.kdrive_sync_title"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(470.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            if !supported {
                ui.label(RichText::new(crate::tl_id!("ui.kdrive_sync_unsupported_build")).color(tokens.warn));
                ui.add_space(8.0);
                if ui.button(crate::tl_id!("ui.close")).clicked() {
                    out.close = true;
                }
                return;
            }

            ui.label(RichText::new(crate::tl_id!("ui.kdrive_sync_intro")).small().color(tokens.muted));
            ui.add_space(8.0);

            ui.label(RichText::new(crate::tl_id!("ui.kdrive_token_label")).strong().color(tokens.text));
            let hint = if form.has_saved_token { crate::tl_id!("ui.kdrive_token_saved_hint") } else { crate::tl_id!("ui.kdrive_token_paste_hint") };
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut form.token_input).password(true).hint_text(hint).desired_width(330.0));
                if form.has_saved_token && ui.button(crate::tl_id!("ui.kdrive_clear_token")).clicked() {
                    out.clear_token = true;
                }
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(crate::tl_id!("ui.kdrive_remote_folder_label")).color(tokens.text));
                    ui.add(egui::TextEdit::singleline(&mut form.remote_path).hint_text(DEFAULT_KDRIVE_REMOTE_PATH).desired_width(275.0));
                });
                ui.vertical(|ui| {
                    ui.label(RichText::new(crate::tl_id!("ui.kdrive_drive_id_label")).color(tokens.text));
                    ui.add(
                        egui::TextEdit::singleline(&mut form.drive_id_input)
                            .hint_text(crate::tl_id!("ui.kdrive_drive_id_auto_hint"))
                            .desired_width(140.0),
                    );
                });
            });

            ui.collapsing(crate::tl_id!("ui.kdrive_advanced_section"), |ui| {
                ui.label(RichText::new(crate::tl_id!("ui.kdrive_endpoint_label")).small().color(tokens.muted));
                ui.add(egui::TextEdit::singleline(&mut form.endpoint).hint_text(DEFAULT_KDRIVE_ENDPOINT).desired_width(420.0));
                ui.checkbox(&mut form.propagate_deletes, crate::tl_id!("ui.kdrive_propagate_deletes_label"));
            });

            ui.add_space(6.0);
            ui.checkbox(&mut form.markdown_default, crate::tl_id!("ui.kdrive_markdown_default_label"));
            if ui.button(crate::tl_id!("ui.kdrive_convert_notebooks_md_btn")).clicked() {
                out.convert_existing_to_markdown = true;
            }
            ui.checkbox(&mut form.auto_sync, crate::tl_id!("ui.kdrive_auto_sync_label"));

            if let Some((ref msg, is_err)) = form.last_message {
                ui.add_space(6.0);
                ui.label(RichText::new(msg).small().color(if is_err { tokens.warn } else { tokens.muted }));
            }

            ui.add_space(10.0);
            ui.separator();
            ui.horizontal(|ui| {
                if sync_busy {
                    ui.label(RichText::new(crate::tl_id!("ui.kdrive_sync_in_progress")).color(tokens.accent));
                    if ui.button(crate::tl_id!("ui.cancel")).clicked() {
                        out.cancel_running = true;
                    }
                } else {
                    let sync_now = ui.add(
                        egui::Button::new(RichText::new(crate::tl_id!("ui.kdrive_sync_now_btn")).strong().color(tokens.text))
                            .min_size(vec2(110.0, 34.0)),
                    );
                    if sync_now.clicked() {
                        out.save_and_sync_now = true;
                    }
                    if ui.add(egui::Button::new(crate::tl_id!("ui.save")).min_size(vec2(80.0, 34.0))).clicked() {
                        out.save_only = true;
                    }
                }
                if ui.add(egui::Button::new(crate::tl_id!("ui.close")).min_size(vec2(80.0, 34.0))).clicked() {
                    out.close = true;
                }
            });
        });

    if !open {
        out.close = true;
    }
    out
}
