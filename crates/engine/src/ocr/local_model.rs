//! Downloadable on-device free math OCR model: Pix2Text MFR 1.5 (`breezedeus/pix2text-mfr-1.5`, MIT).
//!
//! Model weights are pinned to commit `1cef9f0bdcd6a4c63df7de1311fb0894593340cc` on Hugging Face
//! and verified by exact byte size and SHA-256 digest in a staging directory before being installed
//! atomically into `<settings_dir>/models/pix2text-mfr-1.5/`.

use crate::ocr::OcrError;
use crate::sync::{SyncError, SyncHttpClient, sha256_file};
use nocturne_store::Settings;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Stable model identifier and directory name under `<settings_dir>/models/`.
pub const MODEL_ID: &str = "pix2text-mfr-1.5";
/// Pinned Hugging Face commit for `breezedeus/pix2text-mfr-1.5` (MIT license).
pub const MODEL_COMMIT: &str = "1cef9f0bdcd6a4c63df7de1311fb0894593340cc";
/// Base download URL ending in `/`.
pub const MODEL_BASE_URL: &str = "https://huggingface.co/breezedeus/pix2text-mfr-1.5/resolve/1cef9f0bdcd6a4c63df7de1311fb0894593340cc/";

pub const ENCODER_FILE: &str = "encoder_model.onnx";
pub const DECODER_FILE: &str = "decoder_model.onnx";
pub const TOKENIZER_FILE: &str = "tokenizer.json";

/// Image resolution expected by the Pix2Text MFR 1.5 ViT encoder (`[1, 3, 384, 384]`).
pub const IMG_SIZE: u32 = 384;

const START_TOKEN_ID: u32 = 1;
const EOS_TOKEN_ID: u32 = 2;
const MAX_DECODE_TOKENS: usize = 256;

/// One pinned file belonging to the downloadable math OCR model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFileSpec {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// Exact verified metadata for `breezedeus/pix2text-mfr-1.5` at [`MODEL_COMMIT`].
pub const MODEL_FILES: [ModelFileSpec; 3] = [
    ModelFileSpec { name: ENCODER_FILE, size: 87_510_770, sha256: "080a3f660f08bc9ebcacdd96e34be6b6400f8c7e62d7cd0dd8251badc37f610b" },
    ModelFileSpec { name: DECODER_FILE, size: 32_026_253, sha256: "917deb98e91a0453c5f234f58a0f32f9fb037de8527c7eb4ed394daf9e692f2a" },
    ModelFileSpec { name: TOKENIZER_FILE, size: 113_168, sha256: "4ffbeb2143e6a38324bb6111b7a8109530d38a076a8439aa5777535f0a32758a" },
];

/// Combined byte size across all 3 pinned model files (`119_650_191` bytes).
pub const TOTAL_MODEL_BYTES: u64 = 87_510_770 + 32_026_253 + 113_168;

/// Live download progress reported across all 3 model files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DownloadProgress {
    /// Zero-based index of the file currently downloading (`0..file_count`).
    pub file_index: usize,
    /// Total number of files in the model bundle (`3`).
    pub file_count: usize,
    /// Filename currently being downloaded.
    pub file_name: &'static str,
    /// Aggregate bytes downloaded so far across all files.
    pub bytes_downloaded: u64,
    /// Total aggregate bytes across all files ([`TOTAL_MODEL_BYTES`]).
    pub total_bytes: u64,
}

/// On-disk installation state of the local Pix2Text MFR 1.5 model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelStatus {
    NotInstalled,
    Partial { present_files: usize, total_files: usize, bytes_on_disk: u64 },
    Installed { total_bytes: u64 },
}

/// Deterministic storage folder for the downloaded model under the settings directory.
#[must_use]
pub fn default_model_dir(settings: &Settings) -> PathBuf {
    let base = settings.path().parent().unwrap_or_else(|| Path::new("."));
    base.join("models").join(MODEL_ID)
}

/// Returns `true` when this target supports running the local ONNX model.
#[must_use]
pub fn is_supported_on_platform() -> bool {
    cfg!(not(target_arch = "wasm32"))
}

