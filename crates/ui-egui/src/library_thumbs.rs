//! Miniature page thumbnail extraction, zero-thrash cache, painted card/pill primitives,
//! and notebook tree query helpers for the Tablet Studio two-pane library grid.
//!
//! All disk reads go through `nocturne_store` so browser/wasm builds using the virtual notebook
//! shim work without platform-specific file code here.

use crate::{
    icons::{self, Icon},
    theme::{Tokens, luminance, mix, readable_on},
};
use egui::{Color32, CornerRadius, FontId, Id, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use nocturne_formats::{
    boxes,
    markdown::{Block, Span},
};
use nocturne_ink::{Drawing, Rgb, Tool};
use nocturne_model::{NodeColor, NodeKind};
use nocturne_store::tree::Node;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::SystemTime,
};

pub const CARD_MIN_WIDTH: f32 = 172.0;
pub const CARD_GAP: f32 = 16.0;
const MAX_THUMB_LINES: usize = 8;
const MAX_THUMB_STROKES: usize = 200;
const MAX_PTS_PER_STROKE: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportKind {
    Document,
    Picture,
}

/// Management and import actions emitted by the library grid for `app.rs` to execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryAction {
    Rename { path: PathBuf, title: String },
    Delete { path: PathBuf, title: String, count: usize },
    NewPage { section: PathBuf },
    NewSection { parent: PathBuf, parent_kind: NodeKind },
    ImportFile { section: PathBuf, kind: ImportKind, file: PathBuf },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbLine {
    pub text: String,
    pub heading: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThumbStroke {
    pub pts: Vec<[f32; 2]>,
    pub colour: Rgb,
    pub width: f32,
    pub highlighter: bool,
}

#[derive(Clone, Debug)]
pub struct CachedThumb {
    pub title: String,
    pub paper: Option<String>,
    pub page_colour: Option<NodeColor>,
    pub aspect: f32,
    pub lines: Vec<ThumbLine>,
    pub strokes: Vec<ThumbStroke>,
}

type FileStamp = (u64, Option<SystemTime>);

/// Lightweight on-disk `(len, mtime)` stamp for a page file and optional `ink.svg`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThumbStamp {
    pub page: FileStamp,
    pub ink: FileStamp,
}

impl ThumbStamp {
    #[must_use]
    pub fn of_page(page_dir: &Path) -> Option<Self> {
        let pm = std::fs::metadata(nocturne_store::tree::page_file(page_dir)?.1).ok()?;
        let ink = std::fs::metadata(page_dir.join("ink.svg")).ok().map_or((0, None), |m| (m.len(), m.modified().ok()));
        Some(Self { page: (pm.len(), pm.modified().ok()), ink })
    }
}

struct ThumbEntry {
    thumb: CachedThumb,
    tree_title: String,
    stamp: Option<ThumbStamp>,
    needs_check: bool,
}

/// In-memory thumbnail cache: zero disk I/O in steady state, stamp-checked on bulk invalidation.
#[derive(Default)]
pub struct ThumbCache {
    entries: HashMap<PathBuf, ThumbEntry>,
    builds: usize,
}

impl ThumbCache {
    pub fn get_or_build(&mut self, page_dir: &Path, tree_title: &str) -> &CachedThumb {
        use std::collections::hash_map::Entry;
        let make = || ThumbEntry {
            thumb: build_thumb(page_dir, tree_title),
            tree_title: tree_title.to_string(),
            stamp: ThumbStamp::of_page(page_dir),
            needs_check: false,
        };
        match self.entries.entry(page_dir.to_path_buf()) {
            Entry::Occupied(mut occ) => {
                let e = occ.get_mut();
                let fresh = e.tree_title == tree_title && (!e.needs_check || matches!(ThumbStamp::of_page(page_dir), Some(s) if Some(s) == e.stamp));
                if fresh {
                    e.needs_check = false;
                } else {
                    *e = make();
                    self.builds += 1;
                }
                &occ.into_mut().thumb
            }
            Entry::Vacant(vac) => {
                self.builds += 1;
                &vac.insert(make()).thumb
            }
        }
    }
    pub fn invalidate_page(&mut self, page_dir: &Path) {
        self.entries.remove(page_dir);
    }
    pub fn invalidate_all(&mut self) {
        self.entries.values_mut().for_each(|e| e.needs_check = true);
    }
    pub fn drop_prefix(&mut self, prefix: &Path) {
        self.entries.retain(|k, _| !k.starts_with(prefix));
    }
    #[must_use]
    pub fn cached_counts(&self, page_dir: &Path) -> Option<(usize, usize)> {
        self.entries.get(page_dir).map(|e| (e.thumb.lines.len(), e.thumb.strokes.len()))
    }
}

