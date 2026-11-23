//! Handwritten mathematics to LaTeX: the switch, the settings, the picture that is sent, and
//! each service's request and reply, against a real HTTP server on the loopback address.

use nocturne_engine::ocr::{self, OcrError, OcrJob, OcrProvider};
use nocturne_engine::{EngineError, Output, Params, Session, base64, registry};
use nocturne_ink::{Dab, Drawing, Rgb, Stroke, Tool};
use nocturne_store::Settings;
use nocturne_store::testing::TempDir;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const KEY: &str = "sk-test-0123456789-not-a-real-key";

struct Seen {
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("the request body is JSON")
    }
}

/// A small HTTP server on 127.0.0.1 that keeps what it was sent and answers from a queue.
struct Server {
    endpoint: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    replies: Arc<Mutex<VecDeque<(u16, String)>>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        listener.set_nonblocking(true).expect("nonblocking");
        let endpoint = format!("http://127.0.0.1:{}", listener.local_addr().expect("addr").port());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let replies = Arc::new(Mutex::new(VecDeque::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen_bg, replies_bg, stop_bg) = (Arc::clone(&seen), Arc::clone(&replies), Arc::clone(&stop));
        let handle = thread::spawn(move || {
            while !stop_bg.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).expect("blocking stream");
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut first = String::new();
                reader.read_line(&mut first).expect("request line");
                let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
                let mut headers = Vec::new();
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).expect("header");
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = line.split_once(':') {
                        if k.eq_ignore_ascii_case("content-length") {
                            length = v.trim().parse().unwrap_or(0);
                        }
                        headers.push((k.trim().to_string(), v.trim().to_string()));
                    }
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).expect("body");
                seen_bg.lock().expect("seen").push(Seen { path, headers, body });
                let (status, text) = replies_bg.lock().expect("replies").pop_front().unwrap_or((500, "{}".to_string()));
                let head = format!(
                    "HTTP/1.1 {status} Reply\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    text.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(text.as_bytes());
                let _ = stream.flush();
            }
        });
        Server { endpoint, seen, replies, stop, handle: Some(handle) }
    }

    fn reply(&self, status: u16, body: Value) {
        self.replies.lock().expect("replies").push_back((status, body.to_string()));
    }

    fn count(&self) -> usize {
        self.seen.lock().expect("seen").len()
    }

    fn last(&self) -> Seen {
        self.seen.lock().expect("seen").pop().expect("a request arrived")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn session(t: &TempDir) -> Session {
    Session::new(&t.path().join("notebooks"), Settings::at(t.path().join("settings").join("s.toml"))).expect("session")
}

fn p(pairs: &[(&str, &str)]) -> Params {
    pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
}

fn json_of(out: Output) -> Value {
    match out {
        Output::Json(v) => v,
        other => panic!("expected Output::Json, got {other:?}"),
    }
}

/// A plus sign: two pen strokes crossing at (60, 40).
fn plus() -> Vec<Stroke> {
    let mut across = Stroke::new(Tool::Pen, Rgb::BLACK, 3.0);
    across.dabs = (0..=20).map(|i| Dab::new(40.0 + 2.0 * i as f32, 40.0, 1.0)).collect();
    let mut down = Stroke::new(Tool::Pen, Rgb::BLACK, 3.0);
    down.dabs = (0..=20).map(|i| Dab::new(60.0, 20.0 + 2.0 * i as f32, 1.0)).collect();
    vec![across, down]
}

/// A session with recognition on and `provider` pointed at `server`.
fn ready(t: &TempDir, server: &Server, provider: &str, extra: &[(&str, &str)]) -> Session {
    let mut s = session(t);
    s.run("ink.math_ocr_toggle", &p(&[("enabled", "true")])).expect("on");
    let mut pairs = vec![("provider", provider), ("endpoint", server.endpoint.as_str()), ("key", KEY)];
    pairs.extend_from_slice(extra);
    s.run("ink.math_ocr_configure", &p(&pairs)).expect("configure");
    s
}

fn recognise(s: &Session) -> Result<String, OcrError> {
    let job = s.math_ocr_job(&plus())?;
    ocr::recognize(&*s.sync_http(), &job)
}

#[test]
fn recognition_starts_off_and_while_off_nothing_is_sent() {
    let t = TempDir::new("math-ocr-off");
    let server = Server::start();
    let mut s = session(&t);
    assert!(!s.ocr_config().enabled, "off until the owner turns it on");
    let expected_default = if cfg!(any(target_os = "ios", target_os = "macos")) { OcrProvider::AppleVision } else { OcrProvider::Anthropic };
    assert_eq!(s.ocr_config().provider, expected_default);
    for id in ["ink.math_ocr_toggle", "ink.math_ocr_configure", "ink.math_ocr_convert"] {
        let spec = registry().iter().find(|c| c.id == id).unwrap_or_else(|| panic!("{id} is a command"));
        assert!(!spec.changes_tree, "{id} does not change the notebooks");
    }
    // Fully configured, but still off.
    s.run("ink.math_ocr_configure", &p(&[("provider", "anthropic"), ("endpoint", &server.endpoint), ("key", KEY)])).expect("configure");
    assert!(matches!(s.math_ocr_job(&plus()), Err(OcrError::Disabled)));
    // A job made while it was on is refused once it is off.
    s.run("ink.math_ocr_toggle", &p(&[("enabled", "true")])).expect("on");
    let mut job = s.math_ocr_job(&plus()).expect("job");
    job.enabled = false;
    assert!(matches!(ocr::recognize(&*s.sync_http(), &job), Err(OcrError::Disabled)));
    assert_eq!(server.count(), 0, "nothing left the machine");
}

#[test]
fn the_switch_is_set_toggled_refuses_junk_and_is_remembered() {
    let t = TempDir::new("math-ocr-toggle");
    let mut s = session(&t);
    assert_eq!(json_of(s.run("ink.math_ocr_toggle", &Params::new()).expect("toggle")), json!({ "math_ocr_enabled": true }));
    assert_eq!(json_of(s.run("ink.math_ocr_toggle", &p(&[("enabled", "false")])).expect("off")), json!({ "math_ocr_enabled": false }));
    assert!(matches!(s.run("ink.math_ocr_toggle", &p(&[("enabled", "perhaps")])), Err(EngineError::BadParam(_))));
    s.run("ink.math_ocr_toggle", &p(&[("enabled", "true")])).expect("on");
    drop(s);
    assert!(session(&t).ocr_config().enabled, "the next session remembers");
}

#[test]
fn settings_are_kept_keys_are_never_shown_and_only_safe_addresses_are_taken() {
    let t = TempDir::new("math-ocr-configure");
    let mut s = session(&t);
    let out = json_of(
        s.run("ink.math_ocr_configure", &p(&[("provider", " Gemini "), ("endpoint", "https://example.test/"), ("model", "m-1"), ("key", KEY)]))
            .expect("configure"),
    );
    assert_eq!(out["provider"], "gemini");
    assert_eq!(out["endpoint"], "https://example.test");
    assert_eq!(out["model"], "m-1");
    assert_eq!(out["has_key"], true);
    assert!(!out.to_string().contains(KEY), "the key is never in a reply");
    assert!(!std::fs::read_to_string(s.settings.path()).unwrap_or_default().contains(KEY), "nor in the settings file");

    for bad in ["http://example.test", "ftp://127.0.0.1", "http://127.0.0.1.example.test", "http://user@127.0.0.1:9", "example.test"] {
        assert!(matches!(s.run("ink.math_ocr_configure", &p(&[("endpoint", bad)])), Err(EngineError::BadParam(_))), "{bad} is refused");
    }
    assert!(matches!(s.run("ink.math_ocr_configure", &p(&[("provider", "abacus")])), Err(EngineError::BadParam(_))));
    assert_eq!(s.ocr_config().endpoint, "https://example.test", "a refused address changes nothing");

    // With no parameters it reports, and changes nothing.
    let status = json_of(s.run("ink.math_ocr_configure", &Params::new()).expect("status"));
    assert_eq!((status["provider"].as_str(), status["has_key"].as_bool()), (Some("gemini"), Some(true)));
    assert_eq!(json_of(s.run("ink.math_ocr_configure", &p(&[("clear_key", "true")])).expect("clear"))["has_key"], false);
}

#[test]
fn a_key_is_never_sent_to_an_address_or_service_it_was_not_given_for() {
    let t = TempDir::new("math-ocr-key-scope");
    let first = Server::start();
    let second = Server::start();
    let mut s = ready(&t, &first, "openai", &[]);

    // The address changes and no key comes with it: the old key is forgotten.
    let out = json_of(s.run("ink.math_ocr_configure", &p(&[("endpoint", &second.endpoint)])).expect("move"));
    assert_eq!(out["has_key"], false);
    assert!(matches!(recognise(&s), Err(OcrError::NotConfigured(_))));

    // Another service has its own key, and none yet.
    let mut s = ready(&t, &first, "openai", &[]);
    let out = json_of(s.run("ink.math_ocr_configure", &p(&[("provider", "anthropic"), ("endpoint", &second.endpoint)])).expect("switch"));
    assert_eq!(out["has_key"], false);
    assert!(matches!(recognise(&s), Err(OcrError::NotConfigured(_))));
    assert_eq!(first.count() + second.count(), 0, "no request, so no key, went anywhere");

    let job = ready(&t, &first, "openai", &[]).math_ocr_job(&plus()).expect("job");
    assert!(!format!("{job:?}").contains(KEY), "a job does not print its key");
}

#[test]
fn anthropic_gets_a_messages_request_and_its_text_block_is_the_answer() {
    let t = TempDir::new("math-ocr-anthropic");
    let server = Server::start();
    let s = ready(&t, &server, "anthropic", &[]);
    server.reply(
        200,
        json!({ "stop_reason": "end_turn", "content": [
            { "type": "thinking", "thinking": "" },
            { "type": "text", "text": "```latex\n$$ a + b $$\n```" }
        ]}),
    );
    let job = s.math_ocr_job(&plus()).expect("job");
    assert_eq!(ocr::recognize(&*s.sync_http(), &job).expect("latex"), "a + b");

    let seen = server.last();
    assert_eq!(seen.path, "/v1/messages");
    assert_eq!(seen.header("x-api-key"), Some(KEY));
    assert_eq!(seen.header("anthropic-version"), Some("2026-10-21"));
    assert_eq!(seen.header("anthropic-beta"), Some("server-side-fallback-2026-07-01"));
    let body = seen.json();
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["fallbacks"], "default");
    assert_eq!(body["output_config"]["effort"], "low");
    assert!(body["max_tokens"].as_u64().is_some_and(|n| n >= 4000), "room for thinking and the answer");
    for gone in ["temperature", "top_p", "top_k", "thinking"] {
        assert!(body.get(gone).is_none(), "{gone} is not sent");
    }
    let content = &body["messages"][0]["content"];
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(content[0]["type"], "image");
    assert_eq!(content[0]["source"], json!({ "type": "base64", "media_type": "image/png", "data": base64(&job.png) }));
    assert_eq!(content[1]["type"], "text");
    assert!(content[1]["text"].as_str().is_some_and(|text| text.contains("LaTeX")));

    // Another model is used as named, and fallbacks are only asked for where they exist.
    let mut s = s;
    s.run("ink.math_ocr_configure", &p(&[("model", "claude-haiku-5-5")])).expect("model");
    server.reply(200, json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": "x" }] }));
    assert_eq!(recognise(&s).expect("latex"), "x");
    let seen = server.last();
    assert_eq!(seen.json()["model"], "claude-haiku-5-5");
    assert!(seen.json().get("fallbacks").is_none() && seen.header("anthropic-beta").is_none());
}

#[test]
fn a_refusal_a_cut_off_reply_and_each_kind_of_failure_are_told_apart() {
    let t = TempDir::new("math-ocr-failures");
    let server = Server::start();
    let s = ready(&t, &server, "anthropic", &[]);

    server.reply(200, json!({ "stop_reason": "refusal", "stop_details": { "type": "refusal", "category": null }, "content": [] }));
    assert!(matches!(recognise(&s), Err(OcrError::Refused)));
    server.reply(200, json!({ "stop_reason": "max_tokens", "content": [{ "type": "thinking", "thinking": "" }] }));
    assert!(matches!(recognise(&s), Err(OcrError::Truncated)));
    server.reply(200, json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": "  " }] }));
    assert!(matches!(recognise(&s), Err(OcrError::BadReply)));

    let secret_echo = json!({ "error": { "type": "authentication_error", "message": format!("invalid x-api-key {KEY}") } });
    server.reply(401, secret_echo.clone());
    let err = recognise(&s).expect_err("a bad key");
    assert!(matches!(err, OcrError::Auth));
    assert!(!err.to_string().contains(KEY) && !format!("{err:?}").contains(KEY), "an error never repeats the key");
    server.reply(403, secret_echo);
    assert!(matches!(recognise(&s), Err(OcrError::Auth)));
    server.reply(429, json!({}));
    assert!(matches!(recognise(&s), Err(OcrError::RateLimited)));
    server.reply(529, json!({ "error": { "message": KEY } }));
    let err = recognise(&s).expect_err("overloaded");
    assert!(matches!(err, OcrError::Status(529)));
    assert!(!err.to_string().contains(KEY));

    // Nobody is listening at all.
    let dead = {
        let gone = Server::start();
        gone.endpoint.clone()
    };
    let mut s = s;
    s.run("ink.math_ocr_configure", &p(&[("endpoint", &dead), ("key", KEY)])).expect("move");
    let err = recognise(&s).expect_err("no server");
    assert!(matches!(err, OcrError::Transport(_)));
    assert!(!err.to_string().contains(KEY));
}

#[test]
fn gemini_gets_generate_content_with_the_picture_inline() {
    let t = TempDir::new("math-ocr-gemini");
    let server = Server::start();
    let s = ready(&t, &server, "gemini", &[]);
    server.reply(200, json!({ "candidates": [{ "finishReason": "STOP", "content": { "parts": [{ "text": "\\frac{1}" }, { "text": "{2}" }] } }] }));
    let job = s.math_ocr_job(&plus()).expect("job");
    assert_eq!(ocr::recognize(&*s.sync_http(), &job).expect("latex"), "\\frac{1}{2}");
    let seen = server.last();
    assert_eq!(seen.path, format!("/v1beta/models/{}:generateContent", OcrProvider::Gemini.default_model()));
    assert_eq!(seen.header("x-goog-api-key"), Some(KEY));
    assert!(!seen.path.contains(KEY), "the key is a header, not part of the address");
    let parts = &seen.json()["contents"][0]["parts"];
    assert!(parts[0]["text"].as_str().is_some_and(|text| text.contains("LaTeX")));
    assert_eq!(parts[1]["inlineData"], json!({ "mimeType": "image/png", "data": base64(&job.png) }));

    server.reply(200, json!({ "promptFeedback": { "blockReason": "SAFETY" } }));
    assert!(matches!(recognise(&s), Err(OcrError::Refused)));
    server.reply(200, json!({ "candidates": [{ "finishReason": "MAX_TOKENS", "content": { "parts": [] } }] }));
    assert!(matches!(recognise(&s), Err(OcrError::Truncated)));
    server.reply(200, json!({ "candidates": [{ "finishReason": "SAFETY" }] }));
    assert!(matches!(recognise(&s), Err(OcrError::Refused)));
}

#[test]
fn openai_gets_a_chat_completion_with_the_picture_as_a_data_address() {
    let t = TempDir::new("math-ocr-openai");
    let server = Server::start();
    let s = ready(&t, &server, "openai", &[]);
    server.reply(200, json!({ "choices": [{ "finish_reason": "stop", "message": { "content": "\\[ e^{i\\pi} = -1 \\]", "refusal": null } }] }));
    let job = s.math_ocr_job(&plus()).expect("job");
    assert_eq!(ocr::recognize(&*s.sync_http(), &job).expect("latex"), "e^{i\\pi} = -1");
    let seen = server.last();
    assert_eq!(seen.path, "/chat/completions", "the address given stands in for https://api.openai.com/v1");
    assert_eq!(OcrProvider::OpenAi.default_endpoint(), "https://api.openai.com/v1");
    assert_eq!(seen.header("authorization"), Some(format!("Bearer {KEY}").as_str()));
    let body = seen.json();
    assert_eq!(body["model"], OcrProvider::OpenAi.default_model());
    assert!(body["max_completion_tokens"].as_u64().is_some() && body.get("max_tokens").is_none());
    let content = &body["messages"][0]["content"];
    assert_eq!(content[0]["type"], "text");
    assert_eq!(
        content[1],
        json!({ "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{}", base64(&job.png)), "detail": "high" } })
    );

    server.reply(200, json!({ "choices": [{ "finish_reason": "stop", "message": { "content": null, "refusal": "I cannot help with that." } }] }));
    assert!(matches!(recognise(&s), Err(OcrError::Refused)));
    server.reply(200, json!({ "choices": [{ "finish_reason": "length", "message": { "content": "" } }] }));
    assert!(matches!(recognise(&s), Err(OcrError::Truncated)));
}