/// Returns `true` when all 3 pinned model files exist in `dir` with their exact expected byte sizes.
#[must_use]
pub fn is_model_installed(dir: &Path) -> bool {
    matches!(model_status(dir), ModelStatus::Installed { .. })
}

/// Inspects `dir` and returns the current installation state of the local model.
#[must_use]
pub fn model_status(dir: &Path) -> ModelStatus {
    model_status_for_specs(dir, &MODEL_FILES)
}

fn model_status_for_specs(dir: &Path, specs: &[ModelFileSpec]) -> ModelStatus {
    let mut exact_matches = 0usize;
    let mut present_files = 0usize;
    let mut bytes_on_disk = 0u64;
    for spec in specs {
        let path = dir.join(spec.name);
        if let Ok(meta) = std::fs::metadata(&path)
            && meta.is_file()
        {
            present_files += 1;
            let len = meta.len();
            bytes_on_disk = bytes_on_disk.saturating_add(len);
            if len == spec.size {
                exact_matches += 1;
            }
        }
    }
    if exact_matches == specs.len() && !specs.is_empty() {
        ModelStatus::Installed { total_bytes: bytes_on_disk }
    } else if present_files == 0 {
        ModelStatus::NotInstalled
    } else {
        ModelStatus::Partial { present_files, total_files: specs.len(), bytes_on_disk }
    }
}

/// Downloads, SHA-256-verifies, and atomically installs Pix2Text MFR 1.5 into `dir`.
pub fn download_model(
    http: &dyn SyncHttpClient,
    dir: &Path,
    cancel: Option<&AtomicBool>,
    on_progress: &mut dyn FnMut(DownloadProgress),
) -> Result<(), SyncError> {
    download_model_from_specs(http, MODEL_BASE_URL, &MODEL_FILES, dir, cancel, on_progress)
}

/// Downloads and atomically installs a set of pinned [`ModelFileSpec`] files from `base_url` into
/// `dir`, verifying exact byte size and SHA-256 digest in `<dir>/.staging/` before installing.
pub fn download_model_from_specs(
    http: &dyn SyncHttpClient,
    base_url: &str,
    specs: &[ModelFileSpec],
    dir: &Path,
    cancel: Option<&AtomicBool>,
    on_progress: &mut dyn FnMut(DownloadProgress),
) -> Result<(), SyncError> {
    if !http.is_supported() {
        return Err(SyncError::Unsupported("network downloads are not available in this build".into()));
    }
    if cancel.is_some_and(|f| f.load(Ordering::Relaxed)) {
        return Err(SyncError::Cancelled);
    }

    std::fs::create_dir_all(dir).map_err(|e| SyncError::Io(format!("create {}: {e}", dir.display())))?;
    let staging_dir = dir.join(".staging");
    let _ = std::fs::remove_dir_all(&staging_dir);
    std::fs::create_dir_all(&staging_dir).map_err(|e| SyncError::Io(format!("create {}: {e}", staging_dir.display())))?;

    let total_bytes: u64 = specs.iter().map(|s| s.size).sum();
    let base = base_url.trim_end_matches('/');

    let res: Result<(), SyncError> = (|| {
        let mut completed_bytes = 0u64;
        for (idx, spec) in specs.iter().enumerate() {
            if cancel.is_some_and(|f| f.load(Ordering::Relaxed)) {
                return Err(SyncError::Cancelled);
            }
            let url = format!("{base}/{}", spec.name);
            let staged_path = staging_dir.join(spec.name);
            on_progress(DownloadProgress {
                file_index: idx,
                file_count: specs.len(),
                file_name: spec.name,
                bytes_downloaded: completed_bytes,
                total_bytes,
            });
            http.download_to_file(&url, &staged_path, cancel, &mut |file_done, _| {
                on_progress(DownloadProgress {
                    file_index: idx,
                    file_count: specs.len(),
                    file_name: spec.name,
                    bytes_downloaded: completed_bytes.saturating_add(file_done).min(total_bytes),
                    total_bytes,
                });
            })?;
            if cancel.is_some_and(|f| f.load(Ordering::Relaxed)) {
                return Err(SyncError::Cancelled);
            }
            let (actual_sha, actual_size) = sha256_file(&staged_path)?;
            if actual_size != spec.size {
                return Err(SyncError::Protocol(format!("{} size mismatch: expected {} bytes, got {}", spec.name, spec.size, actual_size)));
            }
            if !actual_sha.eq_ignore_ascii_case(spec.sha256) {
                return Err(SyncError::Protocol(format!("{} SHA-256 mismatch: expected {}, got {}", spec.name, spec.sha256, actual_sha)));
            }
            completed_bytes = completed_bytes.saturating_add(spec.size);
            on_progress(DownloadProgress {
                file_index: idx,
                file_count: specs.len(),
                file_name: spec.name,
                bytes_downloaded: completed_bytes.min(total_bytes),
                total_bytes,
            });
        }

        if cancel.is_some_and(|f| f.load(Ordering::Relaxed)) {
            return Err(SyncError::Cancelled);
        }

        // Atomically move verified files from `.staging/` into `dir/`.
        let mut installed: Vec<PathBuf> = Vec::with_capacity(specs.len());
        for spec in specs {
            let src = staging_dir.join(spec.name);
            let dst = dir.join(spec.name);
            if let Err(e) = std::fs::rename(&src, &dst) {
                for path in installed {
                    let _ = std::fs::remove_file(path);
                }
                return Err(SyncError::Io(format!("install {}: {e}", dst.display())));
            }
            installed.push(dst);
        }
        Ok(())
    })();

    let _ = std::fs::remove_dir_all(&staging_dir);
    res
}

