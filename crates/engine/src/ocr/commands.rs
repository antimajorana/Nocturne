//! The commands for equation recognition: the switch, the settings, and one conversion.

use super::{MATHPIX_APP_ID_SECRET, OcrConfig, OcrMode, OcrProvider, endpoint_is_safe, key_matches_endpoint, local_model, recognize};
use crate::commands::{parse_opt_bool, req, switch_param};
use crate::{EngineError, Output, Params, Session};
use serde_json::json;

pub(crate) fn toggle(s: &mut Session, p: &Params) -> Result<Output, EngineError> {
    let enabled = switch_param(p, "enabled", s.ocr_config().enabled)?;
    if !s.set_math_ocr_enabled(enabled) {
        return Err(EngineError::BadParam("the settings file could not be written".into()));
    }
    Ok(Output::Json(json!({ "math_ocr_enabled": enabled })))
}

pub(crate) fn toggle_handwriting(s: &mut Session, p: &Params) -> Result<Output, EngineError> {
    let enabled = switch_param(p, "enabled", s.ocr_config().handwriting_enabled)?;
    if !s.set_handwriting_ocr_enabled(enabled) {
        return Err(EngineError::BadParam("the settings file could not be written".into()));
    }
    Ok(Output::Json(json!({ "handwriting_ocr_enabled": enabled })))
}

fn status(s: &Session) -> serde_json::Value {
    let cfg = s.ocr_config();
    let secrets = s.sync_secrets();
    let native_available = s.native_ocr_backend().is_some_and(|b| b.is_supported());
    let model_installed = local_model::is_model_installed(&s.ocr_model_dir());
    let supported = match cfg.provider {
        OcrProvider::AppleVision => native_available,
        OcrProvider::LocalModel => local_model::is_supported_on_platform(),
        _ => s.sync_http().is_supported() && secrets.is_supported(),
    };
    json!({
        "math_ocr_enabled": cfg.enabled,
        "handwriting_ocr_enabled": cfg.handwriting_enabled,
        "supported": supported,
        "provider": cfg.provider.id(),
        "endpoint": cfg.effective_endpoint(),
        "model": cfg.effective_model(),
        "has_key": s.ocr_has_key(cfg.provider),
        "has_app_id": s.ocr_has_mathpix_app_id(),
        "ready": s.math_ocr_ready(),
        "handwriting_ready": s.handwriting_ocr_ready(),
        "model_installed": model_installed,
        "native_ocr_available": native_available,
    })
}

fn secret_error(e: crate::SyncError) -> EngineError {
    EngineError::BadParam(e.to_string())
}

