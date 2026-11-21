# Attribution

Provenance and licence notes for bundled files. `cargo xtask assets` reports
files under `assets/` without a row here, without failing the build. Last reviewed: 2026-11-18.

## Brand artwork

| File | What | Origin | Licence status |
|---|---|---|---|
| `assets/brand/nocturne-source.png` | The Nocturne icon, a sun setting over water on a rounded square, 1024 x 1024, transparent outside the corners | Drawn by `tools/brand/make_icons.py` from the owner's design. Drawn for Nocturne by its owner (antimajorana, 2026-11-18) | Nocturne's own artwork. Kept outside the project's MIT OR Apache-2.0 licence. |
| `assets/brand/nocturne-512.png` | The same icon drawn at 512 px | Drawn by `tools/brand/make_icons.py` | Same as the source |
| `assets/brand/nocturne-256.png` | Same, 256 px | Drawn by `tools/brand/make_icons.py` | Same as the source |
| `assets/brand/nocturne-128.png` | Same, 128 px | Drawn by `tools/brand/make_icons.py` | Same as the source |
| `assets/brand/nocturne-64.png` | Same, 64 px | Drawn by `tools/brand/make_icons.py` | Same as the source |
| `assets/brand/nocturne-32.png` | Same, 32 px | Drawn by `tools/brand/make_icons.py` | Same as the source |
| `assets/brand/nocturne-square.png` | The same icon as a full square with no transparency, 1024 x 1024, for iOS, which rounds the corners itself | Drawn by `tools/brand/make_icons.py` | Same as the source |
| `assets/app-icon/nocturne.ico` | The Windows app and installer icon, 16, 24, 32, 48, 64, 128 and 256 px in one file | Drawn by `tools/brand/make_icons.py` | Same as the source |
| `assets/brand/nocturne-social-light.png` | The first-use wizard banner and GitHub social preview, 1280 x 640: the Nocturne icon, the name, subtitle and four pills on a starlight ruled page (`#F4F6FC`), light | The Nocturne icon (`assets/brand/nocturne-source.png`) put on the Pillow-generated card by `tools/brand/make_banners.py` (2026-11-18), using the Nocturne palette (`#1A2B93`, `#0E1838`, `#FCC347`, `#6EDCF6`) and system Segoe UI; only rendered glyphs are in the picture, no font file is shipped with it | Same as the Nocturne source for the icon artwork. The rest is Nocturne's own work |
| `assets/brand/nocturne-social-dark.png` | Same, dark: abyssal ruled page (`#071128`), `#EAF1FF` title, `#FCC347` and `#6EDCF6` Nocturne pills | The Nocturne icon (`assets/brand/nocturne-source.png`) put on the Pillow-generated card by `tools/brand/make_banners.py` (2026-11-18) | Same as the Nocturne source for the icon artwork. The rest is Nocturne's own work |
| `assets/demo/banner-start.png` | Large banner on the Welcome notebook's start page, copied from the 1280 x 640 light social card | Copy of `assets/brand/nocturne-social-light.png` | Same as the source |
| `assets/demo/banner-en.png` | The banner on the English welcome page of the Welcome notebook, 720 x 300: the Nocturne icon and the name on a starlight ruled page, with the tagline in English | The Nocturne icon (`assets/brand/nocturne-source.png`) put on the Pillow-generated card by `tools/brand/make_banners.py` (2026-11-18), from `assets/brand/nocturne-source.png` and system Segoe UI; only rendered glyphs are in the picture | Same as the Nocturne source for the icon artwork. The rest is Nocturne's own work |
| `assets/demo/banner-es.png` | The banner on the Spanish welcome page of the Welcome notebook, 720 x 300: the Nocturne icon and the name on a starlight ruled page, with the tagline in Spanish | The Nocturne icon (`assets/brand/nocturne-source.png`) put on the Pillow-generated card by `tools/brand/make_banners.py` (2026-11-18), from `assets/brand/nocturne-source.png` and system Segoe UI; only rendered glyphs are in the picture | Same as the Nocturne source for the icon artwork. The rest is Nocturne's own work |
| `assets/demo/banner-fr.png` | The banner on the French welcome page of the Welcome notebook, 720 x 300: the Nocturne icon and the name on a starlight ruled page, with the tagline in French | The Nocturne icon (`assets/brand/nocturne-source.png`) put on the Pillow-generated card by `tools/brand/make_banners.py` (2026-11-18), from `assets/brand/nocturne-source.png` and system Segoe UI; only rendered glyphs are in the picture | Same as the Nocturne source for the icon artwork. The rest is Nocturne's own work |
| `assets/demo/banner-de.png` | The banner on the German welcome page of the Welcome notebook, 720 x 300: the Nocturne icon and the name on a starlight ruled page, with the tagline in German | The Nocturne icon (`assets/brand/nocturne-source.png`) put on the Pillow-generated card by `tools/brand/make_banners.py` (2026-11-18), from `assets/brand/nocturne-source.png` and system Segoe UI; only rendered glyphs are in the picture | Same as the Nocturne source for the icon artwork. The rest is Nocturne's own work |
| `assets/demo/banner-ru.png` | The banner on the Russian welcome page of the Welcome notebook, 720 x 300: the Nocturne icon and the name on a starlight ruled page, with the tagline in Russian | The Nocturne icon (`assets/brand/nocturne-source.png`) put on the Pillow-generated card by `tools/brand/make_banners.py` (2026-11-18), from `assets/brand/nocturne-source.png` and system Segoe UI; only rendered glyphs are in the picture | Same as the Nocturne source for the icon artwork. The rest is Nocturne's own work |
| `assets/demo/banner-zh.png` | The banner on the Chinese, Simplified welcome page of the Welcome notebook, 720 x 300: the Nocturne icon and the name on a starlight ruled page, with the tagline in Chinese, Simplified | The Nocturne icon (`assets/brand/nocturne-source.png`) put on the Pillow-generated card by `tools/brand/make_banners.py` (2026-11-18), from `assets/brand/nocturne-source.png`, system Segoe UI and bundled Noto Sans CJK SC (SIL OFL 1.1); only rendered glyphs are in the picture | Same as the Nocturne source for the icon artwork. The rest is Nocturne's own work |
| `assets/demo/banner-ja.png` | The banner on the Japanese welcome page of the Welcome notebook, 720 x 300: the Nocturne icon and the name on a starlight ruled page, with the tagline in Japanese | The Nocturne icon (`assets/brand/nocturne-source.png`) put on the Pillow-generated card by `tools/brand/make_banners.py` (2026-11-18), from `assets/brand/nocturne-source.png`, system Segoe UI and bundled Noto Sans CJK SC (SIL OFL 1.1); only rendered glyphs are in the picture | Same as the Nocturne source for the icon artwork. The rest is Nocturne's own work |
| `assets/demo/banner-ko.png` | The banner on the Korean welcome page of the Welcome notebook, 720 x 300: the Nocturne icon and the name on a starlight ruled page, with the tagline in Korean | The Nocturne icon (`assets/brand/nocturne-source.png`) put on the Pillow-generated card by `tools/brand/make_banners.py` (2026-11-18), from `assets/brand/nocturne-source.png`, system Segoe UI and bundled Noto Sans CJK SC (SIL OFL 1.1); only rendered glyphs are in the picture | Same as the Nocturne source for the icon artwork. The rest is Nocturne's own work |
| `assets/demo/banner-he.png` | The banner on the Hebrew welcome page of the Welcome notebook, 720 x 300: the Nocturne icon and the name on a starlight ruled page, wordmark only; Hebrew needs text shaping Pillow does not have here, so that page carries the line as words | The Nocturne icon (`assets/brand/nocturne-source.png`) put on the Pillow-generated card by `tools/brand/make_banners.py` (2026-11-18), from `assets/brand/nocturne-source.png` | Same as the Nocturne source for the icon artwork. The rest is Nocturne's own work |
| `assets/demo/banner-hi.png` | The banner on the Hindi welcome page of the Welcome notebook, 720 x 300: the Nocturne icon and the name on a starlight ruled page, wordmark only; Hindi needs text shaping Pillow does not have here, so that page carries the line as words | The Nocturne icon (`assets/brand/nocturne-source.png`) put on the Pillow-generated card by `tools/brand/make_banners.py` (2026-11-18), from `assets/brand/nocturne-source.png` | Same as the Nocturne source for the icon artwork. The rest is Nocturne's own work |
| `assets/demo/banner-ar.png` | The banner on the Arabic welcome page of the Welcome notebook, 720 x 300: the Nocturne icon and the name on a starlight ruled page, wordmark only; Arabic needs text shaping Pillow does not have here, so that page carries the line as words | The Nocturne icon (`assets/brand/nocturne-source.png`) put on the Pillow-generated card by `tools/brand/make_banners.py` (2026-11-18), from `assets/brand/nocturne-source.png` | Same as the Nocturne source for the icon artwork. The rest is Nocturne's own work |
| `assets/demo/banner-fa.png` | The banner on the Persian welcome page of the Welcome notebook, 720 x 300: the Nocturne icon and the name on a starlight ruled page, wordmark only; Persian needs text shaping Pillow does not have here, so that page carries the line as words | The Nocturne icon (`assets/brand/nocturne-source.png`) put on the Pillow-generated card by `tools/brand/make_banners.py` (2026-11-18), from `assets/brand/nocturne-source.png` | Same as the Nocturne source for the icon artwork. The rest is Nocturne's own work |
| `assets/demo/banner-ur.png` | The banner on the Urdu welcome page of the Welcome notebook, 720 x 300: the Nocturne icon and the name on a starlight ruled page, wordmark only; Urdu needs text shaping Pillow does not have here, so that page carries the line as words | The Nocturne icon (`assets/brand/nocturne-source.png`) put on the Pillow-generated card by `tools/brand/make_banners.py` (2026-11-18), from `assets/brand/nocturne-source.png` | Same as the Nocturne source for the icon artwork. The rest is Nocturne's own work |

