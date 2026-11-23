//! What each service is sent, and where the LaTeX is in what it sends back.
//!
//! The shapes were read from each service's own reference in November 2026:
//! - Anthropic Messages: `POST {endpoint}/v1/messages`, `x-api-key`, `anthropic-version`.
//! - Gemini: `POST {endpoint}/v1beta/models/{model}:generateContent`, `x-goog-api-key`.
//! - OpenAI and compatible servers: `POST {endpoint}/chat/completions`, `Authorization: Bearer`.
//! - Mathpix: `POST {endpoint}/v3/text`, `app_id` and `app_key`.
//!
//! None of them has been called for real by the tests, which use a server on this machine.

use super::{OcrError, OcrJob, OcrMode, OcrProvider, base64, clean_latex};
use crate::sync::{SyncHttpMethod, SyncHttpRequest};
use serde_json::{Value, json};

const SYSTEM_PROMPT: &str = "Transcribe the handwritten mathematics in this picture as clean, KaTeX-compatible LaTeX. \
Use \\begin{aligned}...\\end{aligned} for multi-line derivations or systems of equations. \
Reply with bare LaTeX alone: no explanation, no $ or \\[ \\] delimiters, and no markdown code fence.";

const HANDWRITING_PROMPT: &str = "Transcribe the handwritten text in this picture faithfully as plain text. \
Preserve line breaks and paragraph breaks as written. \
Reply with the transcribed plain text only: no commentary, no preamble, and no markdown code fence.";

const ANTHROPIC_VERSION: &str = "2026-10-21";
const ANTHROPIC_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// The models whose refusals the service can hand to another model by itself.
const ANTHROPIC_WITH_FALLBACKS: [&str; 5] = ["claude-opus-5-5", "claude-opus-5", "claude-fable-5-2", "claude-fable-5-1", "claude-sonnet-5-5"];
/// Room for the model's thinking as well as a line of LaTeX.
const MAX_TOKENS: u32 = 8000;
/// The network client has its own limits; this is what is asked of one that honours it.
const TIMEOUT_SECS: u64 = 60;

fn prompt_for_mode(mode: OcrMode) -> &'static str {
    match mode {
        OcrMode::MathLatex => SYSTEM_PROMPT,
        OcrMode::HandwritingText => HANDWRITING_PROMPT,
    }
}

