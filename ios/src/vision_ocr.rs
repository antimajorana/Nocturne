//! Apple Vision (`VNRecognizeTextRequest`) on-device OCR backend for iPadOS / iOS.
//!
//! Pure-Rust input validation and bounding-box conversion helpers live at module top level so
//! they compile and run under `cargo test -p nocturne-ios` on all hosts. The `objc2-vision`
//! bridge lives inside `#[cfg(target_os = "ios")]`.

use nocturne_engine::ocr::OcrError;
use nocturne_engine::{NativeOcrBackend, VisionObservation};

/// Standard 8-byte PNG file signature.
const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// Minimum Vision recognition confidence accepted for a text observation.
///
/// Single mathematical glyphs (`x`, `+`, `2`, `=`) can legitimately score around `0.2–0.5`,
/// while near-zero noise fragments should be discarded before 2D LaTeX reconstruction.
pub const MIN_VISION_CONFIDENCE: f32 = 0.05;

/// Validates that `png_bytes` is a non-empty PNG payload before passing it to Apple Vision.
pub fn validate_png_input(png_bytes: &[u8]) -> Result<(), OcrError> {
    if png_bytes.len() < PNG_MAGIC.len() || png_bytes[..PNG_MAGIC.len()] != PNG_MAGIC {
        return Err(OcrError::Picture);
    }
    Ok(())
}

/// Filters and converts one Apple Vision candidate (bottom-left normalized `CGRect` coordinates)
/// into a top-left normalized [`VisionObservation`].
///
/// Returns `None` when the text is blank, the bounding box has invalid/non-positive dimensions,
/// or the recognition confidence is non-finite or below [`MIN_VISION_CONFIDENCE`].
#[must_use]
pub fn convert_vision_candidate(text: &str, x: f32, y_bottom: f32, width: f32, height: f32, confidence: f32) -> Option<VisionObservation> {
    let trimmed = text.trim();
    if trimmed.is_empty()
        || !x.is_finite()
        || !y_bottom.is_finite()
        || !width.is_finite()
        || !height.is_finite()
        || !confidence.is_finite()
        || width <= 0.0
        || height <= 0.0
        || confidence < MIN_VISION_CONFIDENCE
    {
        return None;
    }
    Some(VisionObservation::from_bottom_left(trimmed, x, y_bottom, width, height, confidence.clamp(0.0, 1.0)))
}

/// On-device Apple Vision OCR backend implementing [`NativeOcrBackend`].
#[derive(Clone, Copy, Debug, Default)]
pub struct IosVisionOcr {
    _private: (),
}

impl IosVisionOcr {
    /// Creates an Apple Vision OCR backend handle.
    #[must_use]
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl NativeOcrBackend for IosVisionOcr {
    fn is_supported(&self) -> bool {
        cfg!(target_os = "ios")
    }

    fn recognize_png(&self, png_bytes: &[u8]) -> Result<Vec<VisionObservation>, OcrError> {
        #[cfg(target_os = "ios")]
        {
            imp::recognize_png(png_bytes)
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = png_bytes;
            Err(OcrError::Unsupported)
        }
    }
}

#[cfg(target_os = "ios")]
mod imp {
    use super::{convert_vision_candidate, validate_png_input};
    use nocturne_engine::VisionObservation;
    use nocturne_engine::ocr::OcrError;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_foundation::{NSArray, NSData, NSDictionary};
    use objc2_vision::{VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel};

