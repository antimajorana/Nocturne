//! The commands for equation recognition: the switch, the settings, and one conversion.

use super::{MATHPIX_APP_ID_SECRET, OcrConfig, OcrProvider, endpoint_is_safe, recognize};
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

fn status(s: &Session) -> serde_json::Value {
    let cfg = s.ocr_config();
    let secrets = s.sync_secrets();
    let has = |name: &str| secrets.get_secret(name).map(|v| v.is_some()).unwrap_or(false);
    json!({
        "math_ocr_enabled": cfg.enabled,
        "supported": s.sync_http().is_supported() && secrets.is_supported(),
        "provider": cfg.provider.id(),
        "endpoint": cfg.effective_endpoint(),
        "model": cfg.effective_model(),
        "has_key": has(&cfg.provider.secret_key()),
        "has_app_id": has(MATHPIX_APP_ID_SECRET),
        "ready": s.math_ocr_ready(),
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
        cfg.provider = OcrProvider::parse(&name)
            .ok_or_else(|| EngineError::BadParam("provider must be anthropic, gemini, openai, mathpix, or custom".into()))?;
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
    let name = cfg.provider.secret_key();
    // A key was given for one address. If the address moves and no key comes with it, the
    // key is forgotten, so it cannot be sent somewhere it was not meant for.
    let moved = cfg.provider == was.provider && cfg.effective_endpoint() != was.effective_endpoint();
    if clear_key || (moved && key.is_none()) {
        secrets.delete_secret(&name).map_err(secret_error)?;
        if cfg.provider == OcrProvider::Mathpix && app_id.is_none() {
            secrets.delete_secret(MATHPIX_APP_ID_SECRET).map_err(secret_error)?;
        }
    }
    if !clear_key && let Some(key) = &key {
        secrets.set_secret(&name, key).map_err(secret_error)?;
    }
    if let Some(app_id) = &app_id {
        secrets.set_secret(MATHPIX_APP_ID_SECRET, app_id).map_err(secret_error)?;
    }
    if cfg != was {
        OcrConfig { enabled: s.ocr_config().enabled, ..cfg }.save(&s.settings)?;
    }
    Ok(Output::Json(status(s)))
}

pub(crate) fn convert(s: &mut Session, p: &Params) -> Result<Output, EngineError> {
    let bad = |m: String| EngineError::BadParam(m);
    if !s.ocr_config().enabled {
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
    let job = s.math_ocr_job(&strokes).map_err(|e| bad(e.to_string()))?;
    let latex = recognize(&*s.sync_http(), &job).map_err(|e| bad(e.to_string()))?;
    Ok(Output::Json(json!({ "latex": latex })))
}