#[test]
fn a_custom_service_needs_its_address_and_model_and_may_have_no_key() {
    let t = TempDir::new("math-ocr-custom");
    let server = Server::start();
    let mut s = session(&t);
    s.run("ink.math_ocr_toggle", &p(&[("enabled", "true")])).expect("on");
    s.run("ink.math_ocr_configure", &p(&[("provider", "custom")])).expect("custom");
    assert!(matches!(s.math_ocr_job(&plus()), Err(OcrError::NotConfigured(_))), "no address yet");
    s.run("ink.math_ocr_configure", &p(&[("endpoint", &format!("{}/v1/", server.endpoint))])).expect("address");
    assert!(matches!(s.math_ocr_job(&plus()), Err(OcrError::NotConfigured(_))), "no model yet");
    s.run("ink.math_ocr_configure", &p(&[("model", "local-vision")])).expect("model");

    server.reply(200, json!({ "choices": [{ "message": { "content": "$x^2$" } }] }));
    assert_eq!(recognise(&s).expect("latex"), "x^2");
    let seen = server.last();
    assert_eq!(seen.path, "/v1/chat/completions");
    assert!(seen.header("authorization").is_none(), "no key, no header");
    assert_eq!(seen.json()["model"], "local-vision");
    assert!(seen.json().get("max_completion_tokens").is_none(), "older compatible servers do not know it");

    s.run("ink.math_ocr_configure", &p(&[("key", KEY)])).expect("key");
    server.reply(200, json!({ "choices": [{ "message": { "content": "y" } }] }));
    assert_eq!(recognise(&s).expect("latex"), "y");
    assert_eq!(server.last().header("authorization"), Some(format!("Bearer {KEY}").as_str()));
}

