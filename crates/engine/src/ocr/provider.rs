//! What each service is sent, and where the LaTeX is in what it sends back.
//!
//! The shapes were read from each service's own reference in November 2026:
//! - Anthropic Messages: `POST {endpoint}/v1/messages`, `x-api-key`, `anthropic-version`.
//! - Gemini: `POST {endpoint}/v1beta/models/{model}:generateContent`, `x-goog-api-key`.
//! - OpenAI and compatible servers: `POST {endpoint}/chat/completions`, `Authorization: Bearer`.
//! - Mathpix: `POST {endpoint}/v3/text`, `app_id` and `app_key`.
//!
//! None of them has been called for real by the tests, which use a server on this machine.

use super::{OcrError, OcrJob, OcrProvider, base64, clean_latex};
use crate::sync::{SyncHttpMethod, SyncHttpRequest};
use serde_json::{Value, json};

const PROMPT: &str = "Transcribe the handwritten mathematics in this picture as LaTeX. \
Reply with the LaTeX alone, on one line: no explanation, no dollar signs, no code fence.";
const ANTHROPIC_VERSION: &str = "2026-10-21";
const ANTHROPIC_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// The models whose refusals the service can hand to another model by itself.
const ANTHROPIC_WITH_FALLBACKS: [&str; 5] = ["claude-opus-5-5", "claude-opus-5", "claude-fable-5-2", "claude-fable-5-1", "claude-sonnet-5-5"];
/// Room for the model's thinking as well as a line of LaTeX.
const MAX_TOKENS: u32 = 8000;
/// The network client has its own limits; this is what is asked of one that honours it.
const TIMEOUT_SECS: u64 = 60;

pub(super) fn request(job: &OcrJob) -> SyncHttpRequest {
    let picture = base64(&job.png);
    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    let mut header = |name: &str, value: &str| headers.push((name.to_string(), value.to_string()));
    let base = job.endpoint.trim_end_matches('/');
    let (url, body) = match job.provider {
        OcrProvider::Anthropic => {
            header("x-api-key", &job.key);
            header("anthropic-version", ANTHROPIC_VERSION);
            let mut body = json!({
                "model": job.model,
                "max_tokens": MAX_TOKENS,
                "output_config": { "effort": "low" },
                "messages": [{ "role": "user", "content": [
                    { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": picture } },
                    { "type": "text", "text": PROMPT }
                ]}]
            });
            if ANTHROPIC_WITH_FALLBACKS.contains(&job.model.as_str()) {
                header("anthropic-beta", ANTHROPIC_FALLBACK_BETA);
                body["fallbacks"] = json!("default");
            }
            (format!("{base}/v1/messages"), body)
        }
        OcrProvider::Gemini => {
            header("x-goog-api-key", &job.key);
            let body = json!({ "contents": [{ "parts": [
                { "text": PROMPT },
                { "inlineData": { "mimeType": "image/png", "data": picture } }
            ]}]});
            (format!("{base}/v1beta/models/{}:generateContent", job.model), body)
        }
        OcrProvider::OpenAi | OcrProvider::Custom => {
            if !job.key.is_empty() {
                header("Authorization", &format!("Bearer {}", job.key));
            }
            let mut body = json!({
                "model": job.model,
                "messages": [{ "role": "user", "content": [
                    { "type": "text", "text": PROMPT },
                    { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{picture}") } }
                ]}]
            });
            if job.provider == OcrProvider::OpenAi {
                body["max_completion_tokens"] = json!(MAX_TOKENS);
            }
            (format!("{base}/chat/completions"), body)
        }
        OcrProvider::Mathpix => {
            header("app_id", &job.app_id);
            header("app_key", &job.key);
            let body = json!({ "src": format!("data:image/png;base64,{picture}"), "formats": ["latex_styled", "text"] });
            (format!("{base}/v3/text"), body)
        }
    };
    SyncHttpRequest { method: SyncHttpMethod::Post, url, headers, body: body.to_string().into_bytes(), timeout_secs: TIMEOUT_SECS }
}

fn text_of(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// The LaTeX in a reply with a 2xx status, or why there is none.
pub(super) fn answer(provider: OcrProvider, body: &[u8]) -> Result<String, OcrError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| OcrError::BadReply)?;
    let (text, refused, cut_off) = match provider {
        OcrProvider::Anthropic => {
            // The first block may be the model's thinking; the answer is the first text block.
            let text = v["content"]
                .as_array()
                .and_then(|blocks| blocks.iter().find(|b| b["type"] == "text"))
                .map(|b| text_of(&b["text"]).to_string())
                .unwrap_or_default();
            (text, v["stop_reason"] == "refusal", v["stop_reason"] == "max_tokens")
        }
        OcrProvider::Gemini => {
            let candidate = &v["candidates"][0];
            let text: String =
                candidate["content"]["parts"].as_array().map(|parts| parts.iter().map(|p| text_of(&p["text"])).collect()).unwrap_or_default();
            let reason = text_of(&candidate["finishReason"]);
            let blocked = !v["promptFeedback"]["blockReason"].is_null()
                || matches!(
                    reason,
                    "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY" | "IMAGE_PROHIBITED_CONTENT"
                );
            (text, blocked, reason == "MAX_TOKENS")
        }
        OcrProvider::OpenAi | OcrProvider::Custom => {
            let choice = &v["choices"][0];
            let content = &choice["message"]["content"];
            // Some compatible servers answer in parts, as they are asked.
            let text = match content.as_array() {
                Some(parts) => parts.iter().map(|p| text_of(&p["text"])).collect(),
                None => text_of(content).to_string(),
            };
            let reason = text_of(&choice["finish_reason"]);
            let refused = !text_of(&choice["message"]["refusal"]).is_empty() || reason == "content_filter";
            (text, refused, reason == "length")
        }
        OcrProvider::Mathpix => {
            if !text_of(&v["error"]).is_empty() {
                return Err(OcrError::BadReply);
            }
            let styled = text_of(&v["latex_styled"]);
            (if styled.trim().is_empty() { text_of(&v["text"]) } else { styled }.to_string(), false, false)
        }
    };
    if refused {
        return Err(OcrError::Refused);
    }
    let latex = clean_latex(&text);
    match (latex.is_empty(), cut_off) {
        (false, _) => Ok(latex),
        (true, true) => Err(OcrError::Truncated),
        (true, false) => Err(OcrError::BadReply),
    }
}
