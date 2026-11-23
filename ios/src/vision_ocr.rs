//! Apple Vision (`VNRecognizeTextRequest`) on-device OCR backend for iPadOS / iOS.
//!
//! Supports both 2D handwritten math-to-LaTeX recognition ([`OcrMode::MathLatex`]) and
//! general handwriting-to-text recognition ([`OcrMode::HandwritingText`]).
//!
//! Pure-Rust input validation, PNG header dimension parsing, UTF-16 math token range splitting,
//! bounding-box conversion, and mode-aware reconstruction helpers live at module top level so
//! they compile and run under `cargo test -p nocturne-ios` on all hosts. The `objc2-vision`
//! bridge lives inside `#[cfg(target_os = "ios")]`.

use nocturne_engine::ocr::OcrError;
use nocturne_engine::ocr::vision_post::{observations_to_latex_with_aspect, observations_to_text};
use nocturne_engine::{NativeOcrBackend, OcrMode, VisionObservation};

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

/// Extracts `(width, height)` from a PNG payload's `IHDR` header (bytes `16..24`, big-endian `u32`).
///
/// Returns `None` when the buffer is shorter than 24 bytes, lacks the PNG signature or `IHDR`
/// chunk tag, or encodes a zero width or height.
#[must_use]
pub fn png_dimensions(png_bytes: &[u8]) -> Option<(u32, u32)> {
    if png_bytes.get(..PNG_MAGIC.len()) != Some(&PNG_MAGIC[..]) || png_bytes.get(12..16) != Some(b"IHDR".as_slice()) {
        return None;
    }
    let w_bytes: [u8; 4] = png_bytes.get(16..20)?.try_into().ok()?;
    let h_bytes: [u8; 4] = png_bytes.get(20..24)?.try_into().ok()?;
    let width = u32::from_be_bytes(w_bytes);
    let height = u32::from_be_bytes(h_bytes);
    (width > 0 && height > 0).then_some((width, height))
}

/// Resolves effective `(width, height)` image dimensions, falling back to the PNG `IHDR` header
/// whenever either `width` or `height` is `0`.
#[must_use]
pub fn resolve_png_dimensions(png_bytes: &[u8], width: u32, height: u32) -> (u32, u32) {
    if width > 0 && height > 0 { (width, height) } else { png_dimensions(png_bytes).unwrap_or((0, 0)) }
}

