//! Platform-neutral ink/pointer tool state tracked by [`crate::Session`].

use crate::error::EngineError;
use serde_json::{Value, json};

/// Active pointer or interaction mode on the page canvas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InkActiveMode {
    #[default]
    Write,
    Draw,
    Erase,
    Pick,
    Lasso,
    Shape,
    Pan,
    Laser,
}

impl InkActiveMode {
    /// Canonical identifier matching `ribbon::Pointer::id()`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Draw => "draw",
            Self::Erase => "erase",
            Self::Pick => "select",
            Self::Lasso => "lasso",
            Self::Shape => "shape",
            Self::Pan => "pan",
            Self::Laser => "laser",
        }
    }

    /// Parses an active mode name (trimmed, ASCII case-insensitive).
    /// Accepts both canonical `"select"` and `"pick"` for [`Self::Pick`].
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "write" => Some(Self::Write),
            "draw" => Some(Self::Draw),
            "erase" => Some(Self::Erase),
            "select" | "pick" => Some(Self::Pick),
            "lasso" => Some(Self::Lasso),
            "shape" => Some(Self::Shape),
            "pan" => Some(Self::Pan),
            "laser" => Some(Self::Laser),
            _ => None,
        }
    }
}

/// Writing nib kind, matching `nocturne_ink::Tool::id()` 1:1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InkPenKind {
    #[default]
    Pen,
    Highlighter,
    Pencil,
    FountainPen,
    BrushPen,
}

impl InkPenKind {
    /// All writing nib kinds in canonical order.
    pub const ALL: [Self; 5] = [Self::Pen, Self::Highlighter, Self::Pencil, Self::FountainPen, Self::BrushPen];

    /// Canonical identifier matching `nocturne_ink::Tool::id()`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Pen => "pen",
            Self::Highlighter => "highlighter",
            Self::Pencil => "pencil",
            Self::FountainPen => "fountain-pen",
            Self::BrushPen => "brush-pen",
        }
    }

    /// Parses a writing nib kind name (trimmed, ASCII case-insensitive).
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "pen" => Some(Self::Pen),
            "highlighter" => Some(Self::Highlighter),
            "pencil" => Some(Self::Pencil),
            "fountain-pen" => Some(Self::FountainPen),
            "brush-pen" => Some(Self::BrushPen),
            _ => None,
        }
    }
}

/// Target mode for the `ink.toggle_eraser` command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EraserToggleMode {
    #[default]
    Toggle,
    Eraser,
    Pen,
}

impl EraserToggleMode {
    /// Parses a `mode` parameter (trimmed, ASCII case-insensitive).
    pub fn parse(raw: &str) -> Result<Self, EngineError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "toggle" => Ok(Self::Toggle),
            "eraser" => Ok(Self::Eraser),
            "pen" => Ok(Self::Pen),
            _ => Err(EngineError::BadParam("mode must be toggle, eraser, or pen".into())),
        }
    }
}

/// What the eraser takes when it touches a stroke.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EraserKind {
    /// The whole stroke.
    #[default]
    Object,
    /// Only the part under the eraser; the stroke is cut into what is left either side.
    Pixel,
}

impl EraserKind {
    /// The stable identifier used by commands and in the settings file.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::Pixel => "pixel",
        }
    }

    /// Parses a stable identifier (trimmed, ASCII case-insensitive).
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "object" => Some(Self::Object),
            "pixel" => Some(Self::Pixel),
            _ => None,
        }
    }
}

/// The eraser's named sizes and how wide each is, in points across.
pub const ERASER_SIZES: [(&str, f32); 4] = [("s", 6.0), ("m", 12.0), ("l", 24.0), ("xl", 40.0)];

/// The narrowest and widest the eraser may be, in points across.
pub const MIN_ERASER_WIDTH: f32 = 2.0;
pub const MAX_ERASER_WIDTH: f32 = 80.0;

/// The ink choices that are not a pen: remembered in the settings file, the same on every page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InkOptions {
    pub eraser_kind: EraserKind,
    /// How wide the eraser is, in points across. It is the eraser's own, not the pen's.
    pub eraser_width: f32,
    /// A pen held still for two seconds at the end of a nearly straight stroke makes it straight.
    pub hold_to_straighten: bool,
}

impl Default for InkOptions {
    fn default() -> Self {
        // The small eraser is the size the eraser had when it took its width from a thin pen.
        Self { eraser_kind: EraserKind::Object, eraser_width: ERASER_SIZES[0].1, hold_to_straighten: true }
    }
}

