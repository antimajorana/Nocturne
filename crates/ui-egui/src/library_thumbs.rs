//! Miniature page thumbnail extraction, stroke normalization, painted card/pill primitives,
//! and notebook tree query helpers for the Tablet Studio two-pane library grid.
//!
//! All disk reads go through `nocturne_store` so browser/wasm builds using the virtual notebook
//! shim work without platform-specific file code here.

use crate::icons::{self, Icon};
use crate::theme::{Tokens, luminance, mix, readable_on};
use egui::{Color32, CornerRadius, FontId, Id, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use nocturne_formats::boxes;
use nocturne_formats::markdown::{Block, Span};
use nocturne_ink::{Drawing, Rgb, Tool};
use nocturne_model::{NodeColor, NodeKind};
use nocturne_store::tree::Node;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const CARD_MIN_WIDTH: f32 = 172.0;
pub const CARD_GAP: f32 = 16.0;
const MAX_THUMB_LINES: usize = 8;
const MAX_THUMB_STROKES: usize = 200;
const MAX_PTS_PER_STROKE: usize = 32;

/// One plain-text preview line extracted from a page's body blocks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbLine {
    pub text: String,
    pub heading: bool,
}

/// One normalized `[0.0..=1.0, 0.0..=1.0]` stroke polyline for miniature card rendering.
#[derive(Clone, Debug, PartialEq)]
pub struct ThumbStroke {
    pub pts: Vec<[f32; 2]>,
    pub colour: Rgb,
    pub width: f32,
    pub highlighter: bool,
}

/// Cached miniature preview data for a single page card in the library grid.
#[derive(Clone, Debug)]
pub struct CachedThumb {
    pub title: String,
    pub paper: Option<String>,
    pub page_colour: Option<NodeColor>,
    pub aspect: f32,
    pub lines: Vec<ThumbLine>,
    pub strokes: Vec<ThumbStroke>,
}

/// Computes `(columns, card_size, preview_height)` for a responsive card grid of width `avail_w`.
#[must_use]
pub fn card_grid_metrics(avail_w: f32) -> (usize, Vec2, f32) {
    let clamped_w = avail_w.max(CARD_MIN_WIDTH);
    let cols = ((clamped_w + CARD_GAP) / (CARD_MIN_WIDTH + CARD_GAP)).floor().max(1.0) as usize;
    let card_w = ((clamped_w - CARD_GAP * (cols.saturating_sub(1) as f32)) / cols as f32).clamp(136.0, 260.0);
    let preview_h = (card_w * 0.72).clamp(112.0, 168.0);
    (cols, vec2(card_w, preview_h + 42.0), preview_h)
}

/// Builds a [`CachedThumb`] from a page folder on disk via `nocturne_store::page`.
#[must_use]
pub fn build_thumb(page_dir: &Path, fallback_title: &str) -> CachedThumb {
    let (paper, raw_colour) = nocturne_store::page::appearance(page_dir).ok().unwrap_or((None, None));
    let page_colour = raw_colour.as_deref().and_then(NodeColor::parse);
    let mut title = fallback_title.to_string();
    let mut lines = Vec::new();
    if let Ok(page) = nocturne_store::page::load_page(page_dir) {
        if !page.title.trim().is_empty() {
            title = page.title;
        }
        lines = extract_body_lines(&boxes::read(page.format, &page.body).0);
    }

    let (aspect, strokes) =
        nocturne_store::page::load_ink_drawing(page_dir).map_or((620.0 / 420.0, Vec::new()), |drawing| normalize_drawing(&drawing));
    CachedThumb { title, paper, page_colour, aspect, lines, strokes }
}

/// Extracts up to `MAX_THUMB_LINES` compact text lines from parsed page blocks, omitting `ink.svg` placeholders.
#[must_use]
pub fn extract_body_lines(blocks: &[Block]) -> Vec<ThumbLine> {
    let mut lines = Vec::new();
    for block in blocks {
        if lines.len() >= MAX_THUMB_LINES {
            break;
        }
        extract_block_lines(block, &mut lines);
    }
    lines
}

