//! `UIPasteboard` bridge implementing [`nocturne_ui_egui::clipboard::Board`] for iPadOS / iOS.

use nocturne_ui_egui::clipboard::{Board, Picture};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString, ns_string};
use objc2_ui_kit::UIPasteboard;

/// System clipboard backed by `UIPasteboard.generalPasteboard`.
#[derive(Default)]
pub struct IosPasteboard;

impl IosPasteboard {
    pub fn new() -> Self {
        Self
    }
}

impl Board for IosPasteboard {
    fn text(&mut self) -> Option<String> {
        let _mtm = MainThreadMarker::new()?;
        let board = UIPasteboard::generalPasteboard();
        #[allow(unsafe_code)]
        // SAFETY: `MainThreadMarker::new()` above guarantees execution on the UIKit main thread,
        // satisfying `UIPasteboard::hasStrings` and `UIPasteboard::string` thread-safety invariants.
        let value = unsafe {
            if !board.hasStrings() {
                return None;
            }
            board.string()?
        };
        let text = value.to_string();
        (!text.is_empty()).then_some(text)
    }

    fn html(&mut self) -> Option<String> {
        let _mtm = MainThreadMarker::new()?;
        let board = UIPasteboard::generalPasteboard();
        let html_type = ns_string!("public.html");
        let html_types = NSArray::from_slice(&[html_type]);
        if !board.containsPasteboardTypes(&html_types) {
            return None;
        }
        if let Some(data) = board.dataForPasteboardType(html_type) {
            return crate::pasteboard_codec::decode_utf8_html(&data.to_vec());
        }
        let value = board.valueForPasteboardType(html_type)?;
        if let Some(data) = value.downcast_ref::<NSData>() {
            return crate::pasteboard_codec::decode_utf8_html(&data.to_vec());
        }
        if let Some(string) = value.downcast_ref::<NSString>() {
            let text = string.to_string();
            return crate::pasteboard_codec::decode_utf8_html(text.as_bytes());
        }
        None
    }

    fn picture(&mut self) -> Option<Picture> {
        let _mtm = MainThreadMarker::new()?;
        let board = UIPasteboard::generalPasteboard();
        let png_type = ns_string!("public.png");
        let png_types = NSArray::from_slice(&[png_type]);
        if board.containsPasteboardTypes(&png_types)
            && let Some(data) = board.dataForPasteboardType(png_type)
            && let Some(picture) = crate::pasteboard_codec::decode_png_picture(&data.to_vec())
        {
            return Some(picture);
        }
        #[allow(unsafe_code)]
        // SAFETY: `MainThreadMarker::new()` above guarantees execution on the UIKit main thread,
        // satisfying `UIPasteboard::hasImages` and `UIPasteboard::image` thread-safety invariants.
        let image = unsafe {
            if !board.hasImages() {
                return None;
            }
            board.image()?
        };
        let png_data = image.png_representation()?;
        crate::pasteboard_codec::decode_png_picture(&png_data.to_vec())
    }

    fn has_picture(&mut self) -> bool {
        let Some(_mtm) = MainThreadMarker::new() else {
            return false;
        };
        let board = UIPasteboard::generalPasteboard();
        let png_types = NSArray::from_slice(&[ns_string!("public.png")]);
        if board.containsPasteboardTypes(&png_types) {
            return true;
        }
        #[allow(unsafe_code)]
        // SAFETY: `MainThreadMarker::new()` above guarantees execution on the UIKit main thread.
        unsafe {
            board.hasImages()
        }
    }

    fn set_text(&mut self, text: &str) -> Result<(), String> {
        let _mtm = MainThreadMarker::new().ok_or_else(|| "iOS pasteboard requires the main thread".to_string())?;
        let board = UIPasteboard::generalPasteboard();
        let value = NSString::from_str(text);
        #[allow(unsafe_code)]
        // SAFETY: `MainThreadMarker::new()` above guarantees execution on the UIKit main thread.
        unsafe {
            board.setString(Some(&value));
        }
        Ok(())
    }

    fn set_html(&mut self, html: &str, text: &str) -> Result<(), String> {
        let _mtm = MainThreadMarker::new().ok_or_else(|| "iOS pasteboard requires the main thread".to_string())?;
        let board = UIPasteboard::generalPasteboard();
        let html_key = ns_string!("public.html");
        let text_key = ns_string!("public.utf8-plain-text");
        let html_data = NSData::with_bytes(html.as_bytes());
        let text_value = NSString::from_str(text);
        let html_obj: &AnyObject = html_data.as_ref();
        let text_obj: &AnyObject = text_value.as_ref();
        let item: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(&[html_key, text_key], &[html_obj, text_obj]);
        let items = NSArray::from_slice(&[&*item]);
        #[allow(unsafe_code)]
        // SAFETY: `MainThreadMarker::new()` above guarantees execution on the UIKit main thread,
        // and `items` contains an `NSDictionary<NSString, AnyObject>` keyed by UTI strings
        // (`public.html`, `public.utf8-plain-text`) with `NSData` / `NSString` values.
        unsafe {
            board.setItems(&items);
        }
        Ok(())
    }
}