/// Resolves the width-to-height aspect ratio for `png_bytes`, falling back to the PNG `IHDR`
/// header when `width == 0 || height == 0` and defaulting to `1.0` when dimensions are unavailable.
#[must_use]
pub fn resolve_image_aspect(png_bytes: &[u8], width: u32, height: u32) -> f32 {
    let (w, h) = resolve_png_dimensions(png_bytes, width, height);
    if w > 0 && h > 0 { w as f32 / h.max(1) as f32 } else { 1.0 }
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

/// One math token slice paired with its UTF-16 `NSRange` offset and length within the parent line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MathTokenRange<'a> {
    pub text: &'a str,
    /// Zero-based UTF-16 code-unit offset (`NSRange.location`).
    pub utf16_start: usize,
    /// UTF-16 code-unit length (`NSRange.length`).
    pub utf16_len: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MathTokenClass {
    Radical,
    Word,
    Number,
    Scripted,
}

fn is_math_operator_char(c: char) -> bool {
    matches!(
        c,
        '+' | '-'
            | '='
            | '<'
            | '>'
            | '*'
            | '/'
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '|'
            | ','
            | ';'
            | ':'
            | '−'
            | '–'
            | '—'
            | '×'
            | '÷'
            | '±'
            | '∓'
            | '·'
            | '⋅'
            | '≤'
            | '≥'
            | '≠'
            | '≈'
            | '≡'
            | '∝'
            | '∼'
            | '≅'
            | '≪'
            | '≫'
            | '∑'
            | '∏'
            | '∫'
            | '∮'
            | '∈'
            | '∉'
            | '⊂'
            | '⊃'
            | '⊆'
            | '⊇'
            | '∪'
            | '∩'
            | '→'
            | '←'
            | '↔'
            | '⇒'
            | '⇐'
            | '⇔'
            | '⊥'
            | '∥'
    )
}

fn is_two_char_ascii_operator(first: char, second: char) -> bool {
    matches!((first, second), ('<', '=') | ('>', '=') | ('!', '=') | ('-', '>') | ('=', '>'))
}

/// Splits a recognized math line into per-token slices with exact UTF-16 `NSRange` offsets for
/// `VNRecognizedText::boundingBoxForRange_error`.
///
/// Splits on whitespace, math operators/delimiters (preserving two-character ASCII relations like
/// `<=`, `>=`, `!=`, `->`, `=>`), and unscripted letter/digit transitions (so Vision outputs such
/// as `x2` receive distinct sub-boxes for superscript/subscript detection), while keeping Unicode
/// super/subscripts (`x²`), explicit `^`/`_` scripts (`x^2`, `a_1`), decimal numbers (`3.14`), and
/// radicals (`√2`, `√x`) intact.
#[must_use]
pub fn split_math_token_ranges<'a>(text: &'a str) -> Vec<MathTokenRange<'a>> {
    let mut tokens: Vec<MathTokenRange<'a>> = Vec::new();
    let mut cur: Option<(usize, usize, MathTokenClass)> = None;
    let mut utf16_pos = 0usize;
    let mut iter = text.char_indices().peekable();

    let flush = |tokens: &mut Vec<MathTokenRange<'a>>, cur: &mut Option<(usize, usize, MathTokenClass)>, byte_end: usize, utf16_end: usize| {
        if let Some((byte_start, utf16_start, _)) = cur.take() {
            let utf16_len = utf16_end.saturating_sub(utf16_start);
            if utf16_len > 0
                && let Some(slice) = text.get(byte_start..byte_end)
            {
                tokens.push(MathTokenRange { text: slice, utf16_start, utf16_len });
            }
        }
    };

    while let Some((byte_idx, ch)) = iter.next() {
        let ch_utf16 = ch.len_utf16();

        if ch.is_whitespace() {
            flush(&mut tokens, &mut cur, byte_idx, utf16_pos);
            utf16_pos += ch_utf16;
            continue;
        }

        if let Some(&(_, next_ch)) = iter.peek()
            && is_two_char_ascii_operator(ch, next_ch)
        {
            flush(&mut tokens, &mut cur, byte_idx, utf16_pos);
            let _ = iter.next();
            let op_utf16 = ch_utf16 + next_ch.len_utf16();
            let op_end = byte_idx + ch.len_utf8() + next_ch.len_utf8();
            if let Some(slice) = text.get(byte_idx..op_end) {
                tokens.push(MathTokenRange { text: slice, utf16_start: utf16_pos, utf16_len: op_utf16 });
            }
            utf16_pos += op_utf16;
            continue;
        }

        if is_math_operator_char(ch) {
            flush(&mut tokens, &mut cur, byte_idx, utf16_pos);
            let op_end = byte_idx + ch.len_utf8();
            if let Some(slice) = text.get(byte_idx..op_end) {
                tokens.push(MathTokenRange { text: slice, utf16_start: utf16_pos, utf16_len: ch_utf16 });
            }
            utf16_pos += ch_utf16;
            continue;
        }

        if ch == '^' || ch == '_' {
            cur = match cur {
                Some((bs, us, _)) => Some((bs, us, MathTokenClass::Scripted)),
                None => Some((byte_idx, utf16_pos, MathTokenClass::Scripted)),
            };
            utf16_pos += ch_utf16;
            continue;
        }

        if ch == '√' {
            flush(&mut tokens, &mut cur, byte_idx, utf16_pos);
            cur = Some((byte_idx, utf16_pos, MathTokenClass::Radical));
            utf16_pos += ch_utf16;
            continue;
        }

        if ch.is_ascii_digit() {
            match cur {
                None => cur = Some((byte_idx, utf16_pos, MathTokenClass::Number)),
                Some((_, _, MathTokenClass::Number | MathTokenClass::Scripted)) => {}
                Some((bs, us, MathTokenClass::Radical)) => cur = Some((bs, us, MathTokenClass::Number)),
                Some((_, _, MathTokenClass::Word)) => {
                    flush(&mut tokens, &mut cur, byte_idx, utf16_pos);
                    cur = Some((byte_idx, utf16_pos, MathTokenClass::Number));
                }
            }
            utf16_pos += ch_utf16;
            continue;
        }

        if ch == '.' {
            if cur.is_none() {
                cur = Some((byte_idx, utf16_pos, MathTokenClass::Word));
            }
            utf16_pos += ch_utf16;
            continue;
        }

        if ch.is_alphabetic() {
            match cur {
                None => cur = Some((byte_idx, utf16_pos, MathTokenClass::Word)),
                Some((_, _, MathTokenClass::Word | MathTokenClass::Scripted)) => {}
                Some((bs, us, MathTokenClass::Radical)) => cur = Some((bs, us, MathTokenClass::Word)),
                Some((_, _, MathTokenClass::Number)) => {
                    flush(&mut tokens, &mut cur, byte_idx, utf16_pos);
                    cur = Some((byte_idx, utf16_pos, MathTokenClass::Word));
                }
            }
            utf16_pos += ch_utf16;
            continue;
        }

        // Attached modifiers such as Unicode superscripts/subscripts (`²`, `₁`), primes (`'`, `′`),
        // degrees (`°`), or factorial (`!`) stay grouped with the active token.
        if cur.is_none() {
            cur = Some((byte_idx, utf16_pos, MathTokenClass::Word));
        }
        utf16_pos += ch_utf16;
    }

    flush(&mut tokens, &mut cur, text.len(), utf16_pos);
    tokens
}