pub(super) fn request(job: &OcrJob) -> Result<SyncHttpRequest, OcrError> {
    let picture = base64(&job.png);
    let prompt = prompt_for_mode(job.mode);
    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    let mut header = |name: &str, value: &str| headers.push((name.to_string(), value.to_string()));
    let base = job.endpoint.trim_end_matches('/');
    let (url, body) = match job.provider {
        OcrProvider::AppleVision | OcrProvider::LocalModel => return Err(OcrError::Unsupported),
        OcrProvider::Anthropic => {
            header("x-api-key", &job.key);
            header("anthropic-version", ANTHROPIC_VERSION);
            let mut body = json!({
                "model": job.model,
                "max_tokens": MAX_TOKENS,
                "output_config": { "effort": "low" },
                "messages": [{ "role": "user", "content": [
                    { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": picture } },
                    { "type": "text", "text": prompt }
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
                { "text": prompt },
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
                    { "type": "text", "text": prompt },
                    { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{picture}"), "detail": "high" } }
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
            let formats = match job.mode {
                OcrMode::MathLatex => json!(["latex_styled", "text"]),
                OcrMode::HandwritingText => json!(["text"]),
            };
            let body = json!({ "src": format!("data:image/png;base64,{picture}"), "formats": formats });
            (format!("{base}/v3/text"), body)
        }
    };
    Ok(SyncHttpRequest { method: SyncHttpMethod::Post, url, headers, body: body.to_string().into_bytes(), timeout_secs: TIMEOUT_SECS })
}

fn text_of(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// Trims plain handwritten text while preserving line breaks and paragraph breaks.
/// Strips an outer fenced code block if a cloud model wrapped its reply despite instructions.
fn clean_plain_text(raw: &str) -> String {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut text = normalized.trim();
    if let Some(rest) = text.strip_prefix("```")
        && let Some((_, after_nl)) = rest.split_once('\n')
        && let Some(inner) = after_nl.trim_end().strip_suffix("```")
    {
        text = inner.trim();
    }
    text.to_string()
}

/// The LaTeX or handwritten text in a reply with a 2xx status, or why there is none.
pub(super) fn answer(provider: OcrProvider, mode: OcrMode, body: &[u8]) -> Result<String, OcrError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| OcrError::BadReply)?;
    let (text, refused, cut_off) = match provider {
        OcrProvider::AppleVision | OcrProvider::LocalModel => return Err(OcrError::Unsupported),
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
            // Skip thinking parts (`"thought": true`) so chain-of-thought never contaminates the output.
            let text: String = candidate["content"]["parts"]
                .as_array()
                .map(|parts| {
                    parts.iter().filter(|p| !p.get("thought").and_then(Value::as_bool).unwrap_or(false)).map(|p| text_of(&p["text"])).collect()
                })
                .unwrap_or_default();
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
            let err_info_id = text_of(&v["error_info"]["id"]);
            if err_info_id.starts_with("http_unauthorized") || err_info_id.starts_with("auth_") {
                return Err(OcrError::Auth);
            }
            if !text_of(&v["error"]).is_empty() || v.get("error_info").is_some_and(|e| !e.is_null()) {
                return Err(OcrError::BadReply);
            }
            let chosen = match mode {
                OcrMode::MathLatex => {
                    let styled = text_of(&v["latex_styled"]);
                    if clean_latex(styled).is_empty() { text_of(&v["text"]) } else { styled }
                }
                OcrMode::HandwritingText => {
                    let plain = text_of(&v["text"]);
                    if plain.trim().is_empty() { text_of(&v["latex_styled"]) } else { plain }
                }
            };
            (chosen.to_string(), false, false)
        }
    };
    if refused {
        return Err(OcrError::Refused);
    }
    let cleaned = match mode {
        OcrMode::MathLatex => clean_latex(&text),
        OcrMode::HandwritingText => clean_plain_text(&text),
    };
    match (cleaned.is_empty(), cut_off) {
        (false, _) => Ok(cleaned),
        (true, true) => Err(OcrError::Truncated),
        (true, false) => Err(OcrError::BadReply),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_job(provider: OcrProvider, mode: OcrMode) -> OcrJob {
        OcrJob {
            enabled: true,
            mode,
            provider,
            endpoint: "https://example.test/v1".to_string(),
            model: "test-model".to_string(),
            key: "test-key".to_string(),
            app_id: "app-id".to_string(),
            png: vec![0x89, b'P', b'N', b'G'],
            width: 100,
            height: 50,
            native_ocr: None,
            model_dir: None,
        }
    }

    #[test]
    fn openai_requests_high_detail_and_mode_specific_prompts() {
        let math_req = request(&sample_job(OcrProvider::OpenAi, OcrMode::MathLatex)).expect("math req");
        let math_body: Value = serde_json::from_slice(&math_req.body).expect("json");
        let content = &math_body["messages"][0]["content"];
        assert_eq!(content[1]["image_url"]["detail"], "high");
        assert!(content[0]["text"].as_str().is_some_and(|t| t.contains("KaTeX") && t.contains("\\begin{aligned}")));

        let hw_req = request(&sample_job(OcrProvider::OpenAi, OcrMode::HandwritingText)).expect("hw req");
        let hw_body: Value = serde_json::from_slice(&hw_req.body).expect("json");
        let hw_content = &hw_body["messages"][0]["content"];
        assert_eq!(hw_content[1]["image_url"]["detail"], "high");
        assert!(hw_content[0]["text"].as_str().is_some_and(|t| t.contains("plain text") && t.contains("paragraph breaks")));
    }

    #[test]
    fn handwriting_mode_preserves_multiline_text_and_paragraph_breaks() {
        let body = json!({
            "choices": [{
                "finish_reason": "stop",
                "message": { "content": "  Line one\r\nLine two\n\nSecond paragraph.  " }
            }]
        })
        .to_string();
        let text = answer(OcrProvider::OpenAi, OcrMode::HandwritingText, body.as_bytes()).expect("plain text");
        assert_eq!(text, "Line one\nLine two\n\nSecond paragraph.");
    }
}