#[test]
fn mathpix_needs_an_app_id_as_well_and_answers_in_its_own_shape() {
    let t = TempDir::new("math-ocr-mathpix");
    let server = Server::start();
    let mut s = ready(&t, &server, "mathpix", &[]);
    assert!(matches!(s.math_ocr_job(&plus()), Err(OcrError::NotConfigured(_))), "an app id is needed too");
    let out = json_of(s.run("ink.math_ocr_configure", &p(&[("app_id", "nocturne_test_app")])).expect("app id"));
    assert!(!out.to_string().contains("nocturne_test_app"), "the app id is kept with the key, not shown");

    server.reply(200, json!({ "latex_styled": "\\int_0^1 x\\,dx", "text": "\\( \\int_0^1 x\\,dx \\)" }));
    let job = s.math_ocr_job(&plus()).expect("job");
    assert_eq!(ocr::recognize(&*s.sync_http(), &job).expect("latex"), "\\int_0^1 x\\,dx");
    let seen = server.last();
    assert_eq!(seen.path, "/v3/text");
    assert_eq!((seen.header("app_id"), seen.header("app_key")), (Some("nocturne_test_app"), Some(KEY)));
    assert_eq!(seen.json()["src"], format!("data:image/png;base64,{}", base64(&job.png)));

    server.reply(200, json!({ "text": "\\( a \\le b \\)" }));
    assert_eq!(recognise(&s).expect("latex"), "a \\le b");
    server.reply(200, json!({ "error": "Content not found", "error_info": { "id": "image_no_content" } }));
    assert!(matches!(recognise(&s), Err(OcrError::BadReply)));
}

