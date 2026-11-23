//! Provider setup body panels and unit tests for `math_ocr`.

use super::*;

pub(super) fn show_local_model_body(ui: &mut Ui, form: &Setup, download: Option<DownloadProgress>, t: &Tokens, outcome: &mut SetupOutcome) {
    ui.label(RichText::new(crate::tl_id!("ui.ocr_local_model_desc")).color(t.muted));
    ui.add_space(8.0);
    if !local_model::is_supported_on_platform() {
        ui.label(RichText::new(crate::tl_id!("ui.ocr_local_model_unsupported")).color(t.warn));
        return;
    }
    if let Some(prog) = download {
        let frac = if prog.total_bytes > 0 { (prog.bytes_downloaded as f32 / prog.total_bytes as f32).clamp(0.0, 1.0) } else { 0.0 };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let pct = (frac * 100.0).round().clamp(0.0, 100.0) as u32;
        #[allow(clippy::cast_precision_loss)]
        let done = format!("{:.1}", prog.bytes_downloaded as f64 / (1024.0 * 1024.0));
        #[allow(clippy::cast_precision_loss)]
        let total = format!("{:.1}", prog.total_bytes as f64 / (1024.0 * 1024.0));
        let text = crate::i18n::format_message_id("ui.ocr_model_downloading", &[("done", &done), ("total", &total), ("percent", &pct.to_string())]);
        ui.label(RichText::new(text).color(t.text));
        ui.add_space(4.0);
        progress_bar(ui, frac, t);
        ui.add_space(8.0);
        if modal::danger_button(ui, crate::tl_id!("ui.cancel"), t).clicked() {
            *outcome = SetupOutcome::CancelDownload;
        }
    } else if form.model_installed {
        ui.label(RichText::new(crate::tl_id!("ui.ocr_model_installed")).color(t.ok));
        ui.add_space(8.0);
        if modal::danger_button(ui, crate::tl_id!("ui.ocr_model_remove"), t).clicked() {
            *outcome = SetupOutcome::RemoveModel;
        }
    } else {
        ui.label(RichText::new(crate::tl_id!("ui.ocr_model_not_installed")).color(t.muted));
        ui.add_space(8.0);
        if modal::primary_button(ui, crate::tl_id!("ui.ocr_model_download"), t).clicked() {
            *outcome = SetupOutcome::DownloadModel;
        }
    }
}