/// Removes the downloaded model files and any `.staging/` folder from `dir`.
pub fn remove_model(dir: &Path) -> Result<(), SyncError> {
    let staging = dir.join(".staging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|e| SyncError::Io(format!("remove {}: {e}", staging.display())))?;
    }
    for spec in MODEL_FILES {
        let path = dir.join(spec.name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(SyncError::Io(format!("remove {}: {e}", path.display()))),
        }
    }
    let _ = std::fs::remove_dir(dir);
    Ok(())
}

/// Preprocesses a PNG image into a normalized CHW `[1, 3, 384, 384]` `f32` buffer:
/// decoded to RGB, resized to `384x384` (Catmull-Rom / bicubic), and normalized via `(v / 255.0 - 0.5) / 0.5`.
pub fn preprocess_png(png_bytes: &[u8]) -> Result<Vec<f32>, OcrError> {
    let img = image::load_from_memory(png_bytes).map_err(|_| OcrError::Picture)?;
    let rgb = img.to_rgb8();
    let resized = image::imageops::resize(&rgb, IMG_SIZE, IMG_SIZE, image::imageops::FilterType::CatmullRom);
    let plane = (IMG_SIZE as usize) * (IMG_SIZE as usize);
    let mut out = vec![0.0f32; 3 * plane];
    for (idx, pixel) in resized.pixels().enumerate() {
        let [r, g, b] = pixel.0;
        out[idx] = (f32::from(r) / 255.0 - 0.5) / 0.5;
        out[plane + idx] = (f32::from(g) / 255.0 - 0.5) / 0.5;
        out[2 * plane + idx] = (f32::from(b) / 255.0 - 0.5) / 0.5;
    }
    Ok(out)
}

/// Pure-Rust Hugging Face ByteLevel BPE decoder reading `tokenizer.json`.
#[derive(Clone, Debug)]
pub struct BpeTokenizer {
    id_to_token: HashMap<u32, String>,
    special_ids: HashSet<u32>,
    unicode_to_byte: HashMap<char, u8>,
}

impl BpeTokenizer {
    /// Parses a Hugging Face `tokenizer.json` payload.
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, String> {
        let root: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| format!("invalid tokenizer.json: {e}"))?;
        let mut id_to_token = HashMap::new();
        let mut special_ids: HashSet<u32> = [0, 1, 2, 3, 4].into_iter().collect();

