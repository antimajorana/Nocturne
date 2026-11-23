//! Themes: light and dark, each with seven accents, 14 in all. The UI reads colours
//! from `Tokens` and never uses literal colours.
//!
//! Every text and accent colour is checked against its background (WCAG contrast) when
//! the tokens are built, so no theme can ship unreadable text. The tests check all 14.

use egui::Color32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Light,
    Dark,
}

impl Mode {
    pub fn id(self) -> &'static str {
        match self {
            Mode::Light => "light",
            Mode::Dark => "dark",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Light => "Light",
            Mode::Dark => "Dark",
        }
    }

    pub fn is_dark(self) -> bool {
        self == Mode::Dark
    }
}

/// How the mode is chosen: fixed, or following the system's light or dark setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModePref {
    Light,
    Dark,
    System,
}

impl ModePref {
    pub fn id(self) -> &'static str {
        match self {
            ModePref::Light => "light",
            ModePref::Dark => "dark",
            ModePref::System => "system",
        }
    }

    pub fn parse(text: &str) -> Option<ModePref> {
        match text.trim() {
            "light" => Some(ModePref::Light),
            "dark" => Some(ModePref::Dark),
            "system" => Some(ModePref::System),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Accent {
    /// The Nocturne ultramarine on starlight and midnight-navy surfaces.
    Default,
    Red,
    Green,
    Yellow,
    Blue,
    Gray,
    /// Purple surfaces as well as a purple accent.
    Purple,
}

impl Accent {
    pub const ALL: [Accent; 7] = [Accent::Default, Accent::Red, Accent::Green, Accent::Yellow, Accent::Blue, Accent::Gray, Accent::Purple];

    pub fn id(self) -> &'static str {
        match self {
            Accent::Default => "default",
            Accent::Red => "red",
            Accent::Green => "green",
            Accent::Yellow => "yellow",
            Accent::Blue => "blue",
            Accent::Gray => "gray",
            Accent::Purple => "purple",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Accent::Default => "Default",
            Accent::Red => "Red",
            Accent::Green => "Green",
            Accent::Yellow => "Yellow",
            Accent::Blue => "Blue",
            Accent::Gray => "Gray",
            Accent::Purple => "Purple",
        }
    }

    pub fn parse(text: &str) -> Option<Accent> {
        Accent::ALL.into_iter().find(|a| a.id() == text.trim())
    }

    /// The accent colour before the contrast check.
    fn base(self, mode: Mode) -> Color32 {
        let (l, d) = match self {
            Accent::Default => ((0x1a, 0x2b, 0x93), (0x7b, 0x96, 0xff)),
            Accent::Red => ((0xc0, 0x39, 0x2b), (0xff, 0x8a, 0x80)),
            Accent::Green => ((0x1e, 0x8a, 0x44), (0x6c, 0xd0, 0x8a)),
            Accent::Yellow => ((0xa8, 0x74, 0x00), (0xf2, 0xc9, 0x4c)),
            Accent::Blue => ((0x1a, 0x5f, 0xd0), (0x8f, 0xb8, 0xff)),
            Accent::Gray => ((0x52, 0x58, 0x62), (0xb4, 0xba, 0xc4)),
            Accent::Purple => ((0x7a, 0x45, 0xb8), (0xb0, 0x85, 0xe8)),
        };
        let (r, g, b) = if mode.is_dark() { d } else { l };
        Color32::from_rgb(r, g, b)
    }

    /// How much of the accent is mixed into the surfaces. Default and Gray stay untinted.
    fn tint(self, mode: Mode) -> f32 {
        match self {
            Accent::Default | Accent::Gray => 0.0,
            _ => {
                if mode.is_dark() {
                    0.09
                } else {
                    0.05
                }
            }
        }
    }

    /// Base untinted surfaces `(panel0, editor0, faint0, text, muted0)` for this accent family.
    ///
    /// - `Accent::Default` preserves Nocturne's signature starlight / midnight-navy palette.
    /// - `Accent::Gray` uses true neutral charcoal / alabaster with no navy bias.
    /// - Chromatic accents start from clean neutral charcoal / paper so tinting never muddies
    ///   warm hues with navy blue.
    fn base_surfaces(self, dark: bool) -> (Color32, Color32, Color32, Color32, Color32) {
        match (self, dark) {
            (Accent::Default, true) => (rgb((12, 24, 54)), rgb((7, 17, 40)), rgb((20, 36, 76)), rgb((234, 241, 255)), rgb((150, 170, 212))),
            (Accent::Default, false) => (rgb((244, 246, 252)), rgb((255, 255, 255)), rgb((233, 238, 249)), rgb((14, 24, 56)), rgb((78, 92, 132))),
            (Accent::Gray, true) => (rgb((24, 25, 28)), rgb((16, 17, 19)), rgb((34, 36, 40)), rgb((238, 240, 243)), rgb((158, 162, 170))),
            (Accent::Gray, false) => (rgb((245, 246, 247)), rgb((255, 255, 255)), rgb((235, 237, 240)), rgb((26, 28, 32)), rgb((92, 96, 104))),
            (_, true) => (rgb((22, 23, 26)), rgb((15, 16, 18)), rgb((32, 34, 38)), rgb((240, 242, 245)), rgb((156, 160, 168))),
            (_, false) => (rgb((246, 246, 247)), rgb((255, 255, 255)), rgb((236, 237, 239)), rgb((24, 25, 28)), rgb((90, 93, 100))),
        }
    }
}

/// Unified corner radius for standard controls (buttons, inputs, chips, tabs).
pub const RADIUS_CONTROL: u8 = 4;
/// Unified corner radius for floating menus, dropdowns, and popovers.
pub const RADIUS_POPUP: u8 = 8;
/// Unified corner radius for cards, preview panels, and floating widgets.
pub const RADIUS_CARD: u8 = 10;
/// Unified corner radius for modal dialog surfaces.
pub const RADIUS_MODAL: u8 = 12;
/// Unified corner radius for full stadium pills and capsule bars.
pub const RADIUS_PILL: u8 = 99;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Theme {
    pub mode: Mode,
    pub accent: Accent,
}

impl Theme {
    pub const DEFAULT: Theme = Theme { mode: Mode::Light, accent: Accent::Default };
    pub const RADIUS_CONTROL: u8 = RADIUS_CONTROL;
    pub const RADIUS_CARD: u8 = RADIUS_CARD;
    pub const RADIUS_POPUP: u8 = RADIUS_POPUP;
    pub const RADIUS_MODAL: u8 = RADIUS_MODAL;
    pub const RADIUS_PILL: u8 = RADIUS_PILL;

    /// All 14 themes: light then dark, each with the seven accents.
    pub fn all() -> Vec<Theme> {
        [Mode::Light, Mode::Dark].into_iter().flat_map(|mode| Accent::ALL.into_iter().map(move |accent| Theme { mode, accent })).collect()
    }

    pub fn id(self) -> String {
        format!("{}-{}", self.mode.id(), self.accent.id())
    }

    pub fn label(self) -> String {
        format!("{} {}", self.mode.label(), self.accent.label())
    }

    pub fn parse(id: &str) -> Option<Theme> {
        Theme::all().into_iter().find(|t| t.id() == id.trim())
    }

    pub fn tokens(self) -> Tokens {
        Tokens::for_theme(self)
    }

    /// Drop shadow for floating modals and elevated dialogs.
    pub fn shadow_floating(self) -> egui::Shadow {
        self.tokens().shadow_floating()
    }

    /// Drop shadow for transient menus, dropdowns, and popovers.
    pub fn shadow_popup(self) -> egui::Shadow {
        self.tokens().shadow_popup()
    }

    /// The egui visuals for this theme, fully populated across all five widget states,
    /// selection wash, text cursor, window surfaces, strokes, corner radii, and shadows.
    pub fn visuals(self) -> egui::Visuals {
        let t = self.tokens();
        let mut v = if self.mode.is_dark() { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.dark_mode = self.mode.is_dark();
        v.weak_text_color = Some(t.muted);
        v.panel_fill = t.panel;
        v.window_fill = t.surface_elevated;
        v.window_stroke = egui::Stroke::new(1.0, t.border);
        v.window_corner_radius = egui::CornerRadius::same(RADIUS_MODAL);
        v.window_shadow = t.shadow_floating();
        v.menu_corner_radius = egui::CornerRadius::same(RADIUS_POPUP);
        v.popup_shadow = t.shadow_popup();
        v.extreme_bg_color = t.editor;
        v.text_edit_bg_color = Some(t.editor);
        v.faint_bg_color = t.faint;
        v.code_bg_color = t.code_bg;
        v.hyperlink_color = t.link;
        v.selection.bg_fill = t.row_selected;
        v.selection.stroke = egui::Stroke::new(1.0, t.accent);
        v.text_cursor.stroke = egui::Stroke::new(2.0, t.accent);

        let r = egui::CornerRadius::same(RADIUS_CONTROL);

        v.widgets.noninteractive.bg_fill = t.panel;
        v.widgets.noninteractive.weak_bg_fill = t.panel;
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, t.border_subtle);
        v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, t.text);
        v.widgets.noninteractive.corner_radius = r;
        v.widgets.noninteractive.expansion = 0.0;

        v.widgets.inactive.bg_fill = t.faint;
        v.widgets.inactive.weak_bg_fill = t.faint;
        v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, t.border);
        v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, t.text);
        v.widgets.inactive.corner_radius = r;
        v.widgets.inactive.expansion = 0.0;

        v.widgets.hovered.bg_fill = t.row_hover;
        v.widgets.hovered.weak_bg_fill = t.row_hover;
        v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, t.accent);
        v.widgets.hovered.fg_stroke = egui::Stroke::new(1.5, t.strong);
        v.widgets.hovered.corner_radius = r;
        v.widgets.hovered.expansion = 0.0;

        v.widgets.active.bg_fill = t.row_selected;
        v.widgets.active.weak_bg_fill = t.row_selected;
        v.widgets.active.bg_stroke = egui::Stroke::new(1.5, t.accent);
        v.widgets.active.fg_stroke = egui::Stroke::new(1.5, t.strong);
        v.widgets.active.corner_radius = r;
        v.widgets.active.expansion = 0.0;

        v.widgets.open.bg_fill = t.row_selected;
        v.widgets.open.weak_bg_fill = t.row_selected;
        v.widgets.open.bg_stroke = egui::Stroke::new(1.0, t.accent);
        v.widgets.open.fg_stroke = egui::Stroke::new(1.5, t.strong);
        v.widgets.open.corner_radius = r;
        v.widgets.open.expansion = 0.0;

        v.warn_fg_color = t.warn;
        v.error_fg_color = t.warn;
        v
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tokens {
    /// The theme these tokens were derived from.
    pub theme: Theme,
    pub panel: Color32,
    /// Elevated surface for modals, popovers, menus, and floating cards.
    pub surface_elevated: Color32,
    pub editor: Color32,
    pub faint: Color32,
    /// Visible structural border for inputs, buttons, modals, and popups (>= 3:1 contrast).
    pub border: Color32,
    /// Quiet hairline separator for dividers, group rules, and subtle outlines.
    pub border_subtle: Color32,
    /// The outline of an active text container, kept separate from other faint surfaces.
    pub container_border: Color32,
    pub text: Color32,
    /// Disabled control text and icons: softer than `muted` yet still legible (>= 3:1).
    pub disabled: Color32,
    pub accent: Color32,
    /// Accent colour tuned for readable text on `panel` and `surface_elevated` (>= 4.5:1).
    pub accent_text: Color32,
    pub notebook: Color32,
    pub group: Color32,
    pub section: Color32,
    pub page: Color32,
    pub muted: Color32,
    pub strong: Color32,
    pub link: Color32,
    pub code_bg: Color32,
    pub quote_bar: Color32,
    pub row_hover: Color32,
    pub row_selected: Color32,
    /// The wash behind selected words on the page. Not `row_selected`: that one is tinted
    /// from the panel, because a selected row of the tree sits on the panel, and the page
    /// is drawn on `editor`. Where a theme puts those two colours far apart the panel's
    /// tint lands on top of the page's own colour and the selection is invisible, which is
    /// what the owner saw on 2026-10-19 in a green theme: the words were selected, the
    /// band was painted, and nothing could be seen.
    pub selection: Color32,
    pub ok: Color32,
    pub warn: Color32,
    /// Primary call-to-action button fill, hover fill, and contrast-guaranteed text.
    pub btn_primary_bg: Color32,
    pub btn_primary_hover: Color32,
    pub btn_primary_text: Color32,
    /// Destructive action button fill, hover fill, and contrast-guaranteed text.
    pub btn_danger_bg: Color32,
    pub btn_danger_hover: Color32,
    pub btn_danger_text: Color32,
    /// Translucent backdrop scrim behind modal dialogs.
    pub scrim: Color32,
    /// Destructive red hover fill for window and modal close buttons (white icon on top).
    pub close_hover: Color32,
    /// Rule lines and grid lines drawn on the page itself (OneNote's View, Rule Lines).
    /// Decoration, so the target is low on purpose: visible, never competing with the text.
    pub rule_line: Color32,
    /// Every fifth grid line, or the margin line of a ruled page. A little stronger.
    pub rule_line_strong: Color32,
    /// The lines round the cells of a table. Stronger than a ruled page, because a table
    /// is read as a grid and its lines carry the shape; still quiet beside the words.
    pub table_line: Color32,
    /// The coloured band across the top of the window, where the title, the Quick Access
    /// Toolbar and the ribbon's tab names sit, as in Office. Dark enough for white writing.
    pub band: Color32,
    /// Writing and icons on the band.
    pub band_text: Color32,
    /// Greyed writing and icons on the band.
    pub band_dim: Color32,
    /// A button or tab on the band that the pointer is over.
    pub band_hover: Color32,
}