impl InkOptions {
    pub(crate) const ERASER_MODE_KEY: &'static str = "eraser_mode";
    pub(crate) const ERASER_WIDTH_KEY: &'static str = "eraser_width";
    pub(crate) const HOLD_TO_STRAIGHTEN_KEY: &'static str = "hold_to_straighten";

    /// Reads the choices from the settings file. Anything missing or unreadable is the default.
    pub(crate) fn load(settings: &nocturne_store::Settings) -> Self {
        let fresh = Self::default();
        Self {
            eraser_kind: settings.get(Self::ERASER_MODE_KEY).and_then(|v| EraserKind::parse(&v)).unwrap_or(fresh.eraser_kind),
            eraser_width: settings.get(Self::ERASER_WIDTH_KEY).and_then(|v| Self::parse_width(&v)).unwrap_or(fresh.eraser_width),
            hold_to_straighten: settings
                .get(Self::HOLD_TO_STRAIGHTEN_KEY)
                .map_or(fresh.hold_to_straighten, |v| !v.trim().eq_ignore_ascii_case("false")),
        }
    }

    /// A width given as a named size (`s`, `m`, `l`, `xl`) or as points across, within the limits.
    #[must_use]
    pub fn parse_width(raw: &str) -> Option<f32> {
        let raw = raw.trim().to_ascii_lowercase();
        if let Some((_, width)) = ERASER_SIZES.iter().find(|(name, _)| *name == raw) {
            return Some(*width);
        }
        raw.parse::<f32>().ok().filter(|w| w.is_finite() && (MIN_ERASER_WIDTH..=MAX_ERASER_WIDTH).contains(w))
    }
}

/// Synchronized ink tool state tracked by [`crate::Session`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InkToolState {
    pub active_mode: InkActiveMode,
    pub previous_writing_tool: InkPenKind,
    pub rack_index: usize,
}

impl InkToolState {
    /// Serializes the tool state to the JSON shape returned by `ink.toggle_eraser`.
    pub fn to_json(self) -> Value {
        json!({
            "active_mode": self.active_mode.id(),
            "eraser": self.active_mode == InkActiveMode::Erase,
            "rack_index": self.rack_index,
            "writing_tool": self.previous_writing_tool.id(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_mode_round_trips_and_handles_select_pick() {
        for mode in [
            InkActiveMode::Write,
            InkActiveMode::Draw,
            InkActiveMode::Erase,
            InkActiveMode::Pick,
            InkActiveMode::Lasso,
            InkActiveMode::Shape,
            InkActiveMode::Pan,
            InkActiveMode::Laser,
        ] {
            assert_eq!(InkActiveMode::parse(mode.id()), Some(mode));
            assert_eq!(InkActiveMode::parse(&format!("  {}  ", mode.id().to_ascii_uppercase())), Some(mode));
        }
        assert_eq!(InkActiveMode::Pick.id(), "select");
        assert_eq!(InkActiveMode::parse("pick"), Some(InkActiveMode::Pick));
        assert_eq!(InkActiveMode::parse(" PICK "), Some(InkActiveMode::Pick));
        assert_eq!(InkActiveMode::parse("unknown"), None);
        assert_eq!(InkActiveMode::parse(""), None);
    }

    #[test]
    fn pen_kind_round_trips_all_variants() {
        for kind in InkPenKind::ALL {
            assert_eq!(InkPenKind::parse(kind.id()), Some(kind));
            assert_eq!(InkPenKind::parse(&format!(" {} ", kind.id().to_ascii_uppercase())), Some(kind));
        }
        assert_eq!(InkPenKind::parse("chalk"), None);
        assert_eq!(InkPenKind::parse(""), None);
    }

    #[test]
    fn eraser_toggle_mode_parses_and_rejects_invalid() {
        assert_eq!(EraserToggleMode::parse("toggle").ok(), Some(EraserToggleMode::Toggle));
        assert_eq!(EraserToggleMode::parse(" ERASER ").ok(), Some(EraserToggleMode::Eraser));
        assert_eq!(EraserToggleMode::parse("Pen").ok(), Some(EraserToggleMode::Pen));
        assert!(matches!(EraserToggleMode::parse(""), Err(EngineError::BadParam(_))));
        assert!(matches!(EraserToggleMode::parse("lasso"), Err(EngineError::BadParam(_))));
    }

    #[test]
    fn ink_tool_state_defaults_and_serializes() {
        let s = InkToolState::default();
        assert_eq!(s.active_mode, InkActiveMode::Write);
        assert_eq!(s.previous_writing_tool, InkPenKind::Pen);
        assert_eq!(s.rack_index, 0);
        assert_eq!(
            s.to_json(),
            json!({
                "active_mode": "write",
                "eraser": false,
                "rack_index": 0,
                "writing_tool": "pen",
            })
        );
    }
}