        if let Some(vocab) = root.get("model").and_then(|m| m.get("vocab")).and_then(|v| v.as_object()) {
            for (token, id_val) in vocab {
                if let Some(id) = id_val.as_u64().and_then(|n| u32::try_from(n).ok()) {
                    id_to_token.insert(id, token.clone());
                }
            }
        }
        if let Some(added) = root.get("added_tokens").and_then(|a| a.as_array()) {
            for item in added {
                if let Some(id) = item.get("id").and_then(|v| v.as_u64()).and_then(|n| u32::try_from(n).ok()) {
                    if let Some(content) = item.get("content").and_then(|v| v.as_str()) {
                        id_to_token.insert(id, content.to_string());
                    }
                    if item.get("special").and_then(|v| v.as_bool()).unwrap_or(false) {
                        special_ids.insert(id);
                    }
                }
            }
        }
        if id_to_token.is_empty() {
            return Err("tokenizer.json contains an empty vocabulary".into());
        }
        Ok(Self { id_to_token, special_ids, unicode_to_byte: build_unicode_to_byte_table() })
    }

    /// Decodes a sequence of generated token IDs into UTF-8 LaTeX, skipping special tokens `0..=4`
    /// and any tokenizer-declared special tokens, reversing GPT-2 ByteLevel Unicode mapping, and
    /// trimming leading/trailing whitespace (such as `add_prefix_space`).
    #[must_use]
    pub fn decode(&self, token_ids: &[u32]) -> String {
        let mut raw_bytes: Vec<u8> = Vec::new();
        for &id in token_ids {
            if id == EOS_TOKEN_ID {
                break;
            }
            if id <= 4 || self.special_ids.contains(&id) {
                continue;
            }
            let Some(piece) = self.id_to_token.get(&id) else {
                continue;
            };
            for ch in piece.chars() {
                if let Some(&b) = self.unicode_to_byte.get(&ch) {
                    raw_bytes.push(b);
                } else {
                    let mut buf = [0u8; 4];
                    raw_bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
            }
        }
        String::from_utf8_lossy(&raw_bytes).trim().to_string()
    }
}

/// Builds the exact reverse mapping of GPT-2 / Hugging Face `bytes_to_unicode()`.
fn build_unicode_to_byte_table() -> HashMap<char, u8> {
    let mut bs: Vec<u8> = Vec::with_capacity(256);
    bs.extend(b'!'..=b'~');
    bs.extend(0xA1u8..=0xACu8);
    bs.extend(0xAEu8..=0xFFu8);
    let mut cs: Vec<u32> = bs.iter().map(|&b| u32::from(b)).collect();
    let mut n = 0u32;
    for b in 0u8..=255u8 {
        if !bs.contains(&b) {
            bs.push(b);
            cs.push(256 + n);
            n += 1;
        }
    }
    let mut map = HashMap::with_capacity(256);
    for (b, code) in bs.into_iter().zip(cs) {
        if let Some(ch) = char::from_u32(code) {
            map.insert(ch, b);
        }
    }
    map
}