pub(crate) fn configure(s: &mut Session, p: &Params) -> Result<Output, EngineError> {
    let was = s.ocr_config();
    let mut cfg = was.clone();
    let given = |key: &str| p.get(key).map(|v| v.trim().to_string());

    // Everything is checked before anything is kept, so a refused value changes nothing.
    if let Some(name) = given("provider").filter(|v| !v.is_empty()) {
        cfg.provider = OcrProvider::parse(&name).ok_or_else(|| {
            EngineError::BadParam("provider must be apple_vision, local_model, anthropic, gemini, openai, mathpix, or custom".into())
        })?;
        if cfg.provider != was.provider {
            // Another service starts at its own address with its own model.
            cfg.endpoint.clear();
            cfg.model.clear();
        }
    }
    if let Some(raw) = given("endpoint") {
        let address = raw.trim_end_matches('/');
        if address.is_empty() || address.eq_ignore_ascii_case("default") {
            cfg.endpoint.clear();
        } else if endpoint_is_safe(address) {
            cfg.endpoint = if address == cfg.provider.default_endpoint() { String::new() } else { address.to_string() };
        } else {
            return Err(EngineError::BadParam("endpoint must begin https://, or be http:// on 127.0.0.1 or localhost".into()));
        }
    }
    if let Some(model) = given("model") {
        if model.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '?' | '#' | '"' | '\\')) {
            return Err(EngineError::BadParam("model must be a plain model name".into()));
        }
        cfg.model = if model.eq_ignore_ascii_case("default") { String::new() } else { model };
    }
    let clear_key = parse_opt_bool(p, "clear_key")?.unwrap_or(false);
    let key = given("key").filter(|v| !v.is_empty());
    let app_id = given("app_id").filter(|v| !v.is_empty());

    let secrets = s.sync_secrets();
    if !cfg.provider.is_on_device() {
        let name = cfg.provider.secret_key();
        let bound_key = cfg.provider.key_endpoint_setting_key();
        let target_endpoint = cfg.effective_endpoint().to_string();
        // A key was given for one address. If the address differs from either the previous active
        // address or the address that key was originally saved for (even after switching away to
        // another provider and back), forget any secret that was not re-supplied for the new address.
        let moved = (cfg.provider == was.provider && target_endpoint != was.effective_endpoint())
            || !key_matches_endpoint(&s.settings, cfg.provider, &target_endpoint);

        if clear_key {
            secrets.delete_secret(&name).map_err(secret_error)?;
            if cfg.provider == OcrProvider::Mathpix {
                secrets.delete_secret(MATHPIX_APP_ID_SECRET).map_err(secret_error)?;
            }
            let _ = s.settings.set(&bound_key, "");
        } else {
            if moved {
                if key.is_none() {
                    secrets.delete_secret(&name).map_err(secret_error)?;
                }
                if cfg.provider == OcrProvider::Mathpix && app_id.is_none() {
                    secrets.delete_secret(MATHPIX_APP_ID_SECRET).map_err(secret_error)?;
                }
                if key.is_none() && (cfg.provider != OcrProvider::Mathpix || app_id.is_none()) {
                    let _ = s.settings.set(&bound_key, "");
                }
            }
            let mut stored_secret = false;
            if let Some(key) = &key {
                secrets.set_secret(&name, key).map_err(secret_error)?;
                stored_secret = true;
            }
            if cfg.provider == OcrProvider::Mathpix
                && let Some(app_id) = &app_id
            {
                secrets.set_secret(MATHPIX_APP_ID_SECRET, app_id).map_err(secret_error)?;
                stored_secret = true;
            }
            if stored_secret {
                s.settings.set(&bound_key, &target_endpoint)?;
            }
        }
    }
    if cfg != was {
        let current = s.ocr_config();
        OcrConfig { enabled: current.enabled, handwriting_enabled: current.handwriting_enabled, ..cfg }.save(&s.settings)?;
    }
    Ok(Output::Json(status(s)))
}

pub(crate) fn remove_model(s: &mut Session, _p: &Params) -> Result<Output, EngineError> {
    let dir = s.ocr_model_dir();
    local_model::remove_model(&dir).map_err(|e| EngineError::BadParam(e.to_string()))?;
    Ok(Output::Json(json!({ "removed": true, "model_installed": local_model::is_model_installed(&dir) })))
}

pub(crate) fn convert(s: &mut Session, p: &Params) -> Result<Output, EngineError> {
    let bad = |m: String| EngineError::BadParam(m);
    let mode = match p.get("mode").map(|v| v.trim()).filter(|v| !v.is_empty()) {
        None => OcrMode::MathLatex,
        Some(raw) => OcrMode::parse(raw).ok_or_else(|| bad("mode must be math or handwriting".into()))?,
    };
    let cfg = s.ocr_config();
    let enabled = match mode {
        OcrMode::MathLatex => cfg.enabled,
        OcrMode::HandwritingText => cfg.handwriting_enabled,
    };
    if !enabled {
        return Err(bad(super::OcrError::Disabled.to_string()));
    }
    let page = s.resolve(req(p, "path")?)?;
    let no_ink = || EngineError::BadParam("that page has no ink".into());
    let text = std::fs::read_to_string(page.join("ink.svg")).map_err(|_| no_ink())?;
    let (drawing, _) = nocturne_ink::svg::read(&text).map_err(|_| no_ink())?;
    if drawing.strokes.is_empty() {
        return Err(no_ink());
    }
    let strokes = match p.get("strokes").map(|v| v.trim()).filter(|v| !v.is_empty()) {
        None => drawing.strokes,
        Some(list) => {
            let mut chosen = Vec::new();
            for part in list.split(',') {
                let at: usize = part.trim().parse().map_err(|_| bad("strokes must be whole numbers, comma separated".into()))?;
                let stroke = drawing.strokes.get(at).ok_or_else(|| bad(format!("there is no stroke {at} on that page")))?;
                chosen.push(stroke.clone());
            }
            chosen
        }
    };
    let job = s.start_ocr_job(&strokes, mode).map_err(|e| bad(e.to_string()))?;
    let result = recognize(&*s.sync_http(), &job).map_err(|e| bad(e.to_string()))?;
    match mode {
        OcrMode::MathLatex => Ok(Output::Json(json!({ "latex": result }))),
        OcrMode::HandwritingText => Ok(Output::Json(json!({ "text": result }))),
    }
}