/// Normalizes a [`Drawing`] into `(aspect_ratio, strokes)` with coordinates in `0.0..=1.0`.
#[must_use]
pub fn normalize_drawing(drawing: &Drawing) -> (f32, Vec<ThumbStroke>) {
    let mut world = Rect::from_min_size(Pos2::ZERO, vec2(drawing.width.max(620.0), drawing.height.max(420.0)));
    for d in drawing.strokes.iter().flat_map(|s| &s.dabs).filter(|d| d.x.is_finite() && d.y.is_finite()) {
        world.extend_with(pos2(d.x, d.y));
    }
    let (ww, wh) = (world.width().max(1.0), world.height().max(1.0));
    let aspect = (ww / wh).clamp(0.5, 2.4);
    let mut strokes = Vec::new();
    for s in drawing.strokes.iter().take(MAX_THUMB_STROKES) {
        let finite: Vec<_> = s.dabs.iter().filter(|d| d.x.is_finite() && d.y.is_finite()).collect();
        if finite.is_empty() {
            continue;
        }
        let step = (finite.len() / MAX_PTS_PER_STROKE).max(1);
        let norm = |x: f32, y: f32| [((x - world.min.x) / ww).clamp(0.0, 1.0), ((y - world.min.y) / wh).clamp(0.0, 1.0)];
        let mut pts: Vec<[f32; 2]> = finite.iter().step_by(step).map(|d| norm(d.x, d.y)).collect();
        if let Some(last) = finite.last().map(|d| norm(d.x, d.y))
            && pts.last() != Some(&last)
        {
            pts.push(last);
        }
        strokes.push(ThumbStroke { pts, colour: s.colour, width: s.width, highlighter: s.tool == Tool::Highlighter });
    }
    (aspect, strokes)
}

/// Paints a visual page thumbnail card and its centered, space-preserving title; returns `true` when clicked.
pub fn draw_page_card(ui: &mut Ui, title: &str, thumb: &CachedThumb, card_size: Vec2, preview_h: f32, t: &Tokens) -> bool {
    let (rect, resp) = ui.allocate_exact_size(card_size, Sense::click());
    let owned_title = title.to_string();
    resp.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &owned_title));

    let preview_rect = Rect::from_min_size(rect.min, vec2(card_size.x, preview_h));
    let preview_bg = thumb.page_colour.map_or(t.editor, |nc| {
        let (r, g, b) = nc.rgb(t.editor.r() < 128);
        mix(t.editor, Color32::from_rgb(r, g, b), 0.16)
    });
    let dark_page = luminance(preview_bg) < 0.35;
    ui.painter().rect_filled(preview_rect, CornerRadius::same(10), preview_bg);

    let clip = preview_rect.shrink(6.0);
    let painter = ui.painter_at(clip);
    paint_paper_rules(&painter, clip, thumb.paper.as_deref(), t);
    paint_thumb_lines(&painter, clip, thumb, t);
    paint_thumb_strokes(&painter, clip, thumb, dark_page);

    let border = if resp.hovered() { Stroke::new(1.6, t.accent) } else { Stroke::new(1.0, t.container_border) };
    ui.painter().rect_stroke(preview_rect, CornerRadius::same(10), border, StrokeKind::Inside);

    let title_area = Rect::from_min_max(pos2(rect.min.x + 4.0, preview_rect.max.y + 6.0), pos2(rect.max.x - 4.0, rect.max.y));
    let job = egui::text::LayoutJob::simple(title.to_string(), FontId::proportional(13.0), t.text, title_area.width().max(24.0));
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let gx = (rect.center().x - galley.size().x * 0.5).max(title_area.min.x);
    ui.painter_at(title_area).galley(pos2(gx, title_area.min.y), galley, t.text);
    resp.clicked()
}

