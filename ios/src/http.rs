//! `NSURLSession` HTTP bridge implementing [`nocturne_engine::SyncHttpClient`] on iPadOS / iOS.

use nocturne_engine::{SyncError, SyncHttpMethod};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static DOWNLOAD_TMP_SEQ: AtomicU64 = AtomicU64::new(1);

/// Maps a [`SyncHttpMethod`] to its canonical HTTP verb string.
#[must_use]
pub fn method_as_str(method: SyncHttpMethod) -> &'static str {
    match method {
        SyncHttpMethod::Get => "GET",
        SyncHttpMethod::Post => "POST",
        SyncHttpMethod::Delete => "DELETE",
    }
}

/// Validates a request URL before handing it to `NSURLSession`.
pub fn validate_request_url(url: &str) -> Result<&str, SyncError> {
    let trimmed = url.trim();
    let authority = trimmed.split("://").nth(1).and_then(|s| s.split(['/', '?', '#']).next()).unwrap_or("");
    if authority.contains('@') {
        return Err(SyncError::NotConfigured("sync URL must not embed userinfo credentials".into()));
    }
    if trimmed.starts_with("https://") {
        return Ok(trimmed);
    }
    for prefix in ["http://127.0.0.1", "http://localhost"] {
        if let Some(rest) = trimmed.strip_prefix(prefix)
            && (rest.is_empty() || rest.starts_with(':') || rest.starts_with('/'))
        {
            return Ok(trimmed);
        }
    }
    Err(SyncError::NotConfigured("sync URL must start with https://".into()))
}

/// Converts raw `NSURLSessionTask` byte counters into `(bytes_received, expected_total_bytes)`.
///
/// Negative values (such as `NSURLSessionTransferSizeUnknown = -1`) collapse to `0` / `None`.
#[must_use]
pub fn parse_download_progress(received: i64, expected: i64) -> (u64, Option<u64>) {
    let done = u64::try_from(received).unwrap_or(0);
    let total = (expected > 0).then_some(expected as u64);
    (done, total)
}

/// Validates that a download HTTP status code is in `200..=299`.
pub fn check_download_http_status(url: &str, status: u16) -> Result<(), SyncError> {
    if (200..=299).contains(&status) { Ok(()) } else { Err(SyncError::Protocol(format!("download {url} failed with HTTP {status}"))) }
}

/// Moves (or copies across volumes) a completed temporary download file `src` to `dest` cleanly,
/// creating parent directories as needed and removing any partial staging file on error.
pub fn finalize_downloaded_temp_file(src: &Path, dest: &Path) -> Result<u64, SyncError> {
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| SyncError::Io(format!("create {}: {e}", parent.display())))?;
    }

    if std::fs::rename(src, dest).is_err() {
        let seq = DOWNLOAD_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let parent = dest.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
        let tmp = parent.join(format!(".nocturne-dl-{}-{seq}.part", std::process::id()));
        if let Err(err) = std::fs::copy(src, &tmp) {
            let _ = std::fs::remove_file(&tmp);
            return Err(SyncError::Io(format!("copy download to {}: {err}", dest.display())));
        }
        if let Err(err) = std::fs::rename(&tmp, dest) {
            let _ = std::fs::remove_file(&tmp);
            return Err(SyncError::Io(format!("move download to {}: {err}", dest.display())));
        }
        let _ = std::fs::remove_file(src);
    }

    let meta = std::fs::metadata(dest).map_err(|e| SyncError::Io(format!("stat {}: {e}", dest.display())))?;
    Ok(meta.len())
}

#[cfg(target_os = "ios")]
mod imp {
    use super::{check_download_http_status, finalize_downloaded_temp_file, method_as_str, parse_download_progress, validate_request_url};
    use block2::RcBlock;
    use nocturne_engine::{SyncError, SyncHttpClient, SyncHttpMethod, SyncHttpRequest, SyncHttpResponse};
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_foundation::{
        NSData, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSString, NSURL, NSURLRequestCachePolicy, NSURLResponse, NSURLSession,
    };
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    /// Thread-safe `NSURLSession` transport initialized on the UIKit main thread and usable from sync worker threads.
    pub struct NsUrlSessionClient {
        _private: (),
    }

    impl NsUrlSessionClient {
        #[must_use]
        pub fn new(_mtm: MainThreadMarker) -> Self {
            Self { _private: () }
        }
    }