/// Reconstructs recognized LaTeX or plain handwriting text from Apple Vision observations for
/// the requested [`OcrMode`].
///
/// Uses `width / height` (defaulting to `1.0` when either dimension is `0`) for aspect-aware 2D
/// layout in [`OcrMode::MathLatex`], and reading-order text layout in [`OcrMode::HandwritingText`].
#[must_use]
pub fn reconstruct_observations(observations: &[VisionObservation], width: u32, height: u32, mode: OcrMode) -> String {
    match mode {
        OcrMode::MathLatex => {
            let aspect = if width > 0 && height > 0 { width as f32 / height.max(1) as f32 } else { 1.0 };
            observations_to_latex_with_aspect(observations, aspect)
        }
        OcrMode::HandwritingText => observations_to_text(observations),
    }
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
            imp::recognize_png(png_bytes, OcrMode::MathLatex)
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = png_bytes;
            Err(OcrError::Unsupported)
        }
    }

    fn recognize_png_with_mode(&self, png_bytes: &[u8], mode: OcrMode) -> Result<Vec<VisionObservation>, OcrError> {
        #[cfg(target_os = "ios")]
        {
            imp::recognize_png(png_bytes, mode)
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = (png_bytes, mode);
            Err(OcrError::Unsupported)
        }
    }
}

#[cfg(target_os = "ios")]
mod imp {
    use super::{convert_vision_candidate, split_math_token_ranges, validate_png_input};
    use nocturne_engine::ocr::OcrError;
    use nocturne_engine::{OcrMode, VisionObservation};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{AnyThread, MainThreadMarker, available};
    use objc2_foundation::{NSArray, NSData, NSDictionary, NSRange};
    use objc2_vision::{VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedText, VNRequest, VNRequestTextRecognitionLevel};

    /// Minimum normalized text height so small exponents, subscripts, and operators are kept.
    const MIN_VISION_TEXT_HEIGHT: f32 = 0.01;