/// Paints a rounded left-sidebar row with optional disclosure chevron, icon, title, and page-only count.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sidebar_pill(
    ui: &mut Ui,
    key: &Path,
    icon: Icon,
    icon_color: Color32,
    title: &str,
    count: usize,
    active: bool,
    depth: usize,
    disclosure: Option<bool>,
    t: &Tokens,
) -> (bool, bool) {
    let indent = (depth as f32 * 14.0).min(42.0);
    let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width().max(120.0), 34.0), Sense::hover());
    let pill = Rect::from_min_max(pos2(outer.min.x + indent, outer.min.y), outer.max);
    let disc_w = if disclosure.is_some() { 20.0 } else { 0.0 };
    let disc_rect = Rect::from_min_max(pill.min, pos2(pill.min.x + disc_w, pill.max.y));
    let main_rect = Rect::from_min_max(pos2(pill.min.x + disc_w, pill.min.y), pill.max);

    let main_resp = ui.interact(main_rect, Id::unique(("library-pill", key)), Sense::click());
    let a11y = format!("{title} ({count})");
    main_resp.widget_info(move || egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, &a11y));
    let disc_clicked = disclosure.is_some() && ui.interact(disc_rect, Id::unique(("library-disc", key)), Sense::click()).clicked();

    if active {
        ui.painter().rect_filled(pill, CornerRadius::same(10), t.band);
    } else if main_resp.hovered() {
        ui.painter().rect_filled(pill, CornerRadius::same(10), t.row_hover);
    }

    let (fg, count_fg, ico_fg) = if active { (t.band_text, t.band_text, t.band_text) } else { (t.text, t.muted, icon_color) };
    if let Some(expanded) = disclosure {
        let c = disc_rect.center();
        let pts = if expanded {
            vec![pos2(c.x - 4.0, c.y - 2.0), pos2(c.x, c.y + 2.5), pos2(c.x + 4.0, c.y - 2.0)]
        } else {
            vec![pos2(c.x - 2.0, c.y - 4.0), pos2(c.x + 2.5, c.y), pos2(c.x - 2.0, c.y + 4.0)]
        };
        ui.painter().add(Shape::line(pts, Stroke::new(1.5, count_fg)));
    }

    let ico_x = main_rect.min.x + if disclosure.is_some() { 2.0 } else { 10.0 };
    let ico_rect = Rect::from_center_size(pos2(ico_x + 8.0, pill.center().y), vec2(16.0, 16.0));
    icons::paint(ui.painter(), ico_rect, icon, ico_fg);

    let count_galley = ui.painter().layout_no_wrap(count.to_string(), FontId::proportional(12.0), count_fg);
    let count_x = pill.max.x - 10.0 - count_galley.size().x;
    ui.painter().galley(pos2(count_x, pill.center().y - count_galley.size().y * 0.5), count_galley, count_fg);

    let text_left = ico_rect.max.x + 8.0;
    let text_clip = Rect::from_min_max(pos2(text_left, pill.min.y), pos2((count_x - 6.0).max(text_left + 12.0), pill.max.y));
    let title_galley = ui.painter().layout_no_wrap(title.to_string(), FontId::proportional(13.5), fg);
    ui.painter_at(text_clip).galley(pos2(text_left, pill.center().y - title_galley.size().y * 0.5), title_galley, fg);

    (main_resp.clicked(), disc_clicked)
}

pub(crate) fn page_count(node: &Node) -> usize {
    usize::from(node.info.kind == NodeKind::Page) + node.children.iter().map(page_count).sum::<usize>()
}

pub(crate) fn auto_expand_new_containers(nodes: &[Node], seen: &mut HashSet<PathBuf>, expanded: &mut HashSet<PathBuf>) {
    for n in nodes.iter().filter(|n| n.info.kind != NodeKind::Page) {
        if seen.insert(n.path.clone()) {
            expanded.insert(n.path.clone());
        }
        auto_expand_new_containers(&n.children, seen, expanded);
    }
}

pub(crate) fn first_node_of_kind(nodes: &[Node], want: NodeKind) -> Option<PathBuf> {
    nodes.iter().find_map(|n| if n.info.kind == want { Some(n.path.clone()) } else { first_node_of_kind(&n.children, want) })
}

pub(crate) fn find_node<'a>(nodes: &'a [Node], target: &Path) -> Option<&'a Node> {
    nodes.iter().find_map(|n| if n.path == target { Some(n) } else { find_node(&n.children, target) })
}

pub(crate) fn enclosing_container_path(tree: &[Node], target: &Path) -> Option<PathBuf> {
    std::iter::successors(Some(target), |p| p.parent())
        .find_map(|cand| find_node(tree, cand).filter(|n| n.info.kind != NodeKind::Page).map(|n| n.path.clone()))
}

pub(crate) fn collect_pages(node: &Node, out: &mut Vec<(PathBuf, String)>) {
    if node.info.kind == NodeKind::Page {
        out.push((node.path.clone(), node.info.title.clone()));
    }
    for child in &node.children {
        collect_pages(child, out);
    }
}