fn rgb(c: (u8, u8, u8)) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

fn channel(c: u8) -> f32 {
    let s = f32::from(c) / 255.0;
    if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
}

pub fn luminance(c: Color32) -> f32 {
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

/// WCAG contrast ratio, 1.0 to 21.0.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Moves `fg` away from `bg` (towards black on a light surface, white on a dark one)
/// until the contrast is at least `min`.
fn ensure_contrast(fg: Color32, bg: Color32, min: f32, dark: bool) -> Color32 {
    let target = if dark { Color32::WHITE } else { Color32::BLACK };
    let mut c = fg;
    for _ in 0..60 {
        if contrast(c, bg) >= min {
            return c;
        }
        c = mix(c, target, 0.05);
    }
    target
}

/// A colour the user chose (a notebook cover, a section tab), moved just far enough to be
/// readable on `bg`. The hue is kept; only the lightness changes.
pub fn readable_on(fg: Color32, bg: Color32, min: f32, dark: bool) -> Color32 {
    ensure_contrast(fg, bg, min, dark)
}

/// A line on `bg` that is just visible: mixed towards `towards` until the contrast reaches
/// `target`, then stopped. Used for rule lines and grid lines on a page, in both modes.
/// The rule and grid lines of a page given a line colour of its own (issue #288): the same
/// faint and stronger lines as the theme's, `(rule, strong)`, leaning towards the colour
/// instead of the accent, so they stay quiet behind the words in every theme. `None` is the
/// theme's own lines.
pub fn rule_lines_in(t: &Tokens, colour: Option<nocturne_model::NodeColor>) -> (Color32, Color32) {
    let Some(colour) = colour else { return (t.rule_line, t.rule_line_strong) };
    let (r, g, b) = colour.rgb(t.editor.r() < 128);
    let towards = Color32::from_rgb(r, g, b);
    (faint_line(t.editor, towards, 1.25), faint_line(t.editor, towards, 1.8))
}

fn faint_line(bg: Color32, towards: Color32, target: f32) -> Color32 {
    let mut c = bg;
    for _ in 0..60 {
        if contrast(c, bg) >= target {
            return c;
        }
        c = mix(c, towards, 0.04);
    }
    towards
}

/// `mix(base, tint, t)` with `t` lowered until `text` still has contrast `min` on the result.
/// A bright accent (yellow) would otherwise wash out the text on a selected row.
fn tint_keeping_contrast(base: Color32, tint: Color32, t_max: f32, text: Color32, min: f32) -> Color32 {
    let mut t = t_max;
    while t > 0.0 {
        let c = mix(base, tint, t);
        if contrast(text, c) >= min {
            return c;
        }
        t -= 0.02;
    }
    base
}

/// The wash behind selected words on the page: the page's own colour carried towards the
/// accent, far enough to be seen against the page and not so far that the words on it stop
/// being readable.
///
/// The accent is tried first, because a selection in the theme's own colour is what a
/// reader expects. A pale accent on a pale page cannot be told apart from it, and then the
/// text colour is used instead, which every theme keeps well away from the page by
/// definition.
fn selection_wash(editor: Color32, acc: Color32, text: Color32) -> Color32 {
    for tint in [acc, text] {
        let mut t = 0.30;
        while t > 0.04 {
            let c = mix(editor, tint, t);
            if contrast(text, c) >= 7.0 && contrast(c, editor) >= 1.25 {
                return c;
            }
            t -= 0.02;
        }
    }
    editor
}

impl Tokens {
    pub const RADIUS_CONTROL: u8 = RADIUS_CONTROL;
    pub const RADIUS_CARD: u8 = RADIUS_CARD;
    pub const RADIUS_POPUP: u8 = RADIUS_POPUP;
    pub const RADIUS_MODAL: u8 = RADIUS_MODAL;
    pub const RADIUS_PILL: u8 = RADIUS_PILL;

    pub fn for_theme(theme: Theme) -> Tokens {
        let dark = theme.mode.is_dark();
        let acc = theme.accent.base(theme.mode);
        let tint = theme.accent.tint(theme.mode);
        let (panel0, editor0, faint0, text, muted0) = theme.accent.base_surfaces(dark);
        let panel = mix(panel0, acc, tint);
        let surface_elevated = if dark { mix(panel, Color32::WHITE, 0.06) } else { mix(panel, Color32::WHITE, 0.72) };
        let editor = mix(editor0, acc, tint * 0.5);
        let faint = mix(faint0, acc, tint);
        let border_base = if dark { surface_elevated } else { panel };
        let border = faint_line(border_base, text, 3.0);
        let border_subtle = faint_line(border_base, text, 1.55);
        let disabled = faint_line(border_base, text, 3.0);
        let container_border = faint_line(editor, text, 2.5);
        let muted = ensure_contrast(ensure_contrast(muted0, panel, 4.5, dark), surface_elevated, 4.5, dark);
        let accent = ensure_contrast(ensure_contrast(acc, panel, 3.0, dark), surface_elevated, 3.0, dark);
        let accent_text = ensure_contrast(ensure_contrast(acc, panel, 4.5, dark), surface_elevated, 4.5, dark);
        // The band: the accent, taken darker until white writing reads on it (7, so it still reads under the hover tint), and in the
        // dark modes kept low, so the band is a deep colour and not a bright one.
        let band_base = if dark { mix(panel0, acc, 0.45) } else { acc };
        let band = ensure_contrast(band_base, Color32::WHITE, 7.0, false);
        let link = ensure_contrast(acc, editor, 4.5, dark);
        let group = ensure_contrast(if dark { rgb((0xfc, 0xc3, 0x47)) } else { rgb((0xc2, 0x7b, 0x08)) }, panel, 3.0, dark);
        let section = ensure_contrast(if dark { rgb((0x6e, 0xdc, 0xf6)) } else { rgb((0x0e, 0x7c, 0x9e)) }, panel, 3.0, dark);

        // Primary & danger buttons: dark modes use luminous accent fills with crisp dark ink
        // (brightening on hover); light modes use deep fills with white text (darkening on hover).
        // Both polarities guarantee WCAG AA (>= 4.5:1, targeted >= 5.0:1) on rest and hover.
        let dark_ink = rgb((10, 12, 16));
        let (btn_primary_bg, btn_primary_hover, btn_primary_text) = if dark {
            let bg = ensure_contrast(acc, dark_ink, 5.0, true);
            let hover = mix(bg, Color32::WHITE, 0.14);
            (bg, hover, dark_ink)
        } else {
            let bg = ensure_contrast(acc, Color32::WHITE, 5.0, false);
            let hover = mix(bg, Color32::BLACK, 0.14);
            (bg, hover, Color32::WHITE)
        };
        let (btn_danger_bg, btn_danger_hover, btn_danger_text) = if dark {
            let bg = ensure_contrast(rgb((0xf2, 0x74, 0x6b)), dark_ink, 5.0, true);
            let hover = mix(bg, Color32::WHITE, 0.14);
            (bg, hover, dark_ink)
        } else {
            let bg = ensure_contrast(rgb((0xc6, 0x28, 0x28)), Color32::WHITE, 5.0, false);
            let hover = mix(bg, Color32::BLACK, 0.14);
            (bg, hover, Color32::WHITE)
        };
        let scrim = if dark { Color32::from_black_alpha(165) } else { Color32::from_black_alpha(115) };
        let close_hover = ensure_contrast(rgb((0xc4, 0x2b, 0x1c)), Color32::WHITE, 4.5, false);

        Tokens {
            theme,
            panel,
            surface_elevated,
            editor,
            faint,
            border,
            border_subtle,
            container_border,
            text,
            disabled,
            accent,
            accent_text,
            notebook: accent,
            group,
            section,
            page: muted,
            muted,
            strong: if dark { Color32::WHITE } else { Color32::BLACK },
            link,
            code_bg: mix(panel, acc, if dark { 0.16 } else { 0.10 }),
            quote_bar: accent,
            row_hover: tint_keeping_contrast(panel, acc, 0.10, text, 7.0),
            row_selected: tint_keeping_contrast(panel, acc, 0.24, text, 7.0),
            selection: selection_wash(editor, acc, text),
            ok: ensure_contrast(if dark { rgb((0x6c, 0xd0, 0x8a)) } else { rgb((0x1e, 0x8a, 0x44)) }, panel, 3.0, dark),
            warn: ensure_contrast(if dark { rgb((0xff, 0x8a, 0x80)) } else { rgb((0xb3, 0x26, 0x1e)) }, panel, 4.5, dark),
            btn_primary_bg,
            btn_primary_hover,
            btn_primary_text,
            btn_danger_bg,
            btn_danger_hover,
            btn_danger_text,
            scrim,
            close_hover,
            rule_line: faint_line(editor, acc, 1.25),
            rule_line_strong: faint_line(editor, acc, 1.8),
            table_line: faint_line(editor, text, 2.2),
            band,
            band_text: Color32::WHITE,
            band_dim: mix(band, Color32::WHITE, 0.6),
            band_hover: mix(band, Color32::WHITE, 0.18),
        }
    }

    /// True when these tokens belong to a dark theme.
    pub fn is_dark(&self) -> bool {
        self.theme.mode.is_dark()
    }

    /// Returns the base (non-band) tokens for transient popups and menus launched from a
    /// control that may itself sit on the dark title band (such as the Quick Access Toolbar).
    pub fn for_popup(&self) -> Tokens {
        self.theme.tokens()
    }

    /// Drop shadow for floating modals and elevated dialogs.
    pub fn shadow_floating(&self) -> egui::Shadow {
        if self.is_dark() {
            egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(140) }
        } else {
            egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(48) }
        }
    }

    /// Drop shadow for transient menus, dropdowns, and popovers.
    pub fn shadow_popup(&self) -> egui::Shadow {
        if self.is_dark() {
            egui::Shadow { offset: [0, 6], blur: 16, spread: 0, color: Color32::from_black_alpha(115) }
        } else {
            egui::Shadow { offset: [0, 4], blur: 12, spread: 0, color: Color32::from_black_alpha(36) }
        }
    }

    /// The same colours seen from the band: its own surface and its own writing, so a control
    /// drawn for the page's surfaces (the Quick Access Toolbar, the theme button) reads there.
    pub fn on_band(&self) -> Tokens {
        Tokens {
            panel: self.band,
            text: self.band_text,
            accent: self.band_text,
            accent_text: self.band_text,
            muted: self.band_dim,
            disabled: self.band_dim,
            strong: self.band_text,
            row_hover: self.band_hover,
            row_selected: self.band_hover,
            selection: self.band_hover,
            ..*self
        }
    }
}