## Fonts

| File | What | Origin | Licence status |
|---|---|---|---|
| `assets/fonts/NotoNaskhArabic.ttf` | Noto Naskh Arabic, variable weight. Bundled so Arabic, and the other right-to-left scripts it covers, draw on a machine with no such font installed | The Noto Project, via the Google Fonts repository (`ofl/notonaskharabic`), downloaded 2026-10-18. Copyright 2022 The Noto Project Authors | **SIL Open Font License 1.1**, text in `assets/fonts/OFL-NotoNaskhArabic.txt`. Bundled unmodified, which is what the licence asks |
| `assets/fonts/OFL-NotoNaskhArabic.txt` | The licence text above | Shipped with the font | SIL OFL 1.1 |
| `assets/fonts/NotoSansHebrew.ttf` | Noto Sans Hebrew, variable width and weight. Bundled so Hebrew draws on a machine with no such font installed | The Noto Project, via the Google Fonts repository (`ofl/notosanshebrew`, file `NotoSansHebrew[wdth,wght].ttf`, main branch at commit `47d54ca1`), downloaded 2026-10-18. SHA-256 `7ef36a2c3593758cdb622e1bdef4f84523e92fbc3ccc667438dd80ff54c2de88`. Copyright 2022 The Noto Project Authors | **SIL Open Font License 1.1**, text in `assets/fonts/OFL-NotoSansHebrew.txt`. Bundled unmodified; renamed only |
| `assets/fonts/OFL-NotoSansHebrew.txt` | The licence text above | Shipped with the font | SIL OFL 1.1 |
| `assets/fonts/NotoSansDevanagari.ttf` | Noto Sans Devanagari, variable width and weight. Bundled so Hindi, Marathi, Nepali and Sanskrit draw on a machine with no such font installed | The Noto Project, via the Google Fonts repository (`ofl/notosansdevanagari`, file `NotoSansDevanagari[wdth,wght].ttf`, main branch at commit `47d54ca1`), downloaded 2026-10-18. SHA-256 `14ec4af41f27482216d1c2229f417ff9b1425e1babb014e57d1d40d03229853e`. Copyright 2022 The Noto Project Authors | **SIL Open Font License 1.1**, text in `assets/fonts/OFL-NotoSansDevanagari.txt`. Bundled unmodified; renamed only |
| `assets/fonts/OFL-NotoSansDevanagari.txt` | The licence text above | Shipped with the font | SIL OFL 1.1 |
| `assets/fonts/NotoSansCJKsc-Regular.otf` | Noto Sans CJK SC, Regular. 16 MB. Bundled so Chinese, Japanese and Korean draw on a machine with no such font installed (the SC build carries the Japanese and Korean glyphs too, with Simplified Chinese forms where the scripts share a character) | The Noto CJK project (`notofonts/noto-cjk`, `Sans/OTF/SimplifiedChinese`, commit `f8d157532fbfaeda587e826d4cd5b21a49186f7c`), SHA-256 `2c76254f6fc379fddfce0a7e84fb5385bb135d3e399294f6eeb6680d0365b74b`. Copyright 2014-2021 Adobe (http://www.adobe.com/), with Reserved Font Name 'Source' | **SIL Open Font License 1.1**, text in `assets/fonts/OFL-NotoSansCJKsc.txt`. Bundled unmodified |
| `assets/fonts/OFL-NotoSansCJKsc.txt` | The licence text above | Shipped with the font | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_AMS-Regular.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_Caligraphic-Bold.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_Caligraphic-Regular.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_Fraktur-Bold.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_Fraktur-Regular.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_Main-Bold.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_Main-BoldItalic.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_Main-Italic.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_Main-Regular.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_Math-BoldItalic.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_Math-Italic.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_SansSerif-Bold.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_SansSerif-Italic.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_SansSerif-Regular.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_Script-Regular.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_Size1-Regular.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_Size2-Regular.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_Size3-Regular.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_Size4-Regular.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/KaTeX_Typewriter-Regular.ttf` | KaTeX maths font, for drawing equations | The KaTeX project (Khan Academy and Design Science), unmodified, as packaged in the ratex-katex-fonts crate 0.1.15 | SIL OFL 1.1 |
| `assets/fonts/katex/OFL-KaTeX.txt` | The licence text for the KaTeX fonts above, with the copyright lines from the fonts' own name tables | Written for the project; licence body from the SIL OFL 1.1 | SIL OFL 1.1 |

## Documentation pictures

The import walkthrough illustrations below are original artwork for Nocturne
with no external source material.

| File | What | Origin | Licence status |
|---|---|---|---|
| `assets/demo/import-folder.svg` | Synthetic OneNote folder import walkthrough | Nocturne project | MIT OR Apache-2.0 |
| `assets/demo/import-file.svg` | Synthetic OneNote file import walkthrough | Nocturne project | MIT OR Apache-2.0 |
| `assets/demo/import-open.png` | Open screen showing the import actions | Rendered by `ui_shot` with the Welcome notebook demo, in the default theme | MIT OR Apache-2.0 |
| `assets/demo/imported-notebook.png` | Imported notebook view using synthetic fixture data | Rendered by `ui_shot`; no personal notebook data | MIT OR Apache-2.0 |
| `assets/demo/import-progress.png` | Import progress dialog using synthetic fixture data | Rendered by `ui_shot` with the Welcome notebook demo, in the default theme; no personal notebook data | MIT OR Apache-2.0 |

These are screenshots of the app itself, made by `cargo xtask screenshots` from sample
notebooks. They hold no personal data.

| File | What |
|---|---|

## PDF printout rendering

| File | What | Origin | Licence status |
| --- | --- | --- | --- |
| `licenses/PDF-RENDERING-NOTICES.txt` | The notices for the rows below, in the form the licences require | Written from the licence texts of the hayro crates and the PDFium font files | Shipped in the portable zip and the installer |
| hayro crates: `hayro`, `hayro-ccitt`, `hayro-font`, `hayro-interpret`, `hayro-jbig2`, `hayro-jpeg2000`, `hayro-syntax` | A pure-Rust PDF renderer, linked into the program to draw PDF printouts | Laurenz Stampfl, https://github.com/LaurenzV/hayro (locked in `Cargo.lock`) | MIT License, used under that option; the notice is in `licenses/PDF-RENDERING-NOTICES.txt` |
| Substitute fonts inside `hayro-interpret`: FoxitSans, FoxitSerif, FoxitFixed, FoxitSymbol, FoxitDingbats (`.pfb`) | Stand-ins for the 14 standard PDF fonts, compiled into the binary so printouts draw without a font installed | Extracted from PDFium; Foxit Software Inc. copyright, PDFium Authors | BSD-style licence; the text is in `licenses/PDF-RENDERING-NOTICES.txt` |
| Colour profiles inside `hayro-interpret`: `CGATS001Compat-v2-micro.icc`, `LAB.icc` | Colour profiles compiled into the binary for PDF colour | Compact-ICC-Profiles (CMYK, CC0 1.0); generated with LittleCMS | CC0 1.0 Universal, no notice required |

## Code and fonts

- The icons in the tree and toolbar are drawn in code (`crates/ui-egui/src/icons.rs`). No icon
  files are used.
- The UI uses the fonts bundled with egui. They carry their own licences, which ship with the
  egui crates. Their terms were not re-read for this file.
- Dependencies are listed in `Cargo.lock`. A licence audit of them (for example with
  `cargo deny`) has not been run.
- Nocturne's own code is dual-licensed under MIT or Apache-2.0, at your option: `LICENSE-MIT`,
  `LICENSE-APACHE`, `NOTICE`. The rows above are not covered by it where they say so.
- The `.one` file format notes in `docs/research/` paraphrase Microsoft's published
  specifications. No Microsoft text or code is included.

| File | What | Origin | Licence status |
|---|---|---|---|
| `crates/ui-egui/assets/fluent/*.svg` | Selected Fluent UI System Icons used by the ribbon | Microsoft Fluent UI System Icons, MIT licensed; source: https://github.com/microsoft/fluentui-system-icons | Microsoft Corporation and contributors |
| `crates/ui-egui/assets/fluent-color/*.svg` | Selected Fluent UI System Icons colour versions (calendar, clock, link, image, table, video, mic, document) used by the ribbon | Microsoft Fluent UI System Icons, MIT licensed; source: https://github.com/microsoft/fluentui-system-icons (`assets/<Name>/SVG/ic_fluent_<name>_24_color.svg`) | Microsoft Corporation and contributors |