/// Computes `(columns, card_size, preview_height)` for a responsive card grid of width `avail_w`.
#[must_use]
pub fn card_grid_metrics(avail_w: f32) -> (usize, Vec2, f32) {
    let w = avail_w.max(CARD_MIN_WIDTH);
    let cols = ((w + CARD_GAP) / (CARD_MIN_WIDTH + CARD_GAP)).floor().max(1.0) as usize;
    let card_w = ((w - CARD_GAP * (cols.saturating_sub(1) as f32)) / cols as f32).clamp(136.0, 260.0);
    let preview_h = (card_w * 0.72).clamp(112.0, 168.0);
    (cols, vec2(card_w, preview_h + 42.0), preview_h)
}

/// Builds a [`CachedThumb`] from a page folder on disk via `nocturne_store::page`.
#[must_use]
pub fn build_thumb(page_dir: &Path, fallback_title: &str) -> CachedThumb {
    let (paper, raw_colour) = nocturne_store::page::appearance(page_dir).ok().unwrap_or((None, None));
    let page_colour = raw_colour.as_deref().and_then(NodeColor::parse);
    let (mut title, mut lines) = (fallback_title.to_string(), Vec::new());
    if let Ok(page) = nocturne_store::page::load_page(page_dir) {
        if !page.title.trim().is_empty() {
            title = page.title;
        }
        lines = extract_body_lines(&boxes::read(page.format, &page.body).0);
    }
    let (aspect, strokes) = nocturne_store::page::load_ink_drawing(page_dir).map_or((620.0 / 420.0, Vec::new()), |d| normalize_drawing(&d));
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
    drawing.strokes.iter().flat_map(|s| &s.dabs).filter(|d| d.x.is_finite() && d.y.is_finite()).for_each(|d| world.extend_with(pos2(d.x, d.y)));
    let (ww, wh) = (world.width().max(1.0), world.height().max(1.0));
    let norm = |x: f32, y: f32| [((x - world.min.x) / ww).clamp(0.0, 1.0), ((y - world.min.y) / wh).clamp(0.0, 1.0)];
    let mut strokes = Vec::new();
    for s in drawing.strokes.iter().take(MAX_THUMB_STROKES) {
        let finite: Vec<_> = s.dabs.iter().filter(|d| d.x.is_finite() && d.y.is_finite()).collect();
        if finite.is_empty() {
            continue;
        }
        let mut pts: Vec<[f32; 2]> = finite.iter().step_by((finite.len() / MAX_PTS_PER_STROKE).max(1)).map(|d| norm(d.x, d.y)).collect();
        if let Some(last) = finite.last().map(|d| norm(d.x, d.y))
            && pts.last() != Some(&last)
        {
            pts.push(last);
        }
        strokes.push(ThumbStroke { pts, colour: s.colour, width: s.width, highlighter: s.tool == Tool::Highlighter });
    }
    ((ww / wh).clamp(0.5, 2.4), strokes)
}

fn paint_dots_button(ui: &mut Ui, rect: Rect, id: Id, a11y: String, fg: Color32, fill: Option<Color32>, hover_bg: Color32) -> egui::Response {
    let resp = ui.interact(rect, id, Sense::click());
    resp.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &a11y));
    if let Some(bg) = if resp.hovered() { Some(hover_bg) } else { fill } {
        ui.painter().rect_filled(rect, CornerRadius::same(6), bg);
    }
    for dx in [-4.5_f32, 0.0, 4.5] {
        ui.painter().circle_filled(pos2(rect.center().x + dx, rect.center().y), 1.4, fg);
    }
    resp
}

pub(crate) fn node_menu_items(ui: &mut Ui, kind: NodeKind, path: &Path, title: &str, count: usize, actions: &mut Vec<LibraryAction>) {
    crate::ribbon::mark_popup_open(ui.ctx());
    ui.set_min_width(160.0);
    if kind == NodeKind::Section && ui.button(crate::tl_id!("ui.new_page")).clicked() {
        actions.push(LibraryAction::NewPage { section: path.to_path_buf() });
        ui.close();
    } else if matches!(kind, NodeKind::Notebook | NodeKind::SectionGroup) && ui.button(crate::tl_id!("ui.new_section_button")).clicked() {
        actions.push(LibraryAction::NewSection { parent: path.to_path_buf(), parent_kind: kind });
        ui.close();
    }
    if ui.button(crate::tl_id!("ui.rename_ellipsis")).clicked() {
        actions.push(LibraryAction::Rename { path: path.to_path_buf(), title: title.to_string() });
        ui.close();
    }
    if ui.button(crate::tl_id!("ui.delete_ellipsis")).clicked() {
        actions.push(LibraryAction::Delete { path: path.to_path_buf(), title: title.to_string(), count });
        ui.close();
    }
}