    pub(super) fn recognize_png(png_bytes: &[u8], mode: OcrMode) -> Result<Vec<VisionObservation>, OcrError> {
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
            request.setMinimumTextHeight(MIN_VISION_TEXT_HEIGHT);
            match mode {
                OcrMode::MathLatex => {
                    // Disable natural-language dictionary correction so mathematical variables,
                    // operators, and subscripts are not rewritten into English words.
                    request.setUsesLanguageCorrection(false);
                }
                OcrMode::HandwritingText => {
                    request.setUsesLanguageCorrection(true);
                    if available!(ios = 16.0) {
                        request.setAutomaticallyDetectsLanguage(true);
                    }
                }
            }

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
                let ns_text = best.string();
                let text = ns_text.to_string();
                let confidence = best.confidence();

                if mode == OcrMode::MathLatex
                    && let Some(sub_obs) = extract_math_token_observations(&best, &text, ns_text.length(), confidence)
                {
                    observations.extend(sub_obs);
                    continue;
                }

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

    fn extract_math_token_observations(best: &VNRecognizedText, text: &str, ns_len: usize, confidence: f32) -> Option<Vec<VisionObservation>> {
        let tokens = split_math_token_ranges(text);
        if tokens.len() <= 1 {
            return None;
        }
        let mut out = Vec::with_capacity(tokens.len());
        for tok in tokens {
            if tok.utf16_len == 0 || tok.utf16_start.saturating_add(tok.utf16_len) > ns_len {
                return None;
            }
            let range = NSRange::new(tok.utf16_start, tok.utf16_len);
            #[allow(unsafe_code)]
            // SAFETY: `best` is a valid `VNRecognizedText` candidate returned by `topCandidates`,
            // and `range` has been bounds-checked against `best.string().length()` in UTF-16 units.
            let rect_obs = unsafe { best.boundingBoxForRange_error(range) }.ok()?;
            #[allow(unsafe_code)]
            // SAFETY: `rect_obs` is a valid `VNRectangleObservation` returned by
            // `boundingBoxForRange_error`, and `boundingBox` is a read-only normalized `CGRect`
            // getter on its `VNDetectedObjectObservation` superclass.
            let rect = unsafe { rect_obs.boundingBox() };
            let converted = convert_vision_candidate(
                tok.text,
                rect.origin.x as f32,
                rect.origin.y as f32,
                rect.size.width as f32,
                rect.size.height as f32,
                confidence,
            )?;
            out.push(converted);
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_engine::ocr::observations_to_latex;

    fn sample_png_header(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(24);
        bytes.extend_from_slice(&PNG_MAGIC);
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes
    }

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
    fn extracts_png_ihdr_dimensions_and_aspect_fallback() {
        let wide = sample_png_header(800, 200);
        assert_eq!(png_dimensions(&wide), Some((800, 200)));
        assert_eq!(resolve_png_dimensions(&wide, 0, 0), (800, 200));
        assert_eq!(resolve_png_dimensions(&wide, 640, 0), (800, 200));
        assert_eq!(resolve_png_dimensions(&wide, 0, 480), (800, 200));
        assert_eq!(resolve_png_dimensions(&wide, 320, 160), (320, 160));
        assert!((resolve_image_aspect(&wide, 0, 0) - 4.0).abs() < 1e-5);
        assert!((resolve_image_aspect(&wide, 320, 160) - 2.0).abs() < 1e-5);

        let hd = sample_png_header(1920, 1080);
        assert_eq!(png_dimensions(&hd), Some((1920, 1080)));

        // Rejects truncated, zero-dimension, wrong-signature, or non-IHDR headers.
        assert_eq!(png_dimensions(&[]), None);
        assert_eq!(png_dimensions(&PNG_MAGIC), None);
        assert_eq!(png_dimensions(&wide[..23]), None);
        assert_eq!(png_dimensions(&sample_png_header(0, 200)), None);
        assert_eq!(png_dimensions(&sample_png_header(800, 0)), None);
        let mut wrong_chunk = sample_png_header(800, 200);
        wrong_chunk[12..16].copy_from_slice(b"IDAT");
        assert_eq!(png_dimensions(&wrong_chunk), None);
        assert!((resolve_image_aspect(&PNG_MAGIC, 0, 0) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn splits_math_tokens_with_accurate_utf16_ranges() {
        // Unicode superscripts stay attached to their base variable; operators split cleanly.
        let sup_line = "x² + y² = 1";
        let sup_tokens = split_math_token_ranges(sup_line);
        assert_eq!(
            sup_tokens,
            vec![
                MathTokenRange { text: "x²", utf16_start: 0, utf16_len: 2 },
                MathTokenRange { text: "+", utf16_start: 3, utf16_len: 1 },
                MathTokenRange { text: "y²", utf16_start: 5, utf16_len: 2 },
                MathTokenRange { text: "=", utf16_start: 8, utf16_len: 1 },
                MathTokenRange { text: "1", utf16_start: 10, utf16_len: 1 },
            ]
        );

        // Unscripted ASCII digit after variable splits so Vision sub-boxes capture vertical offset.
        let plain_line = "x2 + y2 = 1";
        let plain_tokens = split_math_token_ranges(plain_line);
        assert_eq!(
            plain_tokens,
            vec![
                MathTokenRange { text: "x", utf16_start: 0, utf16_len: 1 },
                MathTokenRange { text: "2", utf16_start: 1, utf16_len: 1 },
                MathTokenRange { text: "+", utf16_start: 3, utf16_len: 1 },
                MathTokenRange { text: "y", utf16_start: 5, utf16_len: 1 },
                MathTokenRange { text: "2", utf16_start: 6, utf16_len: 1 },
                MathTokenRange { text: "=", utf16_start: 8, utf16_len: 1 },
                MathTokenRange { text: "1", utf16_start: 10, utf16_len: 1 },
            ]
        );

        // Greek letters, Unicode relations, two-char ASCII operators, decimals, radicals, and scripts.
        let greek_line = "α+β≤γ";
        assert_eq!(
            split_math_token_ranges(greek_line),
            vec![
                MathTokenRange { text: "α", utf16_start: 0, utf16_len: 1 },
                MathTokenRange { text: "+", utf16_start: 1, utf16_len: 1 },
                MathTokenRange { text: "β", utf16_start: 2, utf16_len: 1 },
                MathTokenRange { text: "≤", utf16_start: 3, utf16_len: 1 },
                MathTokenRange { text: "γ", utf16_start: 4, utf16_len: 1 },
            ]
        );

        let mixed = "e^x + √2 <= 3.14";
        assert_eq!(
            split_math_token_ranges(mixed),
            vec![
                MathTokenRange { text: "e^x", utf16_start: 0, utf16_len: 3 },
                MathTokenRange { text: "+", utf16_start: 4, utf16_len: 1 },
                MathTokenRange { text: "√2", utf16_start: 6, utf16_len: 2 },
                MathTokenRange { text: "<=", utf16_start: 9, utf16_len: 2 },
                MathTokenRange { text: "3.14", utf16_start: 12, utf16_len: 4 },
            ]
        );

        // Non-BMP mathematical symbols (`𝑥` U+1D465 and `𝜋` U+1D70B occupy 2 UTF-16 code units each).
        let non_bmp = "𝑥² + 𝜋 = 1";
        let non_bmp_tokens = split_math_token_ranges(non_bmp);
        assert_eq!(
            non_bmp_tokens,
            vec![
                MathTokenRange { text: "𝑥²", utf16_start: 0, utf16_len: 3 },
                MathTokenRange { text: "+", utf16_start: 4, utf16_len: 1 },
                MathTokenRange { text: "𝜋", utf16_start: 6, utf16_len: 2 },
                MathTokenRange { text: "=", utf16_start: 9, utf16_len: 1 },
                MathTokenRange { text: "1", utf16_start: 11, utf16_len: 1 },
            ]
        );
        let full_utf16: Vec<u16> = non_bmp.encode_utf16().collect();
        for tok in &non_bmp_tokens {
            let slice = &full_utf16[tok.utf16_start..tok.utf16_start + tok.utf16_len];
            assert_eq!(String::from_utf16(slice).expect("valid utf16 slice"), tok.text);
        }

        assert!(split_math_token_ranges("").is_empty());
        assert!(split_math_token_ranges("   \t\n").is_empty());
    }

    #[test]
    fn reconstructs_math_latex_and_handwriting_text_modes() {
        let math_obs = vec![
            VisionObservation::new("x", 0.10, 0.40, 0.10, 0.25, 0.98),
            VisionObservation::new("2", 0.21, 0.28, 0.06, 0.12, 0.96),
            VisionObservation::new("+", 0.32, 0.44, 0.08, 0.16, 0.99),
            VisionObservation::new("π", 0.44, 0.40, 0.10, 0.25, 0.97),
        ];
        assert_eq!(reconstruct_observations(&math_obs, 800, 200, OcrMode::MathLatex), "x^{2} + \\pi");

        let frac_obs = vec![
            convert_vision_candidate("a", 0.35, 0.65, 0.20, 0.20, 0.94).expect("num"),
            convert_vision_candidate("—", 0.25, 0.45, 0.40, 0.05, 0.88).expect("bar"),
            convert_vision_candidate("b", 0.35, 0.15, 0.20, 0.20, 0.95).expect("den"),
        ];
        assert_eq!(reconstruct_observations(&frac_obs, 400, 400, OcrMode::MathLatex), "\\frac{a}{b}");

        let prose_obs = vec![
            VisionObservation::new("Hello", 0.05, 0.10, 0.20, 0.15, 0.99),
            VisionObservation::new("world", 0.28, 0.10, 0.20, 0.15, 0.99),
            VisionObservation::new("Second line.", 0.05, 0.29, 0.45, 0.15, 0.98),
        ];
        assert_eq!(reconstruct_observations(&prose_obs, 800, 400, OcrMode::HandwritingText), "Hello world\nSecond line.");
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
        assert_eq!(backend.recognize_png_with_mode(&PNG_MAGIC, OcrMode::MathLatex), Err(OcrError::Unsupported));
        assert_eq!(backend.recognize_png_with_mode(&PNG_MAGIC, OcrMode::HandwritingText), Err(OcrError::Unsupported));
    }
}