    pub(super) fn recognize_png(png_bytes: &[u8]) -> Result<Vec<VisionObservation>, OcrError> {
        validate_png_input(png_bytes)?;
        if MainThreadMarker::new().is_some() {
            log::debug!("Apple Vision OCR invoked on the main thread");
        }

        objc2::rc::autoreleasepool(|_| {
            let data = NSData::with_bytes(png_bytes);
            let options: Retained<NSDictionary<VNImageOption, AnyObject>> = NSDictionary::new();
            let handler = VNImageRequestHandler::initWithData_options(VNImageRequestHandler::alloc(), &data, &options);

            let request = VNRecognizeTextRequest::new();
            request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
            // Disable natural-language dictionary correction so mathematical variables,
            // operators, and subscripts are not rewritten into English words.
            request.setUsesLanguageCorrection(false);

            let base_request: &VNRequest = &request;
            let requests = NSArray::from_slice(&[base_request]);
            handler.performRequests_error(&requests).map_err(|err| OcrError::Transport(err.localizedDescription().to_string()))?;

            let Some(results) = request.results() else {
                return Ok(Vec::new());
            };

            let count = results.count();
            let mut observations = Vec::with_capacity(count);
            for idx in 0..count {
                let obs = results.objectAtIndex(idx);
                let candidates = obs.topCandidates(1);
                let Some(best) = candidates.firstObject() else {
                    continue;
                };
                let text = best.string().to_string();
                let confidence = best.confidence();
                #[allow(unsafe_code)]
                // SAFETY: `obs` is a valid `VNRecognizedTextObservation` returned by
                // `VNRecognizeTextRequest::results`, and `boundingBox` is a read-only normalized
                // `CGRect` getter on its `VNDetectedObjectObservation` superclass.
                let rect = unsafe { obs.boundingBox() };

                if let Some(converted) = convert_vision_candidate(
                    &text,
                    rect.origin.x as f32,
                    rect.origin.y as f32,
                    rect.size.width as f32,
                    rect.size.height as f32,
                    confidence,
                ) {
                    observations.push(converted);
                }
            }

            Ok(observations)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_engine::ocr::observations_to_latex;

    #[test]
    fn converts_bottom_left_boxes_and_reconstructs_latex() {
        // Vision bottom-left coordinates for a stacked fraction `a` over `b` separated by `-`:
        // numerator near top (y_bottom = 0.65, h = 0.20 -> top-left y = 0.15)
        // bar in middle     (y_bottom = 0.45, h = 0.05 -> top-left y = 0.50)
        // denominator low   (y_bottom = 0.15, h = 0.20 -> top-left y = 0.65)
        let num = convert_vision_candidate(" a ", 0.35, 0.65, 0.20, 0.20, 0.94).expect("valid numerator");
        let bar = convert_vision_candidate("—", 0.25, 0.45, 0.40, 0.05, 0.88).expect("valid bar");
        let den = convert_vision_candidate("b", 0.35, 0.15, 0.20, 0.20, 1.25).expect("valid denominator");

        assert_eq!(num.text, "a");
        assert!((num.y - 0.15).abs() < 1e-5, "expected top-left y=0.15, got {}", num.y);
        assert!((bar.y - 0.50).abs() < 1e-5, "expected top-left y=0.50, got {}", bar.y);
        assert!((den.y - 0.65).abs() < 1e-5, "expected top-left y=0.65, got {}", den.y);
        assert!((den.confidence - 1.0).abs() < 1e-5, "confidence above 1.0 should clamp to 1.0");

        let latex = observations_to_latex(&[num, bar, den]);
        assert_eq!(latex, "\\frac{a}{b}");
    }

    #[test]
    fn filters_blank_low_confidence_and_degenerate_candidates() {
        assert!(convert_vision_candidate("", 0.1, 0.2, 0.2, 0.1, 0.9).is_none());
        assert!(convert_vision_candidate("   \t\n", 0.1, 0.2, 0.2, 0.1, 0.9).is_none());
        assert!(convert_vision_candidate("x", 0.1, 0.2, 0.2, 0.1, 0.01).is_none());
        assert!(convert_vision_candidate("x", 0.1, 0.2, 0.2, 0.1, -0.5).is_none());
        assert!(convert_vision_candidate("x", 0.1, 0.2, 0.2, 0.1, f32::NAN).is_none());
        assert!(convert_vision_candidate("x", 0.1, 0.2, 0.0, 0.1, 0.9).is_none());
        assert!(convert_vision_candidate("x", 0.1, 0.2, 0.2, -0.1, 0.9).is_none());
        assert!(convert_vision_candidate("x", f32::INFINITY, 0.2, 0.2, 0.1, 0.9).is_none());
        assert!(convert_vision_candidate("x", 0.1, f32::NAN, 0.2, 0.1, 0.9).is_none());
        assert!(convert_vision_candidate("x", 0.1, 0.2, 0.2, 0.1, MIN_VISION_CONFIDENCE).is_some());
    }

    #[test]
    fn validates_png_magic_bytes() {
        assert_eq!(validate_png_input(&[]), Err(OcrError::Picture));
        assert_eq!(validate_png_input(b"not a png"), Err(OcrError::Picture));
        assert_eq!(validate_png_input(&PNG_MAGIC[..7]), Err(OcrError::Picture));
        assert_eq!(validate_png_input(&PNG_MAGIC), Ok(()));
    }

    #[cfg(not(target_os = "ios"))]
    #[test]
    fn non_ios_host_reports_unsupported() {
        let backend = IosVisionOcr::new();
        assert!(!backend.is_supported());
        assert_eq!(backend.recognize_png(&PNG_MAGIC), Err(OcrError::Unsupported));
    }
}