pub(crate) fn resolve_recents(root: &Path, tree: &[Node], recents: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let all = all_open_pages(tree);
    let mut out = Vec::new();
    for raw in recents {
        let cand = if raw.is_relative() { root.join(raw) } else { raw.clone() };
        if let Some((path, title)) = all.iter().find(|(p, _)| *p == cand)
            && !out.iter().any(|(p, _)| p == path)
        {
            out.push((path.clone(), title.clone()));
        }
    }
    out
}

pub(crate) fn resolve_search_cards(tree: &[Node], hits: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let all = all_open_pages(tree);
    let mut out = Vec::new();
    for hit in hits {
        if out.iter().any(|(p, _)| p == hit) {
            continue;
        }
        if let Some((path, title)) = all.iter().find(|(p, _)| p == hit) {
            out.push((path.clone(), title.clone()));
        } else if let Some(info) = nocturne_store::tree::read_node(hit).filter(|i| i.kind == NodeKind::Page) {
            out.push((hit.clone(), info.title));
        }
    }
    out
}

pub(crate) fn dedup_paths(paths: &mut Vec<PathBuf>) {
    let mut seen = HashSet::new();
    paths.retain(|p| seen.insert(p.clone()));
}

pub(crate) fn rewrite_prefix(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    let rel = path.strip_prefix(from).ok()?;
    Some(if rel.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rel) })
}

pub(crate) fn node_visuals(node: &Node, is_expanded: bool, t: &Tokens) -> (Icon, Color32) {
    let icon = match node.info.kind {
        NodeKind::Notebook => Icon::Notebook,
        NodeKind::SectionGroup if is_expanded => Icon::SectionGroupOpen,
        NodeKind::SectionGroup => Icon::SectionGroup,
        NodeKind::Section | NodeKind::Page => Icon::Section,
    };
    let dark = t.panel.r() < 128;
    let color = match node.info.color {
        Some(nc) => {
            let (r, g, b) = nc.rgb(dark);
            readable_on(Color32::from_rgb(r, g, b), t.panel, 3.0, dark)
        }
        None => match node.info.kind {
            NodeKind::Notebook => t.notebook,
            NodeKind::SectionGroup => t.group,
            NodeKind::Section | NodeKind::Page => t.section,
        },
    };
    (icon, color)
}

pub(crate) fn target_section_for_path(tree: &[Node], scope_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = scope_path
        && let Some(node) = find_node(tree, path)
    {
        if node.info.kind == NodeKind::Section {
            return Some(node.path.clone());
        }
        if let Some(sec) = first_node_of_kind(&node.children, NodeKind::Section) {
            return Some(sec);
        }
        if node.info.kind != NodeKind::Page {
            return Some(node.path.clone());
        }
    }
    first_node_of_kind(tree, NodeKind::Section).or_else(|| first_node_of_kind(tree, NodeKind::Notebook))
}

pub(crate) fn resolve_section_parent_for_path(tree: &[Node], scope_path: Option<&Path>) -> Option<(PathBuf, NodeKind)> {
    if let Some(path) = scope_path
        && let Some(found) = std::iter::successors(Some(path), |p| p.parent()).find_map(|cand| {
            find_node(tree, cand)
                .filter(|n| matches!(n.info.kind, NodeKind::Notebook | NodeKind::SectionGroup))
                .map(|n| (n.path.clone(), n.info.kind))
        })
    {
        return Some(found);
    }
    tree.first().map(|nb| (nb.path.clone(), nb.info.kind))
}

fn all_open_pages(tree: &[Node]) -> Vec<(PathBuf, String)> {
    let mut pages = Vec::new();
    for nb in tree {
        collect_pages(nb, &mut pages);
    }
    pages
}

fn paint_paper_rules(painter: &egui::Painter, clip: Rect, paper: Option<&str>, t: &Tokens) {
    let Some(paper) = paper.filter(|p| *p != "none") else { return };
    let mut y = clip.min.y + 16.0;
    while y < clip.max.y {
        painter.line_segment([pos2(clip.min.x, y), pos2(clip.max.x, y)], Stroke::new(0.7, t.rule_line));
        y += 16.0;
    }
    if paper.starts_with("grid") || paper == "craft-grid" {
        let mut x = clip.min.x + 16.0;
        while x < clip.max.x {
            painter.line_segment([pos2(x, clip.min.y), pos2(x, clip.max.y)], Stroke::new(0.7, t.rule_line));
            x += 16.0;
        }
    }
}