/// Paints a visual page thumbnail card, centered space-preserving title, and management menu; returns `true` when clicked to open.
pub fn draw_page_card(
    ui: &mut Ui,
    meta: (&Path, &str, usize),
    thumb: &CachedThumb,
    geom: (Vec2, f32),
    t: &Tokens,
    actions: &mut Vec<LibraryAction>,
) -> bool {
    let ((page_dir, title, count), (card_size, preview_h)) = (meta, geom);
    let (rect, resp) = ui.allocate_exact_size(card_size, Sense::click());
    let owned_title = title.to_string();
    resp.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &owned_title));

    let preview_rect = Rect::from_min_size(rect.min, vec2(card_size.x, preview_h));
    let preview_bg = thumb.page_colour.map_or(t.editor, |nc| {
        let (r, g, b) = nc.rgb(t.editor.r() < 128);
        mix(t.editor, Color32::from_rgb(r, g, b), 0.16)
    });
    ui.painter().rect_filled(preview_rect, CornerRadius::same(10), preview_bg);

    let clip = preview_rect.shrink(6.0);
    let painter = ui.painter_at(clip);
    paint_paper_rules(&painter, clip, thumb.paper.as_deref(), t);
    paint_thumb_lines(&painter, clip, thumb, t);
    paint_thumb_strokes(&painter, clip, thumb, luminance(preview_bg) < 0.35);

    let border = Stroke::new(if resp.hovered() { 1.6 } else { 1.0 }, if resp.hovered() { t.accent } else { t.container_border });
    ui.painter().rect_stroke(preview_rect, CornerRadius::same(10), border, StrokeKind::Inside);

    let dots_rect = Rect::from_min_size(pos2(preview_rect.max.x - 28.0, preview_rect.min.y + 4.0), vec2(24.0, 22.0));
    let label = format!("{title} — {}", crate::tl_id!("ui.more"));
    let dots =
        paint_dots_button(ui, dots_rect, Id::unique(("library-card-menu", page_dir)), label, t.text, Some(t.panel.gamma_multiply(0.85)), t.row_hover);
    egui::Popup::menu(&dots).show(|ui| node_menu_items(ui, NodeKind::Page, page_dir, title, count, actions));
    resp.context_menu(|ui| node_menu_items(ui, NodeKind::Page, page_dir, title, count, actions));

    let title_area = Rect::from_min_max(pos2(rect.min.x + 4.0, preview_rect.max.y + 6.0), pos2(rect.max.x - 4.0, rect.max.y));
    let job = egui::text::LayoutJob::simple(title.to_string(), FontId::proportional(13.0), t.text, title_area.width().max(24.0));
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    ui.painter_at(title_area).galley(pos2((rect.center().x - galley.size().x * 0.5).max(title_area.min.x), title_area.min.y), galley, t.text);
    resp.clicked() && !dots.clicked()
}