#[test]
fn the_picture_is_dark_ink_on_white_with_a_margin_and_fits_a_fixed_size() {
    let png = ocr::raster::png(&plus()).expect("png");
    let img = image::load_from_memory(&png).expect("a real PNG").to_luma8();
    let (w, h) = img.dimensions();
    assert!(w.max(h) <= ocr::raster::MAX_SIDE && w.min(h) >= 32, "{w}x{h}");
    let dark = |x: u32, y: u32| img.get_pixel(x, y).0[0] < 80;
    assert!(dark(w / 2, h / 2), "the strokes cross in the middle");
    assert!(dark(w / 4, h / 2) && dark(w / 2, h / 4), "and reach out along both arms");
    for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1), (w / 4, h / 4), (3 * w / 4, 3 * h / 4)] {
        assert_eq!(img.get_pixel(x, y).0[0], 255, "paper at {x},{y}");
    }

    // Pale ink is still dark in the picture; highlighter is left out; nothing at all is an error.
    let mut pale = plus();
    pale[0].colour = Rgb(0xff, 0xf0, 0x80);
    pale[1].tool = Tool::Highlighter;
    let img = image::load_from_memory(&ocr::raster::png(&pale).expect("png")).expect("png").to_luma8();
    assert!(img.pixels().any(|px| px.0[0] < 80) && img.get_pixel(img.width() / 2, 0).0[0] == 255);
    assert!(matches!(ocr::raster::png(&pale[1..]), Err(OcrError::NoInk)));
    assert!(matches!(ocr::raster::png(&[]), Err(OcrError::NoInk)));

    // A page-wide line still fits, and a far-off or broken stroke does not make a huge picture.
    let mut wide = Stroke::new(Tool::Pen, Rgb::BLACK, 2.0);
    wide.dabs = vec![Dab::new(0.0, 0.0, 1.0), Dab::new(5000.0, 30.0, 1.0)];
    let img = image::load_from_memory(&ocr::raster::png(&[wide]).expect("png")).expect("png");
    assert!(img.width() <= ocr::raster::MAX_SIDE && img.height() <= ocr::raster::MAX_SIDE);
    let mut broken = Stroke::new(Tool::Pen, Rgb::BLACK, 2.0);
    broken.dabs = vec![Dab { x: f32::NAN, y: 0.0, pressure: 1.0 }, Dab { x: f32::INFINITY, y: 4.0, pressure: 1.0 }];
    assert!(matches!(ocr::raster::png(&[broken]), Err(OcrError::NoInk)));
}