fn paint_thumb_lines(painter: &egui::Painter, clip: Rect, thumb: &CachedThumb, t: &Tokens) {
    let mut line_y = clip.min.y + 4.0;
    let body_col = if thumb.strokes.is_empty() { t.text } else { t.muted };
    for line in thumb.lines.iter().take(6) {
        if line_y + 11.0 > clip.max.y {
            break;
        }
        let col = if line.heading { t.text } else { body_col };
        let galley = painter.layout_no_wrap(line.text.clone(), FontId::proportional(if line.heading { 10.5 } else { 9.2 }), col);
        painter.galley(pos2(clip.min.x + 4.0, line_y), galley, col);
        line_y += 13.0;
    }
}

fn paint_thumb_strokes(painter: &egui::Painter, clip: Rect, thumb: &CachedThumb, dark_page: bool) {
    if thumb.strokes.is_empty() {
        return;
    }
    let fit_w = clip.width().min(clip.height() * thumb.aspect);
    let canvas = Rect::from_center_size(clip.center(), vec2(fit_w.max(1.0), (fit_w / thumb.aspect).min(clip.height()).max(1.0)));
    for stroke in &thumb.strokes {
        let Rgb(r, g, b) = crate::inkpad::shown_colour(stroke.colour, dark_page);
        let col = if stroke.highlighter { Color32::from_rgb(r, g, b).gamma_multiply(0.42) } else { Color32::from_rgb(r, g, b) };
        let w = (stroke.width * (canvas.width() / 620.0)).clamp(1.1, if stroke.highlighter { 4.8 } else { 2.6 });
        let pts: Vec<Pos2> =
            stroke.pts.iter().map(|&[nx, ny]| pos2(canvas.min.x + nx * canvas.width(), canvas.min.y + ny * canvas.height())).collect();
        if pts.len() == 1 {
            painter.circle_filled(pts[0], w * 0.6, col);
        } else if pts.len() >= 2 {
            painter.add(Shape::line(pts, Stroke::new(w, col)));
        }
    }
}

fn extract_block_lines(block: &Block, out: &mut Vec<ThumbLine>) {
    let push = |raw: &str, heading: bool, dst: &mut Vec<ThumbLine>| {
        for part in raw.lines() {
            if dst.len() >= MAX_THUMB_LINES {
                break;
            }
            let trimmed = part.split_whitespace().collect::<Vec<_>>().join(" ");
            if !trimmed.is_empty() {
                dst.push(ThumbLine { text: trimmed.chars().take(64).collect(), heading });
            }
        }
    };
    match block {
        Block::Heading { spans, .. } => push(&spans_plain(spans), true, out),
        Block::Paragraph(spans) | Block::Quote(spans) | Block::Item { spans, .. } => push(&spans_plain(spans), false, out),
        Block::Code { text, .. } => push(text, false, out),
        Block::MathBlock(tex) => push(tex, false, out),
        Block::Table { head, rows } => {
            push(&head.iter().map(|c| spans_plain(c)).collect::<Vec<_>>().join(" | "), true, out);
            for row in rows.iter().take(3) {
                push(&row.iter().map(|c| spans_plain(c)).collect::<Vec<_>>().join(" | "), false, out);
            }
        }
        Block::Image { alt, src, .. } if !src.eq_ignore_ascii_case("ink.svg") => push(alt, false, out),
        Block::Image { .. } | Block::Rule => {}
    }
}

