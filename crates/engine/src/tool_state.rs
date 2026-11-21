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

/// The edge of the page view the Tablet Studio tool bar sits on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolbarDock {
    #[default]
    Bottom,
    Top,
    Left,
    Right,
}

impl ToolbarDock {
    pub const ALL: [Self; 4] = [Self::Bottom, Self::Top, Self::Left, Self::Right];

    /// The stable identifier used by commands and in the settings file.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Bottom => "bottom",
            Self::Top => "top",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    /// Parses a stable identifier (trimmed, ASCII case-insensitive).
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|dock| dock.id() == raw)
    }

    /// A bar on the left or right edge runs down the page; on the top or bottom it runs across.
    #[must_use]
    pub fn is_vertical(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

/// One of the things the Tablet Studio tool bar can hold. The pens are one entry: which pens,
/// in what order and colours, is the pen rack's business.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockTool {
    Write,
    Pens,
    Eraser,
    Lasso,
    Select,
    Shapes,
    Picture,
    MathOcr,
    History,
}

impl DockTool {
    /// Every tool, in the order a fresh install shows them.
    pub const ALL: [Self; 9] =
        [Self::History, Self::Write, Self::Pens, Self::Eraser, Self::Lasso, Self::Select, Self::Shapes, Self::Picture, Self::MathOcr];

    /// The stable identifier used by commands and in the settings file.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Pens => "pens",
            Self::Eraser => "eraser",
            Self::Lasso => "lasso",
            Self::Select => "select",
            Self::Shapes => "shapes",
            Self::Picture => "picture",
            Self::MathOcr => "math_ocr",
            Self::History => "history",
        }
    }

    /// Parses a stable identifier (trimmed, ASCII case-insensitive).
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim().to_ascii_lowercase();
        if raw == "math-ocr" {
            return Some(Self::MathOcr);
        }
        Self::ALL.into_iter().find(|tool| tool.id() == raw)
    }
}

/// Which tools the tool bar shows, and in what order. Every tool is always in the list once:
/// a hidden tool keeps its place, so showing it again puts it back where it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DockRoster {
    tools: [(DockTool, bool); 9],
}

impl Default for DockRoster {
    fn default() -> Self {
        Self { tools: DockTool::ALL.map(|tool| (tool, true)) }
    }
}

impl DockRoster {
    /// Every tool in order, each with whether it is shown.
    #[must_use]
    pub fn all(&self) -> [(DockTool, bool); 9] {
        self.tools
    }

    /// The tools that are shown, in order.
    pub fn shown(&self) -> impl Iterator<Item = DockTool> + '_ {
        self.tools.iter().filter(|(_, shown)| *shown).map(|(tool, _)| *tool)
    }

    /// Shows or hides one tool.
    pub fn set_shown(&mut self, tool: DockTool, shown: bool) {
        if let Some(entry) = self.tools.iter_mut().find(|(t, _)| *t == tool) {
            entry.1 = shown;
        }
    }

    /// Moves one tool a place earlier or later. Returns whether it moved.
    pub fn shift(&mut self, tool: DockTool, earlier: bool) -> bool {
        let Some(at) = self.tools.iter().position(|(t, _)| *t == tool) else { return false };
        let to = if earlier { at.checked_sub(1) } else { Some(at + 1).filter(|to| *to < self.tools.len()) };
        let Some(to) = to else { return false };
        self.tools.swap(at, to);
        true
    }

    /// The list as it is kept in the settings file and given to `ink.configure_tools`: the
    /// tools in order, comma separated, with a minus before a hidden one.
    #[must_use]
    pub fn write(&self) -> String {
        self.tools.iter().map(|(tool, shown)| format!("{}{}", if *shown { "" } else { "-" }, tool.id())).collect::<Vec<_>>().join(",")
    }

    /// Reads such a list. A name it does not know is an error, so a command with a slip in it
    /// changes nothing; a tool the list leaves out keeps its usual place after the ones named,
    /// shown; a tool named twice counts the first time.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let mut named: Vec<(DockTool, bool)> = Vec::new();
        for part in raw.split(',').map(str::trim).filter(|part| !part.is_empty()) {
            let (shown, name) = part.strip_prefix('-').map_or((true, part), |rest| (false, rest.trim()));
            let tool = DockTool::parse(name).ok_or_else(|| format!("{name:?} is not a tool"))?;
            if !named.iter().any(|(t, _)| *t == tool) {
                named.push((tool, shown));
            }
        }
        for tool in DockTool::ALL {
            if !named.iter().any(|(t, _)| *t == tool) {
                named.push((tool, true));
            }
        }
        let mut roster = Self::default();
        for (slot, entry) in roster.tools.iter_mut().zip(named) {
            *slot = entry;
        }
        Ok(roster)
    }
}

/// How Tablet Studio presents notebooks and pages when browsing the library.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LibraryView {
    #[default]
    StudioGrid,
    ClassicTree,
}

impl LibraryView {
    pub const ALL: [Self; 2] = [Self::StudioGrid, Self::ClassicTree];

    /// The stable identifier used by commands and in the settings file.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::StudioGrid => "studio_grid",
            Self::ClassicTree => "classic_tree",
        }
    }

    /// Parses a stable identifier (trimmed, ASCII case-insensitive).
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "studio_grid" | "studio-grid" | "studiogrid" | "grid" => Some(Self::StudioGrid),
            "classic_tree" | "classic-tree" | "classictree" | "tree" | "classic" => Some(Self::ClassicTree),
            _ => None,
        }
    }
}

/// How Tablet Studio is laid out: where the tool bar is, and what is on it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StudioOptions {
    pub toolbar_dock: ToolbarDock,
    pub roster: DockRoster,
    pub library_view: LibraryView,
}

impl StudioOptions {
    pub(crate) const TOOLBAR_DOCK_KEY: &'static str = "toolbar_dock";
    pub(crate) const DOCK_TOOLS_KEY: &'static str = "studio_dock_tools";
    pub(crate) const LIBRARY_VIEW_KEY: &'static str = "library_view";

    /// Reads the layout from the settings file. Anything missing or unreadable is the default.
    pub(crate) fn load(settings: &nocturne_store::Settings) -> Self {
        Self {
            toolbar_dock: settings.get(Self::TOOLBAR_DOCK_KEY).and_then(|v| ToolbarDock::parse(&v)).unwrap_or_default(),
            roster: settings.get(Self::DOCK_TOOLS_KEY).and_then(|v| DockRoster::parse(&v).ok()).unwrap_or_default(),
            library_view: settings.get(Self::LIBRARY_VIEW_KEY).and_then(|v| LibraryView::parse(&v)).unwrap_or_default(),
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

    #[test]
    fn dock_tool_and_library_view_round_trip() {
        assert_eq!(DockTool::ALL.len(), 9);
        for tool in DockTool::ALL {
            assert_eq!(DockTool::parse(tool.id()), Some(tool));
        }
        assert_eq!(DockTool::Picture.id(), "picture");
        assert_eq!(DockTool::MathOcr.id(), "math_ocr");
        assert_eq!(DockTool::parse("math-ocr"), Some(DockTool::MathOcr));
        assert_eq!(DockRoster::default().all().len(), 9);
        for view in LibraryView::ALL {
            assert_eq!(LibraryView::parse(view.id()), Some(view));
        }
        assert_eq!(LibraryView::parse("StudioGrid"), Some(LibraryView::StudioGrid));
        assert_eq!(LibraryView::parse("classic-tree"), Some(LibraryView::ClassicTree));
        assert_eq!(LibraryView::parse("unknown"), None);
    }
}
