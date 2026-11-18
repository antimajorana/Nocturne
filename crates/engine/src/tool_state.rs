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