pub(super) fn show_cloud_body(ui: &mut Ui, form: &mut Setup, t: &Tokens) {
    ui.label(RichText::new(crate::tl_id!("ui.recognition_sends_picture")).color(t.muted));
    ui.add_space(8.0);
    let w = (ui.available_width() - 108.0).max(120.0);
    let row = |ui: &mut Ui, lbl: &str, val: &mut String, id: &str, hint: &str, pw: bool| {
        ui.label(RichText::new(lbl).color(t.text));
        let mut edit = egui::TextEdit::singleline(val).id(Id::unique(id)).password(pw).desired_width(w);
        if !hint.is_empty() {
            edit = edit.hint_text(hint);
        }
        ui.add(edit);
        ui.end_row();
    };
    egui::Grid::new(Id::unique("math-ocr-setup-grid")).num_columns(2).spacing(vec2(8.0, 6.0)).show(ui, |ui| {
        row(ui, crate::tl_id!("ui.service_address"), &mut form.endpoint, "math-ocr-endpoint", form.provider.default_endpoint(), false);
        if form.provider != OcrProvider::Mathpix {
            row(ui, crate::tl_id!("ui.model_name"), &mut form.model, "math-ocr-model", form.provider.default_model(), false);
        } else {
            row(ui, crate::tl_id!("ui.app_id"), &mut form.app_id, "math-ocr-app-id", "", false);
        }
        row(ui, crate::tl_id!("ui.service_key"), &mut form.key, "math-ocr-key", "", true);
    });
    ui.add_space(6.0);
    let kept = form.kept();
    let (msg, col) = if form.provider == OcrProvider::Mathpix && !kept && form.has_stored_secret() {
        (crate::tl_id!("ui.ocr_mathpix_incomplete"), t.warn)
    } else {
        (if kept { crate::tl_id!("ui.key_is_kept") } else { crate::tl_id!("ui.no_key_kept") }, t.muted)
    };
    ui.label(RichText::new(msg).color(col));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mathpix_and_on_device_credentials_and_setup_render() {
        let mp = OcrProvider::ALL.iter().position(|p| *p == OcrProvider::Mathpix).expect("mathpix");
        let mut s = Setup { provider: OcrProvider::Mathpix, ..Setup::default() };
        assert!(!s.kept() && !s.has_stored_secret());
        s.has_key[mp] = true;
        assert!(!s.kept() && s.has_stored_secret());
        (s.has_key[mp], s.has_app_id) = (false, true);
        assert!(!s.kept() && s.has_stored_secret());
        s.has_key[mp] = true;
        assert!(s.kept() && s.has_stored_secret());
        s.provider = OcrProvider::AppleVision;
        assert!(s.kept() && !s.has_stored_secret() && !s.can_save(false));
        s.native_ocr_available = true;
        assert!(s.can_save(false));
        s.provider = OcrProvider::LocalModel;
        assert!(s.kept() && !s.has_stored_secret() && !s.can_save(false));
        s.model_installed = true;
        assert!(s.can_save(false) && !s.can_save(true));

        let t = crate::theme::Theme::DEFAULT.tokens();
        let prog = DownloadProgress {
            file_index: 0,
            file_count: 3,
            file_name: local_model::ENCODER_FILE,
            bytes_downloaded: 50_000_000,
            total_bytes: local_model::TOTAL_MODEL_BYTES,
        };
        let cases = [
            (OcrProvider::AppleVision, false, false, None),
            (OcrProvider::AppleVision, false, true, None),
            (OcrProvider::LocalModel, false, false, None),
            (OcrProvider::LocalModel, false, false, Some(prog)),
            (OcrProvider::LocalModel, true, false, None),
            (OcrProvider::Mathpix, false, false, None),
        ];
        for (provider, model_installed, native_ocr_available, dl) in cases {
            let mut form = Setup { provider, model_installed, native_ocr_available, error: Some("err".into()), ..Setup::default() };
            let ctx = Context::default();
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(360.0, 560.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| assert_eq!(show_setup(ui.ctx(), &mut form, dl, &t), SetupOutcome::Keep));
            out.textures_delta.clear();
        }
    }

    #[test]
    fn convert_buttons_and_preview_modes_render() {
        let t = crate::theme::Theme::DEFAULT.tokens();
        let view = Rect::from_min_size(egui::Pos2::ZERO, vec2(600.0, 480.0));
        let held = Rect::from_min_size(egui::pos2(120.0, 140.0), vec2(180.0, 80.0));
        for (math_on, text_on) in [(false, false), (true, false), (false, true), (true, true)] {
            let ctx = Context::default();
            let input = egui::RawInput { screen_rect: Some(view), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                assert_eq!(convert_buttons(ui.ctx(), held, view, math_on, text_on, &t), None);
            });
            out.textures_delta.clear();
        }
        for (mode, sample) in [(OcrMode::MathLatex, "x^2 + 1"), (OcrMode::HandwritingText, "Line one\nLine two")] {
            let mut conv = Conversion {
                page: PathBuf::from("page"),
                strokes: Vec::new(),
                bounds: [0.0; 4],
                mode,
                stage: Stage::Preview { tex: sample.to_string(), opened: false },
            };
            let ctx = Context::default();
            let input = egui::RawInput { screen_rect: Some(view), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                assert_eq!(show(ui.ctx(), &mut conv, &t), Outcome::Keep);
            });
            out.textures_delta.clear();
            if mode == OcrMode::HandwritingText
                && let Stage::Preview { tex, .. } = &conv.stage
            {
                assert!(tex.contains('\n'), "handwriting text preview must preserve newlines");
            }
        }
    }
}
