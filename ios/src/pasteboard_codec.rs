//! Pure-Rust codecs for `UIPasteboard` image and HTML payloads on iOS.
//!
//! Kept platform-neutral so host unit tests exercise the exact validation and
//! decoding logic used by the iOS pasteboard bridge.

use nocturne_ui_egui::clipboard::Picture;
use std::io::Cursor;

/// Maximum width or height accepted from the pasteboard, matching `crates/ui-egui/src/clipboard.rs`.
const MAX_SIDE: u32 = 20_000;

/// Validates and decodes an image payload into a PNG [`Picture`].
///
/// Rejects empty, malformed, zero-sized, or oversized (`> 20_000` on either side)
/// images. When the input is already PNG, its original bytes are kept without
/// re-encoding; other supported raster formats are re-encoded as PNG.
pub fn decode_png_picture(png_bytes: &[u8]) -> Option<Picture> {
    if png_bytes.is_empty() {
        return None;
    }
    let decoded = image::load_from_memory(png_bytes).ok()?;
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    let png = if matches!(image::guess_format(png_bytes), Ok(image::ImageFormat::Png)) {
        png_bytes.to_vec()
    } else {
        let mut encoded = Vec::new();
        decoded.write_to(&mut Cursor::new(&mut encoded), image::ImageFormat::Png).ok()?;
        encoded
    };
    Some(Picture { png, width, height })
}

/// Decodes a UTF-8 HTML payload from the pasteboard, stripping trailing NUL bytes.
///
/// Returns `None` when the payload is empty, whitespace-only, or invalid UTF-8.
pub fn decode_utf8_html(bytes: &[u8]) -> Option<String> {
    let last_non_nul = bytes.iter().rposition(|&b| b != 0)?;
    let trimmed = bytes.get(..=last_non_nul)?;
    let text = std::str::from_utf8(trimmed).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    Some(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};

    fn encode_rgba_png(width: u32, height: u32) -> Vec<u8> {
        let mut img = RgbaImage::from_pixel(width, height, Rgba([10, 20, 30, 255]));
        if width > 1 && height > 1 {
            img.put_pixel(1, 1, Rgba([200, 100, 50, 128]));
        }
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(img).write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png).expect("encode png");
        bytes
    }

    #[test]
    fn decodes_valid_two_by_two_png_picture() {
        let png_bytes = encode_rgba_png(2, 2);
        let picture = decode_png_picture(&png_bytes).expect("valid 2x2 png");
        assert_eq!((picture.width, picture.height), (2, 2));
        assert_eq!(picture.png, png_bytes);
    }

    #[test]
    fn reencodes_non_png_image_to_png() {
        let img = RgbImage::from_pixel(2, 2, Rgb([255, 0, 128]));
        let mut jpeg_bytes = Vec::new();
        DynamicImage::ImageRgb8(img).write_to(&mut Cursor::new(&mut jpeg_bytes), ImageFormat::Jpeg).expect("encode jpeg");

        let picture = decode_png_picture(&jpeg_bytes).expect("valid jpeg");
        assert_eq!((picture.width, picture.height), (2, 2));
        assert_eq!(image::guess_format(&picture.png).expect("png output"), ImageFormat::Png);
    }

    #[test]
    fn rejects_invalid_and_oversized_image_bytes() {
        assert!(decode_png_picture(&[]).is_none());
        assert!(decode_png_picture(b"not an image at all").is_none());

        let oversized = encode_rgba_png(MAX_SIDE + 1, 1);
        assert!(decode_png_picture(&oversized).is_none());
    }

    #[test]
    fn decodes_utf8_html_and_strips_trailing_nul_bytes() {
        assert_eq!(decode_utf8_html(b"<p>Hello</p>\0\0").as_deref(), Some("<p>Hello</p>"));
        assert_eq!(decode_utf8_html("<p>Caf\u{00e9}</p>\0".as_bytes()).as_deref(), Some("<p>Caf\u{00e9}</p>"));
        assert_eq!(decode_utf8_html(b""), None);
        assert_eq!(decode_utf8_html(b"\0\0\0"), None);
        assert_eq!(decode_utf8_html(b"   \n\t\0"), None);
        assert_eq!(decode_utf8_html(b"\xff\xfe<p>bad</p>\0"), None);
    }
}