#[test]
fn an_answer_is_tidied_to_one_line_of_bare_latex() {
    for (raw, want) in [
        ("x", "x"),
        ("  $$\n\\frac{a}{b}\n$$  ", "\\frac{a}{b}"),
        ("```latex\n\\alpha\n```", "\\alpha"),
        ("```\n\\beta\n```", "\\beta"),
        ("```latex x^2 ```", "x^2"),
        ("```tex \\frac{1}{2}```", "\\frac{1}{2}"),
        ("```x^2```", "x^2"),
        ("```a + b```", "a + b"),
        ("```latex $x^2$ ```", "x^2"),
        ("\\[ a \\]", "a"),
        ("\\( a \\)", "a"),
        ("$a$", "a"),
        ("\\begin{aligned}\na &= b \\\\\nc &= d\n\\end{aligned}", "\\begin{aligned} a &= b \\\\ c &= d \\end{aligned}"),
        ("a\r\n+\tb", "a + b"),
        ("\\$5", "\\$5"),
        ("$a$ and $b$", "$a$ and $b$"),
    ] {
        assert_eq!(ocr::clean_latex(raw), want, "{raw:?}");
    }
    assert_eq!(ocr::clean_latex("```\n```"), "");
    assert_eq!(ocr::clean_latex("```latex```"), "");
    assert_eq!(ocr::clean_latex("``````"), "");
}