fn spans_plain(spans: &[Span]) -> String {
    spans.iter().filter(|sp| !sp.image.as_deref().is_some_and(|src| src.eq_ignore_ascii_case("ink.svg"))).map(|sp| sp.text.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_ink::svg::Flavour;
    use nocturne_ink::{Dab, Stroke as InkStroke};
    use nocturne_store::testing::TempDir;
    use nocturne_store::tree::NodeInfo;

    fn sample_node(path: &str, kind: NodeKind, title: &str, children: Vec<Node>) -> Node {
        Node {
            path: PathBuf::from(path),
            info: NodeInfo { kind, title: title.to_string(), color: None, format: None, id: String::new(), order: 0 },
            children,
        }
    }

    #[test]
    fn library_grid_and_thumbs_modules_stay_under_600_lines() {
        let grid_lines = include_str!("library_grid.rs").lines().count();
        let thumb_lines = include_str!("library_thumbs.rs").lines().count();
        assert!((50..600).contains(&grid_lines), "library_grid.rs must stay under 600 lines (got {grid_lines})");
        assert!((50..600).contains(&thumb_lines), "library_thumbs.rs must stay under 600 lines (got {thumb_lines})");
    }

    #[test]
    fn page_count_counts_page_descendants_only() {
        let tree = sample_node(
            "/nb/atlas",
            NodeKind::Notebook,
            "Studio Atlas",
            vec![sample_node(
                "/nb/atlas/lectures",
                NodeKind::Section,
                "Lecture Notes",
                vec![
                    sample_node("/nb/atlas/lectures/p1", NodeKind::Page, "Wave Packet Derivation", Vec::new()),
                    sample_node("/nb/atlas/lectures/p2", NodeKind::Page, "Spring Schedule Overview", Vec::new()),
                ],
            )],
        );
        assert_eq!(page_count(&tree), 2);
        assert_eq!(page_count(&tree.children[0]), 2);
    }

    #[test]
    fn card_grid_metrics_scales_columns_from_portrait_to_landscape() {
        let (landscape_cols, landscape_size, _) = card_grid_metrics(884.0);
        let (portrait_cols, portrait_size, _) = card_grid_metrics(480.0);
        assert!(landscape_cols >= 4, "expected >= 4 columns on 884pt right pane, got {landscape_cols}");
        assert!((2..landscape_cols).contains(&portrait_cols), "portrait narrows column count: {portrait_cols}");
        assert!(landscape_size.x >= 136.0 && portrait_size.x >= 136.0);
    }

    #[test]
    fn build_thumb_loads_real_markdown_lines_and_editable_ink_strokes_while_skipping_ink_placeholder() {
        let tmp = TempDir::new("library-thumbs-build");
        let page_dir = tmp.path().join("wave-packet-derivation");
        std::fs::create_dir_all(&page_dir).expect("create page dir");
        let md = "---\ntitle: Wave Packet Derivation\npage-color: blue\npaper: grid-medium\n---\n\n# Group Velocity\n\nDispersion relation notes\n\n![Ink](ink.svg)\n";
        std::fs::write(page_dir.join("index.md"), md).expect("write index.md");

        let mut drawing = Drawing::new(620.0, 420.0);
        let mut s1 = InkStroke::new(Tool::Pen, Rgb(0x1a, 0x5f, 0xd0), 3.0);
        s1.add(Dab::new(50.0, 80.0, 0.7));
        s1.add(Dab::new(260.0, 140.0, 0.8));
        drawing.add(s1);
        let mut s2 = InkStroke::new(Tool::Highlighter, Rgb(0xf2, 0xc9, 0x4c), 14.0);
        s2.add(Dab::new(60.0, 200.0, 0.5));
        s2.add(Dab::new(300.0, 200.0, 0.5));
        drawing.add(s2);
        std::fs::write(page_dir.join("ink.svg"), nocturne_ink::svg::write(&drawing, Flavour::Editable)).expect("write ink.svg");

        let thumb = build_thumb(&page_dir, "Fallback Title");
        assert_eq!(thumb.title, "Wave Packet Derivation");
        assert_eq!(thumb.page_colour, Some(NodeColor::Blue));
        assert_eq!(thumb.paper.as_deref(), Some("grid-medium"));
        assert_eq!(
            thumb.lines,
            vec![
                ThumbLine { text: "Group Velocity".to_string(), heading: true },
                ThumbLine { text: "Dispersion relation notes".to_string(), heading: false },
            ]
        );
        assert_eq!(thumb.strokes.len(), 2);
        assert!(!thumb.strokes[0].highlighter);
        assert!(thumb.strokes[1].highlighter);
        for st in &thumb.strokes {
            assert!(st.pts.iter().all(|[x, y]| (0.0..=1.0).contains(x) && (0.0..=1.0).contains(y)));
        }
    }
}