    impl SyncHttpClient for NsUrlSessionClient {
        fn request(&self, req: SyncHttpRequest) -> Result<SyncHttpResponse, SyncError> {
            objc2::rc::autoreleasepool(|_| {
                let clean_url = validate_request_url(&req.url)?;
                let ns_url_str = NSString::from_str(clean_url);
                let ns_url = NSURL::URLWithString(&ns_url_str).ok_or_else(|| SyncError::Protocol(format!("invalid URL: {clean_url}")))?;

                let timeout = req.timeout_secs.clamp(5, 300) as f64;
                let request: Retained<NSMutableURLRequest> = NSMutableURLRequest::requestWithURL_cachePolicy_timeoutInterval(
                    &ns_url,
                    NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData,
                    timeout,
                );
                let verb = NSString::from_str(method_as_str(req.method));
                request.setHTTPMethod(&verb);

                for (name, value) in &req.headers {
                    let k = NSString::from_str(name);
                    let v = NSString::from_str(value);
                    request.setValue_forHTTPHeaderField(Some(&v), &k);
                }
                if !req.body.is_empty() {
                    let body_data = NSData::with_bytes(&req.body);
                    request.setHTTPBody(Some(&body_data));
                }

                let pair: Arc<(Mutex<Option<Result<SyncHttpResponse, SyncError>>>, Condvar)> = Arc::new((Mutex::new(None), Condvar::new()));
                let pair_cb = Arc::clone(&pair);

                let block = RcBlock::new(move |data_ptr: *mut NSData, resp_ptr: *mut NSURLResponse, err_ptr: *mut NSError| {
                    let outcome = if !err_ptr.is_null() {
                        #[allow(unsafe_code)]
                        // SAFETY: `NSURLSession` passes a valid `NSError` object pointer for the duration of the completion handler.
                        let err_ref = unsafe { &*err_ptr };
                        Err(SyncError::Transport(err_ref.localizedDescription().to_string()))
                    } else if resp_ptr.is_null() {
                        Err(SyncError::Transport("NSURLSession returned neither response nor error".into()))
                    } else {
                        #[allow(unsafe_code)]
                        // SAFETY: `resp_ptr` is non-null and valid for the duration of the completion handler.
                        let resp_ref = unsafe { &*resp_ptr };
                        let (status, headers) = if let Some(http_resp) = resp_ref.downcast_ref::<NSHTTPURLResponse>() {
                            let code = u16::try_from(http_resp.statusCode()).unwrap_or(500);
                            let mut extracted = Vec::new();
                            for name in ["Retry-After", "Content-Type", "Content-Length"] {
                                let key = NSString::from_str(name);
                                if let Some(val) = http_resp.valueForHTTPHeaderField(&key) {
                                    extracted.push((name.to_string(), val.to_string()));
                                }
                            }
                            (code, extracted)
                        } else {
                            (200, Vec::new())
                        };
                        let body = if data_ptr.is_null() {
                            Vec::new()
                        } else {
                            #[allow(unsafe_code)]
                            // SAFETY: `data_ptr` is non-null and valid for the duration of the completion handler.
                            let data_ref = unsafe { &*data_ptr };
                            data_ref.to_vec()
                        };
                        Ok(SyncHttpResponse { status, headers, body })
                    };

                    if let Ok(mut slot) = pair_cb.0.lock() {
                        *slot = Some(outcome);
                        pair_cb.1.notify_all();
                    }
                });

                let session = NSURLSession::sharedSession();
                #[allow(unsafe_code)]
                // SAFETY: `block` only captures `Arc<(Mutex<...>, Condvar)>`, which is `Send + Sync`.
                let task = unsafe { session.dataTaskWithRequest_completionHandler(&request, &block) };
                task.resume();

                // `timeoutInterval` enforces per-activity idle timeout inside NSURLSession; use a generous
                // outer safety ceiling here so active multi-MB uploads on slow links are not cut off early.
                let wait_limit = Duration::from_secs(req.timeout_secs.clamp(5, 300).saturating_mul(6).max(180));
                let mut guard = pair.0.lock().map_err(|_| SyncError::Transport("NSURLSession mutex poisoned".into()))?;
                if guard.is_none() {
                    let (next_guard, timeout_res) = pair
                        .1
                        .wait_timeout_while(guard, wait_limit, |slot| slot.is_none())
                        .map_err(|_| SyncError::Transport("NSURLSession wait poisoned".into()))?;
                    guard = next_guard;
                    if timeout_res.timed_out() && guard.is_none() {
                        task.cancel();
                        return Err(SyncError::Transport("NSURLSession request timed out".into()));
                    }
                }
                guard.take().unwrap_or_else(|| Err(SyncError::Transport("NSURLSession finished without result".into())))
            })
        }