#[test]
fn a_job_can_be_made_here_and_sent_from_another_thread() {
    fn sendable<T: Send + 'static>(_: &T) {}
    let t = TempDir::new("math-ocr-thread");
    let server = Server::start();
    let s = ready(&t, &server, "openai", &[]);
    server.reply(200, json!({ "choices": [{ "message": { "content": "z" } }] }));
    let job: OcrJob = s.math_ocr_job(&plus()).expect("job");
    sendable(&job);
    let http = s.sync_http();
    let answer = thread::spawn(move || ocr::recognize(&*http, &job)).join().expect("worker");
    assert_eq!(answer.expect("latex"), "z");
}

#[test]
fn the_convert_command_reads_a_pages_ink_and_gives_back_latex() {
    let t = TempDir::new("math-ocr-command");
    let server = Server::start();
    let mut s = ready(&t, &server, "openai", &[]);
    let path_of = |out: Output| match out {
        Output::Path(path) => path,
        other => panic!("expected Output::Path, got {other:?}"),
    };
    let nb = path_of(s.run("notebook.create", &p(&[("name", "Sums")])).expect("notebook"));
    let sec = path_of(s.run("node.create", &p(&[("parent", &nb.to_string_lossy()), ("kind", "section"), ("title", "Week")])).expect("section"));
    let page = path_of(s.run("node.create", &p(&[("parent", &sec.to_string_lossy()), ("kind", "page"), ("title", "One")])).expect("page"));
    let at = page.to_string_lossy().to_string();
    let at = at.as_str();

    assert!(matches!(s.run("ink.math_ocr_convert", &p(&[("path", at)])), Err(EngineError::BadParam(_))), "a page with no ink");
    let mut drawing = Drawing::new(600.0, 400.0);
    drawing.strokes = plus();
    std::fs::write(page.join("ink.svg"), nocturne_ink::svg::write(&drawing, nocturne_ink::svg::Flavour::Editable)).expect("ink");

    server.reply(200, json!({ "choices": [{ "message": { "content": "a+b" } }] }));
    assert_eq!(json_of(s.run("ink.math_ocr_convert", &p(&[("path", at)])).expect("convert")), json!({ "latex": "a+b" }));
    let whole = server.last().body.len();
    server.reply(200, json!({ "choices": [{ "message": { "content": "-" } }] }));
    assert_eq!(json_of(s.run("ink.math_ocr_convert", &p(&[("path", at), ("strokes", "0")])).expect("one stroke")), json!({ "latex": "-" }));
    assert_ne!(server.last().body.len(), whole, "one stroke is a different picture");

    for bad in ["2", "0,x", "-1"] {
        let r = s.run("ink.math_ocr_convert", &p(&[("path", at), ("strokes", bad)]));
        assert!(matches!(r, Err(EngineError::BadParam(_))), "strokes={bad} is refused");
    }
    server.reply(401, json!({}));
    let err = s.run("ink.math_ocr_convert", &p(&[("path", at)])).expect_err("bad key");
    assert!(!err.to_string().contains(KEY));
    let before = server.count();
    s.run("ink.math_ocr_toggle", &p(&[("enabled", "false")])).expect("off");
    assert!(s.run("ink.math_ocr_convert", &p(&[("path", at)])).is_err());
    assert_eq!(server.count(), before, "off means no request");
}