/// Runs pure-Rust Pix2Text MFR 1.5 ONNX inference on `png_bytes` using the model files installed
/// in `model_dir`, returning the decoded LaTeX string.
pub fn recognize_png(model_dir: &Path, png_bytes: &[u8]) -> Result<String, OcrError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        inference::run_inference(model_dir, png_bytes)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (model_dir, png_bytes);
        Err(OcrError::Unsupported)
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod inference {
    use super::{
        BpeTokenizer, DECODER_FILE, ENCODER_FILE, EOS_TOKEN_ID, IMG_SIZE, MAX_DECODE_TOKENS, START_TOKEN_ID, TOKENIZER_FILE, preprocess_png,
    };
    use crate::ocr::OcrError;
    use std::path::Path;
    use std::sync::Arc;
    use tract_onnx::prelude::*;

    pub(super) fn run_inference(model_dir: &Path, png_bytes: &[u8]) -> Result<String, OcrError> {
        let tokenizer_bytes = std::fs::read(model_dir.join(TOKENIZER_FILE)).map_err(|_| OcrError::NotConfigured("the local math model"))?;
        let tokenizer = BpeTokenizer::from_json_bytes(&tokenizer_bytes).map_err(|_| OcrError::BadReply)?;

        let pixel_data = preprocess_png(png_bytes)?;
        let pixel_tensor: Tensor =
            tract_ndarray::Array4::from_shape_vec((1, 3, IMG_SIZE as usize, IMG_SIZE as usize), pixel_data).map_err(|_| OcrError::Picture)?.into();

        let mut encoder_proto =
            tract_onnx::onnx().model_for_path(model_dir.join(ENCODER_FILE)).map_err(|e| OcrError::Transport(format!("load encoder ONNX: {e}")))?;
        encoder_proto
            .set_input_fact(0, InferenceFact::dt_shape(f32::datum_type(), tvec![1, 3, IMG_SIZE as usize, IMG_SIZE as usize]))
            .map_err(|e| OcrError::Transport(format!("configure encoder input: {e}")))?;
        let encoder =
            encoder_proto.into_optimized().and_then(|m| m.into_runnable()).map_err(|e| OcrError::Transport(format!("optimize encoder ONNX: {e}")))?;

        let encoder_outputs = encoder.run(tvec!(pixel_tensor.into())).map_err(|e| OcrError::Transport(format!("run encoder ONNX: {e}")))?;
        let encoder_hidden = encoder_outputs.first().ok_or(OcrError::BadReply)?.clone().into_tensor();
        let enc_shape = encoder_hidden.shape().to_vec();

        let mut decoder_proto =
            tract_onnx::onnx().model_for_path(model_dir.join(DECODER_FILE)).map_err(|e| OcrError::Transport(format!("load decoder ONNX: {e}")))?;

        let outlets = decoder_proto.input_outlets().map_err(|e| OcrError::Transport(format!("decoder inputs: {e}")))?.to_vec();
        let input_count = outlets.len();
        if input_count < 2 {
            return Err(OcrError::Transport(format!("decoder ONNX expected at least 2 inputs, found {input_count}")));
        }

        let mut input_ids_slot: Option<usize> = None;
        let mut enc_hidden_slot: Option<usize> = None;
        let mut enc_mask_slot: Option<usize> = None;
        for (idx, outlet) in outlets.iter().enumerate() {
            let name = decoder_proto.node(outlet.node).name.to_ascii_lowercase();
            if name.contains("input_ids") {
                input_ids_slot = Some(idx);
            } else if name.contains("encoder_hidden") {
                enc_hidden_slot = Some(idx);
            } else if name.contains("attention_mask") {
                enc_mask_slot = Some(idx);
            }
        }
        let input_ids_slot = input_ids_slot.unwrap_or(0);
        let enc_hidden_slot = enc_hidden_slot.unwrap_or(if input_ids_slot == 0 { 1 } else { 0 });

        let seq_sym = decoder_proto.symbols.sym("sequence_length");
        let ids_shape: TVec<TDim> = tvec![1i64.into(), seq_sym.into()];
        decoder_proto
            .set_input_fact(input_ids_slot, InferenceFact::dt_shape(i64::datum_type(), ids_shape))
            .map_err(|e| OcrError::Transport(format!("configure decoder input_ids: {e}")))?;
        let enc_dims: TVec<TDim> = enc_shape.iter().map(|&d| (d as i64).into()).collect();
        decoder_proto
            .set_input_fact(enc_hidden_slot, InferenceFact::dt_shape(f32::datum_type(), enc_dims))
            .map_err(|e| OcrError::Transport(format!("configure decoder encoder_hidden_states: {e}")))?;
        let enc_seq = enc_shape.get(1).copied().unwrap_or(1);
        if let Some(mask_slot) = enc_mask_slot {
            decoder_proto
                .set_input_fact(mask_slot, InferenceFact::dt_shape(i64::datum_type(), tvec![1, enc_seq]))
                .map_err(|e| OcrError::Transport(format!("configure decoder attention mask: {e}")))?;
        }

        let decoder =
            decoder_proto.into_optimized().and_then(|m| m.into_runnable()).map_err(|e| OcrError::Transport(format!("optimize decoder ONNX: {e}")))?;

        let enc_hidden_tv = TValue::from_const(Arc::new(encoder_hidden));
        let enc_mask_tv: Option<TValue> =
            enc_mask_slot.map(|_| TValue::from_const(Arc::new(tract_ndarray::Array2::<i64>::ones((1, enc_seq)).into_tensor())));

        let mut token_ids: Vec<u32> = vec![START_TOKEN_ID];
        for _step in 0..MAX_DECODE_TOKENS {
            let ids_i64: Vec<i64> = token_ids.iter().map(|&id| i64::from(id)).collect();
            let ids_tensor: Tensor = tract_ndarray::Array2::from_shape_vec((1, ids_i64.len()), ids_i64).map_err(|_| OcrError::BadReply)?.into();

            let mut inputs: TVec<TValue> = tvec![Tensor::default().into(); input_count];
            inputs[input_ids_slot] = ids_tensor.into();
            inputs[enc_hidden_slot] = enc_hidden_tv.clone();
            if let (Some(mask_slot), Some(mask_tv)) = (enc_mask_slot, &enc_mask_tv) {
                inputs[mask_slot] = mask_tv.clone();
            }

            let outputs = decoder.run(inputs).map_err(|e| OcrError::Transport(format!("run decoder ONNX: {e}")))?;
            let logits_tv = outputs.first().ok_or(OcrError::BadReply)?;
            let logits = logits_tv.to_plain_array_view::<f32>().map_err(|e| OcrError::Transport(format!("read decoder logits: {e}")))?;
            let vocab_size = *logits.shape().last().ok_or(OcrError::BadReply)?;
            if vocab_size == 0 {
                return Err(OcrError::BadReply);
            }
            let flat = logits.as_slice().ok_or(OcrError::BadReply)?;
            let last_offset = flat.len().checked_sub(vocab_size).ok_or(OcrError::BadReply)?;
            let last_logits = &flat[last_offset..];

            let mut best_id = 0usize;
            let mut best_val = f32::NEG_INFINITY;
            for (vid, &val) in last_logits.iter().enumerate() {
                if val.is_finite() && val > best_val {
                    best_val = val;
                    best_id = vid;
                }
            }
            let next_id = u32::try_from(best_id).map_err(|_| OcrError::BadReply)?;
            if next_id == EOS_TOKEN_ID {
                break;
            }
            token_ids.push(next_id);
        }

        Ok(tokenizer.decode(&token_ids))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::{SyncHttpRequest, SyncHttpResponse, sha256_bytes};
    use image::codecs::png::PngEncoder;
    use image::{ExtendedColorType, ImageEncoder};
    use nocturne_store::testing::TempDir;
    use std::collections::BTreeMap;

    struct InProcessHttp {
        files: BTreeMap<String, Vec<u8>>,
        chunk_size: usize,
        cancel_after_bytes: Option<(u64, std::sync::Arc<AtomicBool>)>,
    }

    impl SyncHttpClient for InProcessHttp {
        fn request(&self, req: SyncHttpRequest) -> Result<SyncHttpResponse, SyncError> {
            match self.files.get(&req.url) {
                Some(bytes) => Ok(SyncHttpResponse { status: 200, headers: Vec::new(), body: bytes.clone() }),
                None => Ok(SyncHttpResponse { status: 404, headers: Vec::new(), body: Vec::new() }),
            }
        }

        fn download_to_file(
            &self,
            url: &str,
            dest: &Path,
            cancel: Option<&AtomicBool>,
            on_progress: &mut dyn FnMut(u64, Option<u64>),
        ) -> Result<u64, SyncError> {
            use std::io::Write;
            let Some(bytes) = self.files.get(url) else {
                return Err(SyncError::Protocol(format!("download {url} failed with HTTP 404")));
            };
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| SyncError::Io(e.to_string()))?;
            }
            let mut file = std::fs::File::create(dest).map_err(|e| SyncError::Io(e.to_string()))?;
            let total = bytes.len() as u64;
            let mut written = 0u64;
            for chunk in bytes.chunks(self.chunk_size.max(1)) {
                if cancel.is_some_and(|f| f.load(Ordering::Relaxed)) {
                    drop(file);
                    let _ = std::fs::remove_file(dest);
                    return Err(SyncError::Cancelled);
                }
                file.write_all(chunk).map_err(|e| SyncError::Io(e.to_string()))?;
                written += chunk.len() as u64;
                on_progress(written, Some(total));
                if let Some((limit, flag)) = &self.cancel_after_bytes
                    && written >= *limit
                {
                    flag.store(true, Ordering::Relaxed);
                }
            }
            file.flush().map_err(|e| SyncError::Io(e.to_string()))?;
            Ok(written)
        }
    }

    #[test]
    fn model_constants_match_pinned_total_bytes() {
        let sum: u64 = MODEL_FILES.iter().map(|f| f.size).sum();
        assert_eq!(sum, TOTAL_MODEL_BYTES);
        assert_eq!(TOTAL_MODEL_BYTES, 119_650_191);
        assert!(MODEL_BASE_URL.ends_with('/'));
        assert!(MODEL_BASE_URL.contains(MODEL_COMMIT));
    }

    #[test]
    fn bpe_tokenizer_decodes_byte_level_tokens_and_skips_specials() {
        // '\u{0120}' ('Ġ') maps to ASCII space 0x20 in GPT-2 ByteLevel BPE;
        // '\u{010A}' ('Ċ') maps to newline 0x0A.
        let tok_json = serde_json::json!({
            "model": {
                "vocab": {
                    "<pad>": 0,
                    "<s>": 1,
                    "</s>": 2,
                    "<unk>": 3,
                    "<mask>": 4,
                    "\u{0120}\\frac": 10,
                    "{1}": 11,
                    "{2}": 12,
                    "\u{0120}+": 13,
                    "\u{0120}x^2": 14
                }
            },
            "added_tokens": [
                { "id": 99, "content": "<custom_special>", "special": true }
            ]
        });
        let tok = BpeTokenizer::from_json_bytes(tok_json.to_string().as_bytes()).expect("parse tokenizer");
        let decoded = tok.decode(&[1, 99, 10, 11, 12, 13, 14, 2, 14]);
        assert_eq!(decoded, "\\frac{1}{2} + x^2");
    }

    #[test]
    fn preprocess_png_outputs_normalized_chw_384x384_tensor() {
        // 2x2 black & white test PNG
        let mut png = Vec::new();
        let pixels: [u8; 4] = [0, 255, 255, 0];
        PngEncoder::new(&mut png).write_image(&pixels, 2, 2, ExtendedColorType::L8).expect("encode png");
        let tensor = preprocess_png(&png).expect("preprocess");
        assert_eq!(tensor.len(), 3 * 384 * 384);
        assert!(tensor.iter().all(|v| v.is_finite() && *v >= -1.0001 && *v <= 1.0001));
        // Solid black image -> all -1.0; solid white image -> all +1.0
        let mut black_png = Vec::new();
        PngEncoder::new(&mut black_png).write_image(&[0u8; 16], 4, 4, ExtendedColorType::L8).expect("black");
        let black = preprocess_png(&black_png).expect("black tensor");
        assert!(black.iter().all(|v| (*v + 1.0).abs() < 1e-5));

        let mut white_png = Vec::new();
        PngEncoder::new(&mut white_png).write_image(&[255u8; 16], 4, 4, ExtendedColorType::L8).expect("white");
        let white = preprocess_png(&white_png).expect("white tensor");
        assert!(white.iter().all(|v| (*v - 1.0).abs() < 1e-5));
    }

    #[test]
    fn download_verifies_sha256_stages_atomically_cancels_and_removes() {
        let t = TempDir::new("local-model-download");
        let model_dir = t.path().join("models").join(MODEL_ID);
        let base = "https://example.test/mfr/";

        let f1 = b"encoder-bytes-0123456789".to_vec();
        let f2 = b"decoder-bytes-abcdef".to_vec();
        let f3 = br#"{"model":{"vocab":{"a":5}}}"#.to_vec();
        let sha1: &'static str = Box::leak(sha256_bytes(&f1).into_boxed_str());
        let sha2: &'static str = Box::leak(sha256_bytes(&f2).into_boxed_str());
        let sha3: &'static str = Box::leak(sha256_bytes(&f3).into_boxed_str());
        let specs = [
            ModelFileSpec { name: ENCODER_FILE, size: f1.len() as u64, sha256: sha1 },
            ModelFileSpec { name: DECODER_FILE, size: f2.len() as u64, sha256: sha2 },
            ModelFileSpec { name: TOKENIZER_FILE, size: f3.len() as u64, sha256: sha3 },
        ];

        // 1. Corrupt SHA-256 must fail and leave neither installed files nor .staging behind.
        let mut bad_files = BTreeMap::new();
        bad_files.insert(format!("{base}{ENCODER_FILE}"), b"CORRUPT-bytes-0123456789".to_vec());
        bad_files.insert(format!("{base}{DECODER_FILE}"), f2.clone());
        bad_files.insert(format!("{base}{TOKENIZER_FILE}"), f3.clone());
        let bad_http = InProcessHttp { files: bad_files, chunk_size: 8, cancel_after_bytes: None };
        let err = download_model_from_specs(&bad_http, base, &specs, &model_dir, None, &mut |_| {}).expect_err("sha mismatch");
        assert!(matches!(err, SyncError::Protocol(ref m) if m.contains("SHA-256")), "got: {err:?}");
        assert!(!model_dir.join(".staging").exists(), ".staging cleaned up on hash failure");
        assert_eq!(model_status_for_specs(&model_dir, &specs), ModelStatus::NotInstalled);

        // 2. Cancellation mid-stream must return Cancelled and clean up .staging.
        let cancel_flag = std::sync::Arc::new(AtomicBool::new(false));
        let mut ok_files = BTreeMap::new();
        ok_files.insert(format!("{base}{ENCODER_FILE}"), f1.clone());
        ok_files.insert(format!("{base}{DECODER_FILE}"), f2.clone());
        ok_files.insert(format!("{base}{TOKENIZER_FILE}"), f3.clone());
        let cancel_http =
            InProcessHttp { files: ok_files.clone(), chunk_size: 4, cancel_after_bytes: Some((8, std::sync::Arc::clone(&cancel_flag))) };
        let err = download_model_from_specs(&cancel_http, base, &specs, &model_dir, Some(&cancel_flag), &mut |_| {}).expect_err("cancelled");
        assert_eq!(err, SyncError::Cancelled);
        assert!(!model_dir.join(".staging").exists(), ".staging cleaned up on cancel");
        assert_eq!(model_status_for_specs(&model_dir, &specs), ModelStatus::NotInstalled);

        // 3. Clean download succeeds, reports monotonic progress, installs atomically, and removes cleanly.
        let ok_http = InProcessHttp { files: ok_files, chunk_size: 5, cancel_after_bytes: None };
        let mut seen_progress = Vec::new();
        download_model_from_specs(&ok_http, base, &specs, &model_dir, None, &mut |p| seen_progress.push(p)).expect("install ok");
        assert!(!model_dir.join(".staging").exists());
        let total = (f1.len() + f2.len() + f3.len()) as u64;
        assert_eq!(model_status_for_specs(&model_dir, &specs), ModelStatus::Installed { total_bytes: total });
        assert!(seen_progress.last().is_some_and(|p| p.bytes_downloaded == total && p.total_bytes == total));
        assert_eq!(std::fs::read(model_dir.join(ENCODER_FILE)).expect("read encoder"), f1);
        assert_eq!(std::fs::read(model_dir.join(DECODER_FILE)).expect("read decoder"), f2);
        assert_eq!(std::fs::read(model_dir.join(TOKENIZER_FILE)).expect("read tokenizer"), f3);

        // 4. Removal cleans up all model files.
        remove_model(&model_dir).expect("remove model");
        assert_eq!(model_status_for_specs(&model_dir, &specs), ModelStatus::NotInstalled);
    }
}
