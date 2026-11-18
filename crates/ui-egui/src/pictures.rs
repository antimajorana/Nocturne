//! The picture formats the page draws, beyond what egui_extras reads by itself.
//!
//! egui_extras draws pictures through the `image` crate. JPEG XL (issue #29) is not one of the
//! formats `image` builds in, so its decoder, `jxl-oxide`, is registered with `image` as a
//! decoding hook for `.jxl` and for JPEG XL's two signatures. Once it is, a `.jxl` in a page's
//! `assets` folder is drawn like a PNG: the file loader's extension check, the page's own byte
//! loader and `image`'s format guessing all find it.

/// Registers the picture decoders `image` does not have, once for the whole process: the hooks
/// are global.
pub fn register_formats() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = jxl_oxide::integration::register_image_decoding_hook();
    });
}

/// egui_extras' picture loaders, with every format above registered first.
pub fn install(ctx: &egui::Context) {
    register_formats();
    egui_extras::install_image_loaders(ctx);
}

/// Converts a rasterized printout PNG for dark-mode viewing by inverting lightness while
/// preserving hue and saturation, turning white paper dark so dark-mode-inverted ink stays
/// legible without hue-inverting diagrams or photos.
pub(crate) fn invert_printout_png_for_dark_mode(bytes: &[u8]) -> Option<Vec<u8>> {
    register_formats();
    let mut rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
    for pixel in rgba.pixels_mut() {
        if pixel.0[3] == 0 {
            continue;
        }
        let [r, g, b] = invert_rgb_lightness(pixel.0[0], pixel.0[1], pixel.0[2]);
        pixel.0[0] = r;
        pixel.0[1] = g;
        pixel.0[2] = b;
    }
    let (w, h) = rgba.dimensions();
    let mut out = Vec::with_capacity(bytes.len().min(w as usize * h as usize * 4));
    let encoder = image::codecs::png::PngEncoder::new_with_quality(
        &mut out,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::NoFilter,
    );
    image::ImageEncoder::write_image(encoder, rgba.as_raw(), w, h, image::ExtendedColorType::Rgba8).ok()?;
    Some(out)
}

fn invert_rgb_lightness(r: u8, g: u8, b: u8) -> [u8; 3] {
    let rf = f32::from(r) / 255.0;
    let gf = f32::from(g) / 255.0;
    let bf = f32::from(b) / 255.0;
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let l = 0.5 * (max + min);
    // Map [0.0..=1.0] lightness linearly onto [0.92..=0.11] so white paper becomes a dark
    // surface (~#1c1c1c) and black text lifts (~#ebebeb) for contrast against light ink.
    let l_dark = (0.92 - 0.81 * l).clamp(0.0, 1.0);
    let d = max - min;
    if d <= 1e-5 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let v = (l_dark * 255.0).round().clamp(0.0, 255.0) as u8;
        return [v, v, v];
    }
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if (max - rf).abs() <= 1e-5 {
        ((gf - bf) / d + if gf < bf { 6.0 } else { 0.0 }) / 6.0
    } else if (max - gf).abs() <= 1e-5 {
        ((bf - rf) / d + 2.0) / 6.0
    } else {
        ((rf - gf) / d + 4.0) / 6.0
    };
    hsl_to_rgb(h, s, l_dark)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [u8; 3] {
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hue_channel = |mut t: f32| -> u8 {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            (v * 255.0).round().clamp(0.0, 255.0) as u8
        }
    };
    [hue_channel(h + 1.0 / 3.0), hue_channel(h), hue_channel(h - 1.0 / 3.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 24 by 16, left half red, right half blue, lossless.
    const TWO_COLOURS: &[u8] = include_bytes!("../tests/fixtures/two-colours.jxl");

    #[test]
    fn a_jpeg_xl_picture_decodes_by_its_signature() {
        register_formats();
        let picture = image::ImageReader::new(std::io::Cursor::new(TWO_COLOURS))
            .with_guessed_format()
            .expect("read")
            .decode()
            .expect("JPEG XL decodes once the hook is registered")
            .to_rgba8();
        assert_eq!(picture.dimensions(), (24, 16));
        assert_eq!(picture.get_pixel(2, 8).0, [255, 0, 0, 255], "the left half is red");
        assert_eq!(picture.get_pixel(21, 8).0, [0, 0, 255, 255], "the right half is blue");
    }

    #[test]
    fn junk_with_a_jpeg_xl_name_is_an_error_not_a_panic() {
        register_formats();
        let junk = [0xFF_u8, 0x0A, 1, 2, 3];
        let read = image::ImageReader::new(std::io::Cursor::new(&junk[..])).with_guessed_format().expect("read");
        assert!(read.decode().is_err());
    }

    /// The same path a picture in a page's `assets` folder takes when it is drawn.
    #[test]
    fn a_jpeg_xl_attachment_loads_through_the_file_loader() {
        use nocturne_store::testing::TempDir;
        let t = TempDir::new("pictures-jxl");
        let page = t.path().join("page");
        std::fs::create_dir_all(page.join("assets")).expect("assets");
        std::fs::write(page.join("assets").join("two-colours.jxl"), TWO_COLOURS).expect("jxl");
        let uri = crate::preview::image_uri(&page, "assets/two-colours.jxl").expect("uri");
        let ctx = egui::Context::default();
        install(&ctx);
        let mut last = String::new();
        for _ in 0..100 {
            match ctx.try_load_image(&uri, egui::SizeHint::default()) {
                Ok(egui::load::ImagePoll::Ready { image }) => {
                    assert_eq!(image.size, [24, 16]);
                    return;
                }
                Ok(egui::load::ImagePoll::Pending { .. }) => last = "pending".into(),
                Err(e) => panic!("{uri}: {e}"),
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("never ready: {last}");
    }

    #[test]
    fn dark_mode_printout_inversion_darkens_paper_lifts_text_and_preserves_hue() {
        let mut img = image::RgbaImage::new(3, 1);
        img.put_pixel(0, 0, image::Rgba([255, 255, 255, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 0, 0, 255]));
        img.put_pixel(2, 0, image::Rgba([220, 40, 40, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut png, image::ImageFormat::Png).expect("encode test png");

        let inverted = invert_printout_png_for_dark_mode(&png.into_inner()).expect("invert png");
        let decoded = image::load_from_memory(&inverted).expect("decode inverted").into_rgba8();
        let paper = decoded.get_pixel(0, 0).0;
        let text = decoded.get_pixel(1, 0).0;
        let red = decoded.get_pixel(2, 0).0;

        assert!(paper[0] <= 32 && paper[1] <= 32 && paper[2] <= 32, "white paper must become dark: {paper:?}");
        assert!(text[0] >= 220 && text[1] >= 220 && text[2] >= 220, "black text must become light: {text:?}");
        assert!(red[0] > red[1] && red[0] > red[2], "red hue must be preserved: {red:?}");
    }
}