#[test]
fn cross_provider_switch_cannot_leak_key_to_new_endpoint_or_corrupt_mathpix_app_id() {
    let t = TempDir::new("math-ocr-isolation");
    let first = Server::start();
    let second = Server::start();

    // Configure OpenAI at `first`, switch to Anthropic, then switch back to OpenAI at `second`
    // WITHOUT a new key: the old OpenAI key bound to `first` must be forgotten and never sent to `second`.
    let mut s = ready(&t, &first, "openai", &[]);
    s.run("ink.math_ocr_configure", &p(&[("provider", "anthropic")])).expect("switch away");
    let back = json_of(s.run("ink.math_ocr_configure", &p(&[("provider", "openai"), ("endpoint", &second.endpoint)])).expect("switch back"));
    assert_eq!(back["has_key"], false, "key bound to first endpoint must not survive move via another provider");
    assert!(matches!(recognise(&s), Err(OcrError::NotConfigured(_))));
    assert_eq!(first.count() + second.count(), 0);

    // Same when switching back to the provider's default endpoint (not `first`):
    let mut s = ready(&t, &first, "openai", &[]);
    s.run("ink.math_ocr_configure", &p(&[("provider", "gemini")])).expect("switch to gemini");
    let back_default = json_of(s.run("ink.math_ocr_configure", &p(&[("provider", "openai")])).expect("back to default"));
    assert_eq!(back_default["has_key"], false, "key bound to custom endpoint must not leak to default endpoint");

    // Passing `app_id` while configuring a non-Mathpix provider must NOT overwrite Mathpix's app_id,
    // and moving Mathpix's endpoint with only a new `key` must forget the old `app_id`.
    let mut s = ready(&t, &first, "mathpix", &[("app_id", "real-mathpix-app")]);
    assert!(s.ocr_credentials_complete(OcrProvider::Mathpix));
    s.run("ink.math_ocr_configure", &p(&[("provider", "openai"), ("app_id", "bogus-app")])).expect("openai with app_id");
    assert_eq!(s.sync_secrets().get_secret("ocr_app_id_mathpix").expect("secret").as_deref(), Some("real-mathpix-app"));

    let moved = json_of(
        s.run("ink.math_ocr_configure", &p(&[("provider", "mathpix"), ("endpoint", &second.endpoint), ("key", "new-key")])).expect("move mathpix"),
    );
    assert_eq!(moved["has_key"], true);
    assert_eq!(moved["has_app_id"], false, "old mathpix app_id must be cleared when moving endpoint");
    assert!(!s.ocr_credentials_complete(OcrProvider::Mathpix));
}