        fn download_to_file(
            &self,
            url: &str,
            dest: &Path,
            cancel: Option<&AtomicBool>,
            on_progress: &mut dyn FnMut(u64, Option<u64>),
        ) -> Result<u64, SyncError> {
            if cancel.is_some_and(|f| f.load(Ordering::Relaxed)) {
                return Err(SyncError::Cancelled);
            }

            objc2::rc::autoreleasepool(|_| {
                let clean_url = validate_request_url(url)?.to_string();
                let ns_url_str = NSString::from_str(&clean_url);
                let ns_url = NSURL::URLWithString(&ns_url_str).ok_or_else(|| SyncError::Protocol(format!("invalid URL: {clean_url}")))?;

                let request: Retained<NSMutableURLRequest> = NSMutableURLRequest::requestWithURL_cachePolicy_timeoutInterval(
                    &ns_url,
                    NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData,
                    300.0,
                );
                let verb = NSString::from_str(method_as_str(SyncHttpMethod::Get));
                request.setHTTPMethod(&verb);
                let accept_key = NSString::from_str("Accept");
                let accept_val = NSString::from_str("*/*");
                request.setValue_forHTTPHeaderField(Some(&accept_val), &accept_key);

                let pair: Arc<(Mutex<Option<Result<u64, SyncError>>>, Condvar)> = Arc::new((Mutex::new(None), Condvar::new()));
                let pair_cb = Arc::clone(&pair);
                let cancelled = Arc::new(AtomicBool::new(false));
                let cancelled_cb = Arc::clone(&cancelled);
                let dest_lock = Arc::new(Mutex::new(()));
                let dest_lock_cb = Arc::clone(&dest_lock);
                let dest_owned = dest.to_path_buf();
                let url_owned = clean_url;

                let block = RcBlock::new(move |loc_ptr: *mut NSURL, resp_ptr: *mut NSURLResponse, err_ptr: *mut NSError| {
                    let outcome = if cancelled_cb.load(Ordering::SeqCst) {
                        Err(SyncError::Cancelled)
                    } else if !err_ptr.is_null() {
                        #[allow(unsafe_code)]
                        // SAFETY: `NSURLSession` passes a valid `NSError` pointer for the duration of the completion handler.
                        let err_ref = unsafe { &*err_ptr };
                        if cancelled_cb.load(Ordering::SeqCst) {
                            Err(SyncError::Cancelled)
                        } else {
                            Err(SyncError::Transport(err_ref.localizedDescription().to_string()))
                        }
                    } else if resp_ptr.is_null() {
                        Err(SyncError::Transport("NSURLSession download returned neither response nor error".into()))
                    } else {
                        #[allow(unsafe_code)]
                        // SAFETY: `resp_ptr` is non-null and valid for the duration of the completion handler.
                        let resp_ref = unsafe { &*resp_ptr };
                        let status_check = if let Some(http_resp) = resp_ref.downcast_ref::<NSHTTPURLResponse>() {
                            let code = u16::try_from(http_resp.statusCode()).unwrap_or(500);
                            check_download_http_status(&url_owned, code)
                        } else {
                            Ok(())
                        };
                        match status_check {
                            Err(err) => Err(err),
                            Ok(()) if loc_ptr.is_null() => {
                                Err(SyncError::Transport("NSURLSession download finished without a temporary file location".into()))
                            }
                            Ok(()) => {
                                #[allow(unsafe_code)]
                                // SAFETY: `loc_ptr` is non-null and valid for the duration of the completion handler.
                                let loc_ref = unsafe { &*loc_ptr };
                                match loc_ref.path() {
                                    None => Err(SyncError::Io("NSURLSession temporary download URL has no filesystem path".into())),
                                    Some(ns_path) => {
                                        let src_path = PathBuf::from(ns_path.to_string());
                                        // Must move/copy the file before this completion handler returns,
                                        // because `NSURLSession` deletes `location` immediately afterward.
                                        let _dest_guard = dest_lock_cb.lock();
                                        if cancelled_cb.load(Ordering::SeqCst) {
                                            Err(SyncError::Cancelled)
                                        } else {
                                            finalize_downloaded_temp_file(&src_path, &dest_owned)
                                        }
                                    }
                                }
                            }
                        }
                    };

                    if let Ok(mut slot) = pair_cb.0.lock() {
                        *slot = Some(outcome);
                        pair_cb.1.notify_all();
                    }
                });

                let session = NSURLSession::sharedSession();
                #[allow(unsafe_code)]
                // SAFETY: `block` only captures `Send + Sync` values (`Arc`, `PathBuf`, `String`).
                let task = unsafe { session.downloadTaskWithRequest_completionHandler(&request, &block) };
                task.resume();

                let poll_interval = Duration::from_millis(100);
                let stall_limit = Duration::from_secs(300);
                let mut last_bytes = 0u64;
                let mut last_total: Option<u64> = None;
                let mut last_activity = Instant::now();

                let outcome = loop {
                    let guard = pair.0.lock().map_err(|_| SyncError::Transport("NSURLSession download mutex poisoned".into()))?;
                    let (mut guard, _) = pair
                        .1
                        .wait_timeout_while(guard, poll_interval, |slot| slot.is_none())
                        .map_err(|_| SyncError::Transport("NSURLSession download wait poisoned".into()))?;
                    if let Some(res) = guard.take() {
                        break res;
                    }
                    drop(guard);

                    if cancel.is_some_and(|f| f.load(Ordering::Relaxed)) {
                        cancelled.store(true, Ordering::SeqCst);
                        task.cancel();
                        let _dest_guard = dest_lock.lock();
                        let _ = std::fs::remove_file(dest);
                        return Err(SyncError::Cancelled);
                    }

                    let (bytes, total) = parse_download_progress(task.countOfBytesReceived(), task.countOfBytesExpectedToReceive());
                    if bytes > last_bytes || (bytes > 0 && total != last_total) {
                        last_bytes = bytes;
                        last_total = total;
                        last_activity = Instant::now();
                        on_progress(bytes, total);
                    }

                    if last_activity.elapsed() >= stall_limit {
                        cancelled.store(true, Ordering::SeqCst);
                        task.cancel();
                        let _dest_guard = dest_lock.lock();
                        let _ = std::fs::remove_file(dest);
                        return Err(SyncError::Transport("NSURLSession download stalled and timed out".into()));
                    }
                };

                if cancel.is_some_and(|f| f.load(Ordering::Relaxed)) {
                    let _dest_guard = dest_lock.lock();
                    let _ = std::fs::remove_file(dest);
                    return Err(SyncError::Cancelled);
                }

                match outcome {
                    Ok(total_bytes) => {
                        on_progress(total_bytes, Some(total_bytes));
                        Ok(total_bytes)
                    }
                    Err(err) => {
                        let _dest_guard = dest_lock.lock();
                        let _ = std::fs::remove_file(dest);
                        Err(err)
                    }
                }
            })
        }
    }
}

