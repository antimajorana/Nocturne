//! Platform-neutral HTTP and secret-storage traits for cloud sync.

use crate::sync::SyncError;
use nocturne_store::Settings;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncHttpMethod {
    Get,
    Post,
    Delete,
}

#[derive(Clone, Debug)]
pub struct SyncHttpRequest {
    pub method: SyncHttpMethod,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeout_secs: u64,
}

#[derive(Clone, Debug)]
pub struct SyncHttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl SyncHttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

pub trait SyncHttpClient: Send + Sync {
    fn is_supported(&self) -> bool {
        true
    }

    fn request(&self, req: SyncHttpRequest) -> Result<SyncHttpResponse, SyncError>;
}

pub trait SecretStore: Send + Sync {
    fn is_supported(&self) -> bool {
        true
    }

    fn get_secret(&self, key: &str) -> Result<Option<String>, SyncError>;
    fn set_secret(&self, key: &str, secret: &str) -> Result<(), SyncError>;
    fn delete_secret(&self, key: &str) -> Result<(), SyncError>;
}

pub struct UnsupportedSyncClient {
    reason: String,
}

impl UnsupportedSyncClient {
    pub fn new(reason: impl Into<String>) -> Self {
        Self { reason: reason.into() }
    }
}

impl SyncHttpClient for UnsupportedSyncClient {
    fn is_supported(&self) -> bool {
        false
    }

    fn request(&self, _req: SyncHttpRequest) -> Result<SyncHttpResponse, SyncError> {
        Err(SyncError::Unsupported(self.reason.clone()))
    }
}

pub struct UnsupportedSecretStore {
    reason: String,
}

impl UnsupportedSecretStore {
    pub fn new(reason: impl Into<String>) -> Self {
        Self { reason: reason.into() }
    }
}

impl SecretStore for UnsupportedSecretStore {
    fn is_supported(&self) -> bool {
        false
    }

    fn get_secret(&self, _key: &str) -> Result<Option<String>, SyncError> {
        Ok(None)
    }

    fn set_secret(&self, _key: &str, _secret: &str) -> Result<(), SyncError> {
        Err(SyncError::Unsupported(self.reason.clone()))
    }

    fn delete_secret(&self, _key: &str) -> Result<(), SyncError> {
        Ok(())
    }
}

/// Stores secrets in `<settings_dir>/<key>.secret` with owner-only permissions on Unix.
/// Never writes secrets into `settings.toml` or any notebook directory.
pub struct FileSecretStore {
    dir: PathBuf,
}

impl FileSecretStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path_for(&self, key: &str) -> PathBuf {
        let safe: String = key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        self.dir.join(format!("{safe}.secret"))
    }
}

impl SecretStore for FileSecretStore {
    fn get_secret(&self, key: &str) -> Result<Option<String>, SyncError> {
        let path = self.path_for(key);
        match std::fs::read_to_string(&path) {
            Ok(s) => {
                let trimmed = s.trim().to_string();
                if trimmed.is_empty() { Ok(None) } else { Ok(Some(trimmed)) }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(SyncError::Io(format!("read secret {}: {e}", path.display()))),
        }
    }

    fn set_secret(&self, key: &str, secret: &str) -> Result<(), SyncError> {
        let trimmed = secret.trim();
        if trimmed.is_empty() {
            return self.delete_secret(key);
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| SyncError::Io(format!("create {}: {e}", self.dir.display())))?;
        let path = self.path_for(key);
        let tmp = path.with_extension("tmp-secret");
        std::fs::write(&tmp, trimmed.as_bytes()).map_err(|e| SyncError::Io(format!("write {}: {e}", tmp.display())))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            SyncError::Io(format!("save secret {}: {e}", path.display()))
        })
    }

    fn delete_secret(&self, key: &str) -> Result<(), SyncError> {
        let path = self.path_for(key);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(SyncError::Io(format!("delete secret {}: {e}", path.display()))),
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
pub struct UreqSyncClient {
    agent: ureq::Agent,
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl Default for UreqSyncClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl UreqSyncClient {
    pub fn new() -> Self {
        use std::time::Duration;
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .user_agent(format!("Nocturne/{}", env!("CARGO_PKG_VERSION")))
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
            .build();
        Self { agent: ureq::Agent::new_with_config(config) }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl SyncHttpClient for UreqSyncClient {
    fn request(&self, req: SyncHttpRequest) -> Result<SyncHttpResponse, SyncError> {
        use std::io::Read;
        let mut resp = match req.method {
            SyncHttpMethod::Get => {
                let mut builder = self.agent.get(&req.url);
                for (k, v) in &req.headers {
                    builder = builder.header(k, v);
                }
                builder.call().map_err(|e| SyncError::Transport(e.to_string()))?
            }
            SyncHttpMethod::Delete => {
                let mut builder = self.agent.delete(&req.url);
                for (k, v) in &req.headers {
                    builder = builder.header(k, v);
                }
                builder.call().map_err(|e| SyncError::Transport(e.to_string()))?
            }
            SyncHttpMethod::Post => {
                let mut builder = self.agent.post(&req.url);
                for (k, v) in &req.headers {
                    builder = builder.header(k, v);
                }
                builder.send(&req.body[..]).map_err(|e| SyncError::Transport(e.to_string()))?
            }
        };
        let status = resp.status().as_u16();
        let headers = resp.headers().iter().filter_map(|(k, v)| v.to_str().ok().map(|val| (k.as_str().to_string(), val.to_string()))).collect();
        let mut body = Vec::new();
        resp.body_mut().with_config().limit(64 * 1024 * 1024).reader().read_to_end(&mut body).map_err(|e| SyncError::Transport(e.to_string()))?;
        Ok(SyncHttpResponse { status, headers, body })
    }
}

pub fn default_http_client() -> Arc<dyn SyncHttpClient> {
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    {
        Arc::new(UreqSyncClient::new())
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        Arc::new(UnsupportedSyncClient::new("kDrive network sync is not enabled in this runtime build"))
    }
}

pub fn default_secret_store(settings: &Settings) -> Arc<dyn SecretStore> {
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    {
        let dir = settings.path().parent().unwrap_or_else(|| std::path::Path::new(".")).to_path_buf();
        Arc::new(FileSecretStore::new(dir))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        let _ = settings;
        Arc::new(UnsupportedSecretStore::new("secure token storage is not enabled in this runtime build"))
    }
}