#[test]
fn gemini_filters_thought_parts_and_mathpix_handles_empty_styled_and_auth_error() {
    let t = TempDir::new("math-ocr-gemini-mathpix-fixes");
    let server = Server::start();

    let s = ready(&t, &server, "gemini", &[]);
    server.reply(
        200,
        json!({
            "candidates": [{
                "finishReason": "STOP",
                "content": {
                    "parts": [
                        { "thought": true, "text": "Let me examine the strokes carefully. Looks like x squared." },
                        { "text": "```latex x^2 ```" }
                    ]
                }
            }]
        }),
    );
    assert_eq!(recognise(&s).expect("gemini non-thought latex"), "x^2");

    // Only thought parts + MAX_TOKENS -> Truncated (not BadReply, not thought leakage).
    server.reply(
        200,
        json!({
            "candidates": [{
                "finishReason": "MAX_TOKENS",
                "content": {
                    "parts": [{ "thought": true, "text": "still thinking..." }]
                }
            }]
        }),
    );
    assert!(matches!(recognise(&s), Err(OcrError::Truncated)));

    // Mathpix: delimiter-only `latex_styled` falls back to `text`; auth error_info returns Auth.
    let s = ready(&t, &server, "mathpix", &[("app_id", "app-1")]);
    server.reply(200, json!({ "latex_styled": "$$ $$", "text": "\\( \\alpha + \\beta \\)" }));
    assert_eq!(recognise(&s).expect("fallback to text"), "\\alpha + \\beta");

    server.reply(200, json!({ "error": "Invalid credentials", "error_info": { "id": "http_unauthorized" } }));
    assert!(matches!(recognise(&s), Err(OcrError::Auth)));
}

#[test]
fn apple_vision_backend_and_local_model_provider_wire_through_session_without_network() {
    use nocturne_engine::sync::UnsupportedSyncClient;
    use nocturne_engine::{NativeOcrBackend, VisionObservation};

    struct FixedVisionBackend;
    impl NativeOcrBackend for FixedVisionBackend {
        fn recognize_png(&self, png_bytes: &[u8]) -> Result<Vec<VisionObservation>, OcrError> {
            assert!(png_bytes.starts_with(b"\x89PNG"), "receives real rasterized PNG bytes");
            Ok(vec![
                VisionObservation::new("x", 0.10, 0.40, 0.10, 0.20, 0.99),
                VisionObservation::new("2", 0.21, 0.30, 0.06, 0.11, 0.99),
                VisionObservation::new("+", 0.32, 0.43, 0.08, 0.14, 0.99),
                VisionObservation::new("π", 0.44, 0.40, 0.10, 0.20, 0.99),
            ])
        }
    }

    let t = TempDir::new("math-ocr-on-device");
    let mut s = session(&t);
    s.set_sync_backends(Arc::new(UnsupportedSyncClient::new("offline")), s.sync_secrets());
    s.run("ink.math_ocr_toggle", &p(&[("enabled", "true")])).expect("on");

    // AppleVision without backend -> Unsupported.
    let st = json_of(s.run("ink.math_ocr_configure", &p(&[("provider", "apple_vision")])).expect("apple_vision"));
    assert_eq!(st["provider"], "apple_vision");
    assert_eq!(st["ready"], false);
    assert_eq!(st["native_ocr_available"], false);
    assert!(matches!(s.math_ocr_job(&plus()), Err(OcrError::Unsupported)));

    // Install NativeOcrBackend -> works completely offline!
    s.set_native_ocr_backend(Arc::new(FixedVisionBackend));
    let st = json_of(s.run("ink.math_ocr_configure", &Params::new()).expect("status"));
    assert_eq!(st["ready"], true);
    assert_eq!(st["native_ocr_available"], true);
    let job = s.math_ocr_job(&plus()).expect("apple vision job");
    assert_eq!(ocr::recognize(&*s.sync_http(), &job).expect("latex"), "x^{2} + \\pi");

    // LocalModel before download -> NotConfigured; remove_model command exists and succeeds cleanly.
    let st = json_of(s.run("ink.math_ocr_configure", &p(&[("provider", "local_model")])).expect("local_model"));
    assert_eq!(st["provider"], "local_model");
    assert_eq!(st["model"], ocr::local_model::MODEL_ID);
    assert_eq!(st["model_installed"], false);
    assert_eq!(st["ready"], false);
    assert!(matches!(s.math_ocr_job(&plus()), Err(OcrError::NotConfigured(_))));

    assert!(registry().iter().any(|c| c.id == "ink.math_ocr_remove_model" && !c.changes_tree));
    std::fs::create_dir_all(s.ocr_model_dir()).expect("dir");
    std::fs::write(s.ocr_model_dir().join(ocr::local_model::TOKENIZER_FILE), b"{}").expect("partial file");
    let removed = json_of(s.run("ink.math_ocr_remove_model", &Params::new()).expect("remove"));
    assert_eq!(removed, json!({ "removed": true, "model_installed": false }));
    assert!(!s.ocr_model_dir().join(ocr::local_model::TOKENIZER_FILE).exists());
}