#[cfg(target_os = "ios")]
pub use imp::NsUrlSessionClient;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_urls_and_methods() {
        assert_eq!(method_as_str(SyncHttpMethod::Get), "GET");
        assert_eq!(method_as_str(SyncHttpMethod::Post), "POST");
        assert_eq!(method_as_str(SyncHttpMethod::Delete), "DELETE");
        assert!(validate_request_url("https://api.infomaniak.com/2/drive").is_ok());
        assert!(validate_request_url("http://127.0.0.1:8080/2/drive").is_ok());
        assert!(validate_request_url("http://example.com/insecure").is_err());
        assert!(validate_request_url("http://localhost.evil.example/2/drive").is_err());
        assert!(validate_request_url("https://user:pass@example.com/model.onnx").is_err());
    }

    #[test]
    fn parses_download_progress_counters() {
        assert_eq!(parse_download_progress(-1, -1), (0, None));
        assert_eq!(parse_download_progress(0, 0), (0, None));
        assert_eq!(parse_download_progress(4096, -1), (4096, None));
        assert_eq!(parse_download_progress(4096, 87_000_000), (4096, Some(87_000_000)));
    }

    #[test]
    fn checks_download_http_status_codes() {
        assert!(check_download_http_status("https://huggingface.co/model.onnx", 200).is_ok());
        assert!(check_download_http_status("https://huggingface.co/model.onnx", 206).is_ok());
        assert!(matches!(
            check_download_http_status("https://huggingface.co/model.onnx", 404),
            Err(SyncError::Protocol(msg)) if msg.contains("404")
        ));
        assert!(matches!(
            check_download_http_status("https://huggingface.co/model.onnx", 503),
            Err(SyncError::Protocol(msg)) if msg.contains("503")
        ));
    }

    #[test]
    fn finalizes_temp_download_into_nested_dest_and_replaces_existing() {
        let dir = std::env::temp_dir().join(format!("nocturne-ios-http-dl-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");

        let src = dir.join("nsurlsession-tmp.bin");
        let dest = dir.join("nested").join("models").join("encoder.onnx");
        std::fs::write(&src, b"payload-v1").expect("write tmp");

        let len1 = finalize_downloaded_temp_file(&src, &dest).expect("finalize v1");
        assert_eq!(len1, 10);
        assert_eq!(std::fs::read(&dest).expect("read v1"), b"payload-v1");
        assert!(!src.exists(), "source temp file should be moved");

        let src2 = dir.join("nsurlsession-tmp-2.bin");
        std::fs::write(&src2, b"payload-version-2").expect("write tmp 2");
        let len2 = finalize_downloaded_temp_file(&src2, &dest).expect("finalize v2");
        assert_eq!(len2, 17);
        assert_eq!(std::fs::read(&dest).expect("read v2"), b"payload-version-2");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
