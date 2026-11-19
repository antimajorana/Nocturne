//! `NSURLSession` HTTP bridge implementing [`nocturne_engine::SyncHttpClient`] on iPadOS / iOS.

use nocturne_engine::{SyncError, SyncHttpMethod};

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

#[cfg(target_os = "ios")]
mod imp {
    use super::{method_as_str, validate_request_url};
    use block2::RcBlock;
    use nocturne_engine::{SyncError, SyncHttpClient, SyncHttpRequest, SyncHttpResponse};
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_foundation::{
        NSData, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSString, NSURL, NSURLRequestCachePolicy, NSURLResponse, NSURLSession,
    };
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

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
    }
}