/// The shared popup treatment for language pickers and menus: themed elevated surface,
/// visible border, shadow, selection, and padding.
pub fn picker_popup_style(tokens: Tokens) -> egui::style::StyleModifier {
    let tokens = tokens.for_popup();
    egui::style::StyleModifier::new(move |style| {
        style.visuals.window_fill = tokens.surface_elevated;
        // The popup can have a strongly tinted surface (for example, the Purple
        // accent) or be launched from the dark title band. Always bind popup text
        // to the popup's own surface tokens.
        style.visuals.override_text_color = Some(tokens.text);
        style.visuals.window_stroke = egui::Stroke::new(1.0, tokens.border);
        style.visuals.popup_shadow = tokens.shadow_popup();
        style.visuals.menu_corner_radius = egui::CornerRadius::same(RADIUS_POPUP);
        style.visuals.selection.bg_fill = tokens.row_selected;
        style.visuals.selection.stroke = egui::Stroke::new(1.0, tokens.accent);
        style.visuals.widgets.hovered.bg_fill = tokens.row_hover;
        style.visuals.widgets.hovered.weak_bg_fill = tokens.row_hover;
        style.visuals.widgets.active.bg_fill = tokens.row_selected;
        style.visuals.widgets.active.weak_bg_fill = tokens.row_selected;
        style.visuals.widgets.open.bg_fill = tokens.row_selected;
        style.visuals.widgets.open.weak_bg_fill = tokens.row_selected;
        style.spacing.menu_margin = egui::Margin::same(8);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_picker_popup_uses_theme_surface_border_and_selection() {
        for theme in Theme::all() {
            let tokens = theme.tokens();
            let mut style = egui::Style::default();
            picker_popup_style(tokens).apply(&mut style);
            assert_eq!(style.visuals.window_fill, tokens.surface_elevated, "{}: popup surface", theme.label());
            assert_eq!(style.visuals.override_text_color, Some(tokens.text), "{}: popup text", theme.label());
            assert!(contrast(tokens.text, tokens.surface_elevated) >= 7.0, "{}: popup text on surface", theme.label());
            assert!(contrast(tokens.text, tokens.row_hover) >= 7.0, "{}: popup text on hover", theme.label());
            assert!(contrast(tokens.text, tokens.row_selected) >= 7.0, "{}: popup text on selection", theme.label());
            assert_eq!(style.visuals.window_stroke.color, tokens.border, "{}: popup border", theme.label());
            assert_eq!(style.visuals.popup_shadow, tokens.shadow_popup(), "{}: popup shadow", theme.label());
            assert_eq!(style.visuals.selection.bg_fill, tokens.row_selected, "{}: selected locale", theme.label());
            assert_eq!(style.visuals.menu_corner_radius, egui::CornerRadius::same(RADIUS_POPUP), "{}: rounded popup", theme.label());
            assert_eq!(style.spacing.menu_margin, egui::Margin::same(8), "{}: popup padding", theme.label());

            // Launching a popup from band-adapted tokens must still render on the elevated surface,
            // never white text on a light popup.
            let mut band_style = egui::Style::default();
            picker_popup_style(tokens.on_band()).apply(&mut band_style);
            assert_eq!(band_style.visuals.window_fill, tokens.surface_elevated, "{}: band popup surface", theme.label());
            assert_eq!(band_style.visuals.override_text_color, Some(tokens.text), "{}: band popup text", theme.label());
        }
    }

    #[test]
    fn there_are_fourteen_distinct_themes() {
        let all = Theme::all();
        assert_eq!(all.len(), 14);
        let ids: std::collections::HashSet<String> = all.iter().map(|t| t.id()).collect();
        assert_eq!(ids.len(), 14);
        let toks: Vec<Tokens> = all.iter().map(|t| t.tokens()).collect();
        for (i, a) in toks.iter().enumerate() {
            for b in toks.iter().skip(i + 1) {
                assert_ne!(a, b, "two themes look the same");
            }
        }
    }

    #[test]
    fn gray_is_true_neutral_and_default_preserves_midnight_navy() {
        let dark_default = Theme { mode: Mode::Dark, accent: Accent::Default }.tokens();
        assert_eq!(dark_default.panel, Color32::from_rgb(12, 24, 54), "Dark Default keeps midnight-navy panel");
        assert_eq!(dark_default.editor, Color32::from_rgb(7, 17, 40), "Dark Default keeps midnight-navy editor");

        for mode in [Mode::Light, Mode::Dark] {
            let gray = Theme { mode, accent: Accent::Gray }.tokens();
            let spread = |c: Color32| c.r().max(c.g()).max(c.b()) - c.r().min(c.g()).min(c.b());
            assert!(spread(gray.panel) <= 6, "{mode:?} Gray panel should be neutral charcoal/slate, got {:?}", gray.panel);
            assert!(spread(gray.editor) <= 6, "{mode:?} Gray editor should be neutral, got {:?}", gray.editor);
            assert!(spread(gray.surface_elevated) <= 6, "{mode:?} Gray elevated surface should be neutral, got {:?}", gray.surface_elevated);
        }

        // Chromatic dark accents stay free of navy blue bias: Dark Red panel must be warmer than blue,
        // Dark Green panel must lean green, and Dark Yellow panel must lean warm.
        let dark_red = Theme { mode: Mode::Dark, accent: Accent::Red }.tokens();
        assert!(dark_red.panel.r() > dark_red.panel.b(), "Dark Red should not be navy-muddied: {:?}", dark_red.panel);
        let dark_green = Theme { mode: Mode::Dark, accent: Accent::Green }.tokens();
        assert!(dark_green.panel.g() > dark_green.panel.b(), "Dark Green should not be navy-muddied: {:?}", dark_green.panel);
        let dark_yellow = Theme { mode: Mode::Dark, accent: Accent::Yellow }.tokens();
        assert!(dark_yellow.panel.r() > dark_yellow.panel.b(), "Dark Yellow should not be navy-muddied: {:?}", dark_yellow.panel);
    }

    #[test]
    fn ids_round_trip() {
        for t in Theme::all() {
            assert_eq!(Theme::parse(&t.id()), Some(t));
        }
        assert_eq!(Theme::parse("light-purple"), Some(Theme { mode: Mode::Light, accent: Accent::Purple }));
        assert_eq!(Theme::parse("sepia-pink"), None);
        assert_eq!(ModePref::parse("system"), Some(ModePref::System));
        assert_eq!(ModePref::parse("auto"), None);
        assert_eq!(Accent::parse(" red "), Some(Accent::Red));
    }

    /// Issue #288: coloured rule lines are as quiet as the theme's own: seen against the page,
    /// never loud enough to hurt the words on them, the stronger line stronger, in every theme.
    #[test]
    fn coloured_rule_lines_stay_quiet_in_every_theme() {
        for theme in Theme::all() {
            let t = theme.tokens();
            assert_eq!(rule_lines_in(&t, None), (t.rule_line, t.rule_line_strong));
            for c in nocturne_model::NodeColor::ALL {
                let (line, strong) = rule_lines_in(&t, Some(c));
                let name = format!("{} {c:?}", theme.label());
                for (what, got, min) in [("line", line, 1.2), ("strong line", strong, 1.7)] {
                    let k = contrast(got, t.editor);
                    assert!(k >= min, "{name}: the {what} cannot be seen {k:.2}");
                    assert!(k <= 4.5, "{name}: the {what} is too loud {k:.2}");
                    assert!(contrast(t.text, got) >= 4.5, "{name}: text over the {what}");
                }
                assert!(contrast(strong, t.editor) > contrast(line, t.editor), "{name}: the strong line is not stronger");
            }
        }
    }

    /// WCAG: body text 7:1 (AAA), secondary text & accent_text & button text 4.5:1 (AA),
    /// borders & disabled & icons 3:1.
    #[test]
    fn every_theme_is_readable() {
        for theme in Theme::all() {
            let t = theme.tokens();
            let name = theme.label();
            assert!(contrast(t.text, t.panel) >= 7.0, "{name}: text on panel {:.2}", contrast(t.text, t.panel));
            assert!(contrast(t.text, t.surface_elevated) >= 7.0, "{name}: text on elevated {:.2}", contrast(t.text, t.surface_elevated));
            assert!(contrast(t.text, t.editor) >= 7.0, "{name}: text on editor {:.2}", contrast(t.text, t.editor));
            assert!(contrast(t.text, t.faint) >= 7.0, "{name}: text on faint {:.2}", contrast(t.text, t.faint));
            assert!(contrast(t.container_border, t.editor) >= 2.5, "{name}: container border {:.2}", contrast(t.container_border, t.editor));
            assert!(contrast(t.border, t.panel) >= 3.0, "{name}: border on panel {:.2}", contrast(t.border, t.panel));
            assert!(contrast(t.border, t.surface_elevated) >= 3.0, "{name}: border on elevated {:.2}", contrast(t.border, t.surface_elevated));
            assert!(contrast(t.border_subtle, t.panel) >= 1.5, "{name}: subtle border on panel {:.2}", contrast(t.border_subtle, t.panel));
            assert!(
                contrast(t.border_subtle, t.surface_elevated) >= 1.5,
                "{name}: subtle border on elevated {:.2}",
                contrast(t.border_subtle, t.surface_elevated)
            );
            assert!(contrast(t.border, t.panel) > contrast(t.border_subtle, t.panel), "{name}: border must be stronger than border_subtle");
            assert!(contrast(t.disabled, t.panel) >= 3.0, "{name}: disabled on panel {:.2}", contrast(t.disabled, t.panel));
            assert!(contrast(t.disabled, t.surface_elevated) >= 3.0, "{name}: disabled on elevated {:.2}", contrast(t.disabled, t.surface_elevated));
            assert!(contrast(t.disabled, t.panel) < contrast(t.muted, t.panel), "{name}: disabled should be quieter than muted");
            assert!(contrast(t.strong, t.panel) >= 7.0, "{name}: strong text");
            assert!(contrast(t.strong, t.surface_elevated) >= 7.0, "{name}: strong text on elevated");
            assert!(contrast(t.muted, t.panel) >= 4.5, "{name}: muted text {:.2}", contrast(t.muted, t.panel));
            assert!(contrast(t.muted, t.surface_elevated) >= 4.5, "{name}: muted text on elevated {:.2}", contrast(t.muted, t.surface_elevated));
            assert!(contrast(t.muted, t.code_bg) >= 3.5, "{name}: muted on code background {:.2}", contrast(t.muted, t.code_bg));
            assert!(contrast(t.link, t.editor) >= 4.5, "{name}: link {:.2}", contrast(t.link, t.editor));
            assert!(contrast(t.accent, t.panel) >= 3.0, "{name}: accent {:.2}", contrast(t.accent, t.panel));
            assert!(contrast(t.accent, t.surface_elevated) >= 3.0, "{name}: accent on elevated {:.2}", contrast(t.accent, t.surface_elevated));
            assert!(contrast(t.accent_text, t.panel) >= 4.5, "{name}: accent_text on panel {:.2}", contrast(t.accent_text, t.panel));
            assert!(
                contrast(t.accent_text, t.surface_elevated) >= 4.5,
                "{name}: accent_text on elevated {:.2}",
                contrast(t.accent_text, t.surface_elevated)
            );
            // Primary and danger buttons: WCAG AA text on both rest and hover states,
            // and visible separation from elevated modal surfaces.
            assert!(
                contrast(t.btn_primary_text, t.btn_primary_bg) >= 4.5,
                "{name}: primary button text {:.2}",
                contrast(t.btn_primary_text, t.btn_primary_bg)
            );
            assert!(
                contrast(t.btn_primary_text, t.btn_primary_hover) >= 4.5,
                "{name}: primary button hover text {:.2}",
                contrast(t.btn_primary_text, t.btn_primary_hover)
            );
            assert!(
                contrast(t.btn_primary_bg, t.surface_elevated) >= 3.0,
                "{name}: primary button vs elevated {:.2}",
                contrast(t.btn_primary_bg, t.surface_elevated)
            );
            assert_ne!(t.btn_primary_bg, t.btn_primary_hover, "{name}: primary hover must differ from rest");
            assert!(
                contrast(t.btn_danger_text, t.btn_danger_bg) >= 4.5,
                "{name}: danger button text {:.2}",
                contrast(t.btn_danger_text, t.btn_danger_bg)
            );
            assert!(
                contrast(t.btn_danger_text, t.btn_danger_hover) >= 4.5,
                "{name}: danger button hover text {:.2}",
                contrast(t.btn_danger_text, t.btn_danger_hover)
            );
            assert!(
                contrast(t.btn_danger_bg, t.surface_elevated) >= 3.0,
                "{name}: danger button vs elevated {:.2}",
                contrast(t.btn_danger_bg, t.surface_elevated)
            );
            assert_ne!(t.btn_danger_bg, t.btn_danger_hover, "{name}: danger hover must differ from rest");
            assert!(
                contrast(Color32::WHITE, t.close_hover) >= 4.5,
                "{name}: white glyph on close_hover {:.2}",
                contrast(Color32::WHITE, t.close_hover)
            );
            assert!(t.scrim.a() >= 90 && t.scrim.a() <= 200, "{name}: scrim alpha {}", t.scrim.a());

            for (what, c) in [("group icon", t.group), ("section icon", t.section), ("ok", t.ok)] {
                assert!(contrast(c, t.panel) >= 3.0, "{name}: {what} {:.2}", contrast(c, t.panel));
            }
            assert!(contrast(t.warn, t.panel) >= 4.5, "{name}: warning text {:.2}", contrast(t.warn, t.panel));
            // A selection on the page must be seen against the page and still be readable.
            assert!(
                contrast(t.selection, t.editor) >= 1.25,
                "{name}: the selection on the page is {:.2} against it, which cannot be seen",
                contrast(t.selection, t.editor)
            );
            assert!(contrast(t.text, t.selection) >= 7.0, "{name}: text on a selection {:.2}", contrast(t.text, t.selection));
            assert!(contrast(t.strong, t.selection) >= 7.0, "{name}: strong text on a selection");
            assert!(contrast(t.text, t.row_selected) >= 7.0, "{name}: text on the selected row {:.2}", contrast(t.text, t.row_selected));
            assert!(contrast(t.strong, t.row_selected) >= 7.0, "{name}: strong text on the selected row");
            assert!(contrast(t.text, t.row_hover) >= 7.0, "{name}: text on a hovered row");
            // The band across the top: white writing on it, and on it under the pointer.
            assert!(contrast(t.band_text, t.band) >= 4.5, "{name}: writing on the band {:.2}", contrast(t.band_text, t.band));
            assert!(
                contrast(t.band_text, t.band_hover) >= 4.5,
                "{name}: writing on a hovered band button {:.2}",
                contrast(t.band_text, t.band_hover)
            );
            assert!(contrast(t.band_dim, t.band) >= 2.0, "{name}: greyed writing on the band {:.2}", contrast(t.band_dim, t.band));
            // Rule and grid lines on a page: visible against the page, but not loud, and
            // they must not hurt the text that sits on top of them.
            for (what, c, min) in [("rule line", t.rule_line, 1.25), ("strong rule line", t.rule_line_strong, 1.8)] {
                let got = contrast(c, t.editor);
                assert!(got >= min, "{name}: {what} is invisible on the page {got:.2}");
                assert!(got <= 4.5, "{name}: {what} is too loud for a page background {got:.2}");
                assert!(contrast(t.text, c) >= 4.5, "{name}: text over a {what} {:.2}", contrast(t.text, c));
            }
            assert!(contrast(t.rule_line_strong, t.editor) > contrast(t.rule_line, t.editor), "{name}: the strong line is not stronger");
        }
    }

    #[test]
    fn light_themes_are_light_and_dark_themes_are_dark() {
        for theme in Theme::all() {
            let l = luminance(theme.tokens().panel);
            if theme.mode.is_dark() {
                assert!(l < 0.1, "{}: {l}", theme.label());
            } else {
                assert!(l > 0.8, "{}: {l}", theme.label());
            }
        }
    }

    #[test]
    fn visuals_follow_the_tokens() {
        for theme in Theme::all() {
            let t = theme.tokens();
            let v = theme.visuals();
            let name = theme.label();
            assert_eq!(v.dark_mode, theme.mode.is_dark(), "{name}: dark_mode");
            assert_eq!(v.panel_fill, t.panel, "{name}: panel_fill");
            assert_eq!(v.window_fill, t.surface_elevated, "{name}: window_fill");
            assert_eq!(v.window_stroke.color, t.border, "{name}: window_stroke");
            assert_eq!(v.window_corner_radius, egui::CornerRadius::same(RADIUS_MODAL), "{name}: window_corner_radius");
            assert_eq!(v.window_shadow, t.shadow_floating(), "{name}: window_shadow");
            assert_eq!(v.menu_corner_radius, egui::CornerRadius::same(RADIUS_POPUP), "{name}: menu_corner_radius");
            assert_eq!(v.popup_shadow, t.shadow_popup(), "{name}: popup_shadow");
            assert_eq!(v.selection.bg_fill, t.row_selected, "{name}: selection wash");
            assert_eq!(v.selection.stroke.color, t.accent, "{name}: selection stroke");
            assert_eq!(v.text_cursor.stroke.color, t.accent, "{name}: cursor color");
            assert_eq!(v.weak_text_color, Some(t.muted), "{name}: weak_text_color");

            // All 5 widget states keep readable foreground on their background fills.
            for (state, w) in [
                ("noninteractive", &v.widgets.noninteractive),
                ("inactive", &v.widgets.inactive),
                ("hovered", &v.widgets.hovered),
                ("active", &v.widgets.active),
                ("open", &v.widgets.open),
            ] {
                assert!(
                    contrast(w.fg_stroke.color, w.bg_fill) >= 7.0,
                    "{name}: widget state {state} fg/bg contrast {:.2}",
                    contrast(w.fg_stroke.color, w.bg_fill)
                );
                assert_eq!(w.corner_radius, egui::CornerRadius::same(RADIUS_CONTROL), "{name}: {state} corner_radius");
            }
            assert_eq!(v.widgets.noninteractive.bg_stroke.color, t.border_subtle, "{name}: noninteractive stroke");
            assert_eq!(v.widgets.inactive.bg_stroke.color, t.border, "{name}: inactive stroke");
            assert_eq!(v.widgets.hovered.bg_stroke.color, t.accent, "{name}: hovered stroke");
            assert_eq!(v.widgets.active.bg_stroke.color, t.accent, "{name}: active stroke");
            assert_eq!(v.widgets.open.bg_stroke.color, t.accent, "{name}: open stroke");
        }
    }

    #[test]
    fn contrast_math_is_right() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.001);
        assert_eq!(mix(Color32::BLACK, Color32::WHITE, 0.5), Color32::from_rgb(128, 128, 128));
        assert_eq!(mix(Color32::RED, Color32::BLUE, 0.0), Color32::RED);
        let fixed = ensure_contrast(Color32::from_rgb(200, 200, 200), Color32::WHITE, 4.5, false);
        assert!(contrast(fixed, Color32::WHITE) >= 4.5);
    }
}