/// Paints a rounded left-sidebar row with optional disclosure chevron, icon, title, count, and node menu.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sidebar_pill(
    ui: &mut Ui,
    row: (&Path, &str, usize),
    visual: (Icon, Color32),
    layout: (bool, usize, Option<bool>),
    menu: Option<(NodeKind, usize)>,
    t: &Tokens,
    actions: &mut Vec<LibraryAction>,
) -> (bool, bool) {
    let ((key, title, count), (active, depth, disclosure)) = (row, layout);
    let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width().max(120.0), 34.0), Sense::hover());
    let pill = Rect::from_min_max(pos2(outer.min.x + (depth as f32 * 14.0).min(42.0), outer.min.y), outer.max);
    let (disc_w, show_dots) = (if disclosure.is_some() { 20.0 } else { 0.0 }, active && menu.is_some());
    let dots_w = if show_dots { 26.0 } else { 0.0 };
    let disc_rect = Rect::from_min_max(pill.min, pos2(pill.min.x + disc_w, pill.max.y));
    let main_rect = Rect::from_min_max(pos2(pill.min.x + disc_w, pill.min.y), pos2(pill.max.x - dots_w, pill.max.y));

    let main_resp = ui.interact(main_rect, Id::unique(("library-pill", key)), Sense::click());
    let a11y = format!("{title} ({count})");
    main_resp.widget_info(move || egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, &a11y));
    let disc_clicked = disclosure.is_some() && ui.interact(disc_rect, Id::unique(("library-disc", key)), Sense::click()).clicked();

    if let Some(bg) = active.then_some(t.band).or_else(|| main_resp.hovered().then_some(t.row_hover)) {
        ui.painter().rect_filled(pill, CornerRadius::same(10), bg);
    }
    let (fg, count_fg, ico_fg) = if active { (t.band_text, t.band_text, t.band_text) } else { (t.text, t.muted, visual.1) };
    if let Some(expanded) = disclosure {
        let c = disc_rect.center();
        let pts = if expanded {
            vec![pos2(c.x - 4.0, c.y - 2.0), pos2(c.x, c.y + 2.5), pos2(c.x + 4.0, c.y - 2.0)]
        } else {
            vec![pos2(c.x - 2.0, c.y - 4.0), pos2(c.x + 2.5, c.y), pos2(c.x - 2.0, c.y + 4.0)]
        };
        ui.painter().add(Shape::line(pts, Stroke::new(1.5, count_fg)));
    }
    let ico_rect = Rect::from_center_size(pos2(main_rect.min.x + if disclosure.is_some() { 10.0 } else { 18.0 }, pill.center().y), vec2(16.0, 16.0));
    icons::paint(ui.painter(), ico_rect, visual.0, ico_fg);

    let count_g = ui.painter().layout_no_wrap(count.to_string(), FontId::proportional(12.0), count_fg);
    let count_x = pill.max.x - dots_w - 10.0 - count_g.size().x;
    ui.painter().galley(pos2(count_x, pill.center().y - count_g.size().y * 0.5), count_g, count_fg);

    let text_left = ico_rect.max.x + 8.0;
    let text_clip = Rect::from_min_max(pos2(text_left, pill.min.y), pos2((count_x - 6.0).max(text_left + 12.0), pill.max.y));
    let title_g = ui.painter().layout_no_wrap(title.to_string(), FontId::proportional(13.5), fg);
    ui.painter_at(text_clip).galley(pos2(text_left, pill.center().y - title_g.size().y * 0.5), title_g, fg);

    if let Some((kind, node_count)) = menu {
        main_resp.context_menu(|ui| node_menu_items(ui, kind, key, title, node_count, actions));
        if show_dots {
            let dots_rect = Rect::from_center_size(pos2(pill.max.x - 14.0, pill.center().y), vec2(22.0, 22.0));
            let label = format!("{title} — {}", crate::tl_id!("ui.more"));
            let dots = paint_dots_button(ui, dots_rect, Id::unique(("library-node-menu", key)), label, fg, None, t.row_hover);
            egui::Popup::menu(&dots).show(|ui| node_menu_items(ui, kind, key, title, node_count, actions));
        }
    }
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
    node.children.iter().for_each(|c| collect_pages(c, out));
}
pub(crate) fn resolve_recents(root: &Path, tree: &[Node], recents: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let all = all_open_pages(tree);
    let mut out = Vec::new();
    for raw in recents {
        let cand = if raw.is_relative() { root.join(raw) } else { raw.clone() };
        if let Some(hit) = all.iter().find(|(p, _)| *p == cand)
            && !out.iter().any(|(p, _)| p == &hit.0)
        {
            out.push(hit.clone());
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
        if let Some(found) = all.iter().find(|(p, _)| p == hit) {
            out.push(found.clone());
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
    let (dark, fallback) = (
        t.panel.r() < 128,
        match node.info.kind {
            NodeKind::Notebook => t.notebook,
            NodeKind::SectionGroup => t.group,
            NodeKind::Section | NodeKind::Page => t.section,
        },
    );
    let color = node.info.color.map_or(fallback, |nc| {
        let (r, g, b) = nc.rgb(dark);
        readable_on(Color32::from_rgb(r, g, b), t.panel, 3.0, dark)
    });
    (icon, color)
}
pub(crate) fn target_section_for_path(tree: &[Node], scope_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(n) = scope_path.and_then(|p| find_node(tree, p)) {
        if n.info.kind == NodeKind::Section {
            return Some(n.path.clone());
        }
        return first_node_of_kind(&n.children, NodeKind::Section).or_else(|| (n.info.kind != NodeKind::Page).then(|| n.path.clone()));
    }
    first_node_of_kind(tree, NodeKind::Section).or_else(|| first_node_of_kind(tree, NodeKind::Notebook))
}
pub(crate) fn resolve_section_parent_for_path(tree: &[Node], scope_path: Option<&Path>) -> Option<(PathBuf, NodeKind)> {
    scope_path
        .and_then(|path| {
            std::iter::successors(Some(path), |p| p.parent()).find_map(|c| {
                find_node(tree, c)
                    .filter(|n| matches!(n.info.kind, NodeKind::Notebook | NodeKind::SectionGroup))
                    .map(|n| (n.path.clone(), n.info.kind))
            })
        })
        .or_else(|| tree.first().map(|nb| (nb.path.clone(), nb.info.kind)))
}
fn all_open_pages(tree: &[Node]) -> Vec<(PathBuf, String)> {
    let mut pages = Vec::new();
    tree.iter().for_each(|nb| collect_pages(nb, &mut pages));
    pages
}
fn paint_paper_rules(painter: &egui::Painter, clip: Rect, paper: Option<&str>, t: &Tokens) {
    let Some(paper) = paper.filter(|p| *p != "none") else { return };
    let (mut y, mut x, stroke) = (clip.min.y + 16.0, clip.min.x + 16.0, Stroke::new(0.7, t.rule_line));
    while y < clip.max.y {
        painter.line_segment([pos2(clip.min.x, y), pos2(clip.max.x, y)], stroke);
        y += 16.0;
    }
    if paper.starts_with("grid") || paper == "craft-grid" {
        while x < clip.max.x {
            painter.line_segment([pos2(x, clip.min.y), pos2(x, clip.max.y)], stroke);
            x += 16.0;
        }
    }
}
fn paint_thumb_lines(painter: &egui::Painter, clip: Rect, thumb: &CachedThumb, t: &Tokens) {
    let (mut y, body_col) = (clip.min.y + 4.0, if thumb.strokes.is_empty() { t.text } else { t.muted });
    for line in thumb.lines.iter().take(6) {
        if y + 11.0 > clip.max.y {
            break;
        }
        let col = if line.heading { t.text } else { body_col };
        let g = painter.layout_no_wrap(line.text.clone(), FontId::proportional(if line.heading { 10.5 } else { 9.2 }), col);
        painter.galley(pos2(clip.min.x + 4.0, y), g, col);
        y += 13.0;
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
        let col = Color32::from_rgb(r, g, b).gamma_multiply(if stroke.highlighter { 0.42 } else { 1.0 });
        let w = (stroke.width * (canvas.width() / 620.0)).clamp(1.1, if stroke.highlighter { 4.8 } else { 2.6 });
        let pts: Vec<Pos2> =
            stroke.pts.iter().map(|&[nx, ny]| pos2(canvas.min.x + nx * canvas.width(), canvas.min.y + ny * canvas.height())).collect();
        if let [only] = pts.as_slice() {
            painter.circle_filled(*only, w * 0.6, col);
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
    let spans = |s: &[Span]| {
        s.iter().filter(|sp| !sp.image.as_deref().is_some_and(|i| i.eq_ignore_ascii_case("ink.svg"))).map(|sp| sp.text.as_str()).collect::<String>()
    };
    match block {
        Block::Heading { spans: s, .. } => push(&spans(s), true, out),
        Block::Paragraph(s) | Block::Quote(s) | Block::Item { spans: s, .. } => push(&spans(s), false, out),
        Block::Code { text, .. } | Block::MathBlock(text) => push(text, false, out),
        Block::Table { head, rows } => {
            push(&head.iter().map(|c| spans(c)).collect::<Vec<_>>().join(" | "), true, out);
            rows.iter().take(3).for_each(|r| push(&r.iter().map(|c| spans(c)).collect::<Vec<_>>().join(" | "), false, out));
        }
        Block::Image { alt, src, .. } if !src.eq_ignore_ascii_case("ink.svg") => push(alt, false, out),
        Block::Image { .. } | Block::Rule => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_ink::{Dab, Stroke as InkStroke, svg::Flavour};
    use nocturne_store::{testing::TempDir, tree::NodeInfo};

    fn sample_node(path: &str, kind: NodeKind, title: &str, children: Vec<Node>) -> Node {
        Node {
            path: PathBuf::from(path),
            info: NodeInfo { kind, title: title.into(), color: None, format: None, id: String::new(), order: 0 },
            children,
        }
    }

    #[test]
    fn library_grid_and_thumbs_modules_stay_under_600_lines() {
        let (grid_lines, thumb_lines) = (include_str!("library_grid.rs").lines().count(), include_str!("library_thumbs.rs").lines().count());
        assert!((50..600).contains(&grid_lines), "library_grid.rs must stay under 600 lines (got {grid_lines})");
        assert!((50..600).contains(&thumb_lines), "library_thumbs.rs must stay under 600 lines (got {thumb_lines})");
    }

    #[test]
    fn page_count_and_card_grid_metrics() {
        let p1 = sample_node("/nb/atlas/lectures/p1", NodeKind::Page, "Wave Packet Derivation", Vec::new());
        let p2 = sample_node("/nb/atlas/lectures/p2", NodeKind::Page, "Spring Schedule Overview", Vec::new());
        let sec = sample_node("/nb/atlas/lectures", NodeKind::Section, "Lecture Notes", vec![p1, p2]);
        let tree = sample_node("/nb/atlas", NodeKind::Notebook, "Studio Atlas", vec![sec]);
        assert_eq!((page_count(&tree), page_count(&tree.children[0])), (2, 2));
        let ((lc, ls, _), (pc, ps, _)) = (card_grid_metrics(884.0), card_grid_metrics(480.0));
        assert!(lc >= 4 && (2..lc).contains(&pc) && ls.x >= 136.0 && ps.x >= 136.0);
    }

    #[test]
    fn build_thumb_and_cache_avoid_disk_thrashing_across_frames_and_bulk_invalidation() {
        let tmp = TempDir::new("library-thumbs-build");
        let page_dir = tmp.path().join("wave-packet-derivation");
        std::fs::create_dir_all(&page_dir).expect("create page dir");
        let md = "---\ntitle: Wave Packet Derivation\npage-color: blue\npaper: grid-medium\n---\n\n# Group Velocity\n\nDispersion relation notes\n\n![Ink](ink.svg)\n";
        std::fs::write(page_dir.join("index.md"), md).expect("write index.md");
        let mut drawing = Drawing::new(620.0, 420.0);
        for (tool, col, w, y) in [(Tool::Pen, Rgb(0x1a, 0x5f, 0xd0), 3.0, 80.0), (Tool::Highlighter, Rgb(0xf2, 0xc9, 0x4c), 14.0, 200.0)] {
            let mut s = InkStroke::new(tool, col, w);
            s.add(Dab::new(50.0, y, 0.7));
            s.add(Dab::new(280.0, y + 40.0, 0.8));
            drawing.add(s);
        }
        std::fs::write(page_dir.join("ink.svg"), nocturne_ink::svg::write(&drawing, Flavour::Editable)).expect("write ink.svg");
        let mut cache = ThumbCache::default();
        let thumb = cache.get_or_build(&page_dir, "Fallback Title").clone();
        assert_eq!(
            (thumb.title.as_str(), thumb.page_colour, thumb.paper.as_deref(), thumb.strokes.len()),
            ("Wave Packet Derivation", Some(NodeColor::Blue), Some("grid-medium"), 2)
        );
        assert_eq!(
            thumb.lines,
            vec![ThumbLine { text: "Group Velocity".into(), heading: true }, ThumbLine { text: "Dispersion relation notes".into(), heading: false }]
        );
        assert!(!thumb.strokes[0].highlighter && thumb.strokes[1].highlighter);
        assert!(thumb.strokes.iter().all(|st| st.pts.iter().all(|[x, y]| (0.0..=1.0).contains(x) && (0.0..=1.0).contains(y))));
        for _ in 0..5 {
            let _ = cache.get_or_build(&page_dir, "Fallback Title");
        }
        assert_eq!(cache.builds, 1, "steady-state frames must reuse cache without rebuilding");
        cache.invalidate_all();
        let _ = cache.get_or_build(&page_dir, "Fallback Title");
        assert_eq!(cache.builds, 1, "invalidate_all skips rebuild when on-disk stamp is unchanged");
        std::fs::write(page_dir.join("index.md"), format!("{md}\nUpdated line\n")).expect("update index.md");
        cache.invalidate_all();
        assert_eq!((cache.get_or_build(&page_dir, "Fallback Title").lines.len(), cache.builds), (3, 2));
    }
}
