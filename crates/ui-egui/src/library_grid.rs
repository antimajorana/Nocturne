//! Streamlined Tablet Studio two-pane library grid (`ui.library_view` = `studio_grid`).
//!
//! Left sidebar: `Recents`, real open `Notebooks`, `SectionGroup`s, and `Sections` with rounded
//! active pills and `NodeKind::Page`-only counts, plus bottom `New notebook` / `+ New section`
//! actions and a `More...` menu. Right pane: scope header, inline title/body search, quick
//! `Handwrite` / `Type` / `Import` pills, and a responsive multi-column grid of cached miniature
//! ink + text page thumbnail cards preserving full multi-word titles.

use crate::icons::{self, Icon};
use crate::library_thumbs::{
    CARD_GAP, auto_expand_new_containers, build_thumb, card_grid_metrics, collect_pages, dedup_paths, draw_page_card, enclosing_container_path,
    find_node, first_node_of_kind, node_visuals, page_count, resolve_recents, resolve_search_cards, resolve_section_parent_for_path, rewrite_prefix,
    sidebar_pill, target_section_for_path,
};
pub use crate::library_thumbs::{CachedThumb, ThumbLine, ThumbStroke};
use crate::ribbon::Act;
use crate::theme::{Tokens, mix};
use egui::{Align, CornerRadius, FontId, Id, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use nocturne_engine::{LibraryView, Output, Params, Session};
use nocturne_model::NodeKind;
use nocturne_store::Settings;
use nocturne_store::tree::Node;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const LIBRARY_SEARCH_FIELD: &str = "library-grid-search-field";
pub const RECENT_PAGES_KEY: &str = "recent_pages";

const MAX_RECENT_PAGES: usize = 24;
const SIDEBAR_WIDTH: f32 = 256.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryScope {
    Recents,
    Node(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewPageMode {
    Handwrite,
    Type,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportKind {
    Document,
    Picture,
}

#[derive(Default)]
pub struct LibraryOut {
    pub open_page: Option<PathBuf>,
    pub new_page: Option<(Option<PathBuf>, NewPageMode)>,
    pub import: Option<(Option<PathBuf>, ImportKind)>,
    pub new_notebook: bool,
    pub new_section_in: Option<(PathBuf, NodeKind)>,
    pub sync_now: bool,
    pub ribbon_acts: Vec<Act>,
}

#[derive(Default)]
pub struct LibraryGrid {
    scope: Option<LibraryScope>,
    expanded: HashSet<PathBuf>,
    seen_containers: HashSet<PathBuf>,
    recent_pages: Vec<PathBuf>,
    search_query: String,
    last_searched: String,
    search_hits: Vec<PathBuf>,
    thumbs: HashMap<PathBuf, CachedThumb>,
}

impl LibraryGrid {
    #[must_use]
    pub fn load(settings: &Settings) -> Self {
        let mut recent_pages: Vec<PathBuf> = settings
            .get(RECENT_PAGES_KEY)
            .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| {
                let trimmed = s.trim();
                (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
            })
            .collect();
        dedup_paths(&mut recent_pages);
        recent_pages.truncate(MAX_RECENT_PAGES);
        if recent_pages.is_empty()
            && let Some(last) = settings.get("last_page").map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
        {
            recent_pages.push(PathBuf::from(last));
        }
        Self { recent_pages, ..Self::default() }
    }

    pub fn record_recent_page(&mut self, page_dir: &Path, settings: &Settings) {
        self.recent_pages.retain(|p| p != page_dir);
        self.recent_pages.insert(0, page_dir.to_path_buf());
        self.recent_pages.truncate(MAX_RECENT_PAGES);
        self.persist_recents(settings);
    }

    pub fn invalidate_page(&mut self, page_dir: &Path) {
        self.thumbs.remove(page_dir);
        self.last_searched.clear();
    }

    pub fn invalidate_all(&mut self) {
        self.thumbs.clear();
        self.last_searched.clear();
    }

    pub fn relocate(&mut self, from: &Path, to: &Path, settings: &Settings) {
        let mut changed = false;
        for entry in &mut self.recent_pages {
            if let Some(updated) = rewrite_prefix(entry, from, to) {
                *entry = updated;
                changed = true;
            }
        }
        if changed {
            dedup_paths(&mut self.recent_pages);
            self.persist_recents(settings);
        }
        if let Some(LibraryScope::Node(cur)) = &self.scope
            && let Some(updated) = rewrite_prefix(cur, from, to)
        {
            self.scope = Some(LibraryScope::Node(updated));
        }
        self.expanded = self.expanded.iter().map(|p| rewrite_prefix(p, from, to).unwrap_or_else(|| p.clone())).collect();
        self.seen_containers = self.seen_containers.iter().map(|p| rewrite_prefix(p, from, to).unwrap_or_else(|| p.clone())).collect();
        self.thumbs.retain(|k, _| !k.starts_with(from));
        self.last_searched.clear();
    }

    pub fn select_for_page(&mut self, page_dir: &Path) {
        if !matches!(self.scope, Some(LibraryScope::Recents))
            && let Some(parent) = page_dir.parent()
        {
            self.scope = Some(LibraryScope::Node(parent.to_path_buf()));
        }
    }

    #[must_use]
    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    pub fn clear_search(&mut self) {
        self.search_query.clear();
        self.last_searched.clear();
        self.search_hits.clear();
    }

    #[must_use]
    pub fn scope(&self) -> Option<&LibraryScope> {
        self.scope.as_ref()
    }

    #[must_use]
    pub fn cached_counts(&self, page_dir: &Path) -> Option<(usize, usize)> {
        self.thumbs.get(page_dir).map(|thumb| (thumb.lines.len(), thumb.strokes.len()))
    }

    fn persist_recents(&self, settings: &Settings) {
        let list: Vec<String> = self.recent_pages.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        if let Ok(serialized) = serde_json::to_string(&list) {
            let _ = settings.set(RECENT_PAGES_KEY, &serialized);
        }
    }

    pub fn show(&mut self, ui: &mut Ui, session: &mut Session, t: &Tokens, sync_busy: bool, sync_configured: bool) -> LibraryOut {
        let mut out = LibraryOut::default();
        self.sync_tree_state(session.tree());
        let valid_recents = resolve_recents(session.root(), session.tree(), &self.recent_pages);
        let target_section = target_section_for_path(session.tree(), self.scope_node_path());
        let scope_title = current_scope_title(session.tree(), self.scope.as_ref());

        let full = ui.available_rect_before_wrap();
        let sidebar_w = SIDEBAR_WIDTH.min((full.width() * 0.36).max(200.0));
        let left_rect = Rect::from_min_max(full.min, pos2(full.min.x + sidebar_w, full.max.y));
        let right_rect = Rect::from_min_max(pos2(left_rect.max.x, full.min.y), full.max);

        ui.painter().rect_filled(left_rect, 0.0, t.panel);
        ui.painter().rect_filled(right_rect, 0.0, mix(t.editor, t.panel, 0.35));
        ui.painter()
            .line_segment([pos2(left_rect.max.x, left_rect.min.y), pos2(left_rect.max.x, left_rect.max.y)], Stroke::new(1.0, t.container_border));

        self.show_sidebar(ui, session.tree(), &valid_recents, left_rect, t, sync_busy, sync_configured, &mut out);
        self.show_right_pane(ui, session, &valid_recents, target_section, &scope_title, right_rect, t, &mut out);
        out
    }

    fn sync_tree_state(&mut self, tree: &[Node]) {
        auto_expand_new_containers(tree, &mut self.seen_containers, &mut self.expanded);
        let next = match &self.scope {
            Some(LibraryScope::Recents) => Some(LibraryScope::Recents),
            Some(LibraryScope::Node(path)) => enclosing_container_path(tree, path).map(LibraryScope::Node),
            None => None,
        };
        self.scope = Some(next.unwrap_or_else(|| default_scope(tree)));
    }

    #[allow(clippy::too_many_arguments)]
    fn show_sidebar(
        &mut self,
        ui: &mut Ui,
        tree: &[Node],
        valid_recents: &[(PathBuf, String)],
        left_rect: Rect,
        t: &Tokens,
        sync_busy: bool,
        sync_configured: bool,
        out: &mut LibraryOut,
    ) {
        let split_y = (left_rect.max.y - 52.0).max(left_rect.min.y);
        let top_rect = Rect::from_min_max(left_rect.min, pos2(left_rect.max.x, split_y)).shrink2(vec2(12.0, 12.0));
        let bottom_rect = Rect::from_min_max(pos2(left_rect.min.x, split_y), left_rect.max).shrink2(vec2(10.0, 8.0));
        ui.painter().line_segment([pos2(left_rect.min.x, split_y), pos2(left_rect.max.x, split_y)], Stroke::new(1.0, t.container_border));

        ui.scope_builder(egui::UiBuilder::new().id_salt("studio-library-sidebar").max_rect(top_rect), |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::tl_id!("ui.notebooks")).strong().size(18.0).color(t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.menu_button(RichText::new(crate::tl_id!("ui.more")).color(t.text), |ui| {
                        sidebar_more_menu(ui, sync_busy, sync_configured, out);
                    });
                });
            });
            ui.add_space(10.0);

            let recents_active = matches!(self.scope, Some(LibraryScope::Recents));
            let recents_key = Path::new("__recents__");
            if sidebar_pill(
                ui,
                recents_key,
                Icon::Clock,
                t.accent,
                crate::tl_id!("ui.library_recents"),
                valid_recents.len(),
                recents_active,
                0,
                None,
                t,
            )
            .0
            {
                self.scope = Some(LibraryScope::Recents);
            }
            ui.add_space(6.0);

            egui::ScrollArea::vertical().id_salt("studio-library-tree-scroll").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                if tree.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new(crate::tl_id!("ui.no_notebooks_yet_use_new_notebook")).color(t.muted));
                } else {
                    for nb in tree {
                        self.render_tree_node(ui, nb, 0, t);
                    }
                }
            });
        });

        ui.scope_builder(egui::UiBuilder::new().id_salt("studio-library-bottom").max_rect(bottom_rect), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let half = ((ui.available_width() - 6.0) * 0.5).max(72.0);
                let new_nb = egui::Button::new(RichText::new(crate::tl_id!("ui.new_notebook")).size(12.5).color(t.text))
                    .corner_radius(8.0)
                    .min_size(vec2(half, 34.0));
                if ui.add(new_nb).clicked() {
                    out.new_notebook = true;
                }
                let new_sec = egui::Button::new(RichText::new(crate::tl_id!("ui.new_section_button")).size(12.5).color(t.text))
                    .corner_radius(8.0)
                    .min_size(vec2(half, 34.0));
                if ui.add(new_sec).clicked() {
                    if let Some(parent) = resolve_section_parent_for_path(tree, self.scope_node_path()) {
                        out.new_section_in = Some(parent);
                    } else {
                        out.new_notebook = true;
                    }
                }
            });
        });
    }

    fn scope_node_path(&self) -> Option<&Path> {
        match &self.scope {
            Some(LibraryScope::Node(path)) => Some(path.as_path()),
            _ => None,
        }
    }

    fn render_tree_node(&mut self, ui: &mut Ui, node: &Node, depth: usize, t: &Tokens) {
        if node.info.kind == NodeKind::Page {
            return;
        }
        let has_children = node.children.iter().any(|c| c.info.kind != NodeKind::Page);
        let is_expanded = self.expanded.contains(&node.path);
        let active = self.scope.as_ref() == Some(&LibraryScope::Node(node.path.clone()));
        let (icon, icon_color) = node_visuals(node, is_expanded, t);
        let (clicked_row, clicked_disc) =
            sidebar_pill(ui, &node.path, icon, icon_color, &node.info.title, page_count(node), active, depth, has_children.then_some(is_expanded), t);
        if clicked_disc {
            if is_expanded {
                self.expanded.remove(&node.path);
            } else {
                self.expanded.insert(node.path.clone());
            }
        } else if clicked_row {
            self.scope = Some(LibraryScope::Node(node.path.clone()));
            if has_children {
                self.expanded.insert(node.path.clone());
            }
        }
        if has_children && self.expanded.contains(&node.path) {
            for child in &node.children {
                self.render_tree_node(ui, child, depth + 1, t);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn show_right_pane(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        valid_recents: &[(PathBuf, String)],
        target_section: Option<PathBuf>,
        scope_title: &str,
        right_rect: Rect,
        t: &Tokens,
        out: &mut LibraryOut,
    ) {
        let inner = right_rect.shrink2(vec2(20.0, 16.0));
        ui.scope_builder(egui::UiBuilder::new().id_salt("studio-library-right").max_rect(inner), |ui| {
            ui.horizontal(|ui| {
                let (title_rect, _) = ui.allocate_exact_size(vec2((ui.available_width() * 0.44).max(120.0), 32.0), Sense::hover());
                let galley = ui.painter().layout_no_wrap(scope_title.to_string(), FontId::proportional(20.0), t.text);
                ui.painter_at(title_rect).galley(pos2(title_rect.min.x, title_rect.center().y - galley.size().y * 0.5), galley, t.text);

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if !self.search_query.is_empty() && ui.button(crate::tl_id!("ui.library_clear_search")).clicked() {
                        self.clear_search();
                    }
                    let search_w = ui.available_width().clamp(150.0, 270.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .id(Id::unique(LIBRARY_SEARCH_FIELD))
                            .hint_text(crate::tl_id!("ui.search_all_pages"))
                            .desired_width(search_w),
                    );
                    let (ico_rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                    icons::paint(ui.painter(), ico_rect, Icon::Search, t.muted);
                });
            });
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if action_pill(ui, Icon::Pen, crate::tl_id!("ui.library_handwrite"), t) {
                    out.new_page = Some((target_section.clone(), NewPageMode::Handwrite));
                }
                if action_pill(ui, Icon::DirLtr, crate::tl_id!("ui.library_type"), t) {
                    out.new_page = Some((target_section.clone(), NewPageMode::Type));
                }
                import_pill(ui, &target_section, t, out);
            });
            ui.add_space(14.0);

            let trimmed_q = self.search_query.trim().to_string();
            let cards = if !trimmed_q.is_empty() {
                if trimmed_q != self.last_searched {
                    let mut p = Params::new();
                    p.insert("q".to_string(), trimmed_q.clone());
                    p.insert("limit".to_string(), "100".to_string());
                    self.search_hits = match session.run("search.query", &p) {
                        Ok(Output::Hits(hits)) => hits.into_iter().map(|h| h.path).collect(),
                        _ => Vec::new(),
                    };
                    self.last_searched = trimmed_q.clone();
                }
                resolve_search_cards(session.tree(), &self.search_hits)
            } else {
                match &self.scope {
                    Some(LibraryScope::Recents) => valid_recents.to_vec(),
                    Some(LibraryScope::Node(path)) => find_node(session.tree(), path)
                        .map(|n| {
                            let mut pages = Vec::new();
                            collect_pages(n, &mut pages);
                            pages
                        })
                        .unwrap_or_default(),
                    None => Vec::new(),
                }
            };

            egui::ScrollArea::vertical().id_salt("studio-library-cards-scroll").auto_shrink([false, false]).show(ui, |ui| {
                if cards.is_empty() {
                    ui.add_space(18.0);
                    let msg = if !trimmed_q.is_empty() {
                        crate::tl_id!("ui.no_pages_match")
                    } else if matches!(self.scope, Some(LibraryScope::Recents)) {
                        crate::tl_id!("ui.library_no_recent_pages")
                    } else if session.tree().is_empty() {
                        crate::tl_id!("ui.no_notebooks_yet_use_new_notebook")
                    } else {
                        crate::tl_id!("ui.no_pages_in_this_section_yet")
                    };
                    ui.label(RichText::new(msg).color(t.muted));
                    return;
                }

                let (cols, card_size, preview_h) = card_grid_metrics(ui.available_width());
                for row in cards.chunks(cols) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = CARD_GAP;
                        for (page_dir, page_title) in row {
                            if self.thumbs.get(page_dir).is_none_or(|c| c.title != *page_title) {
                                self.thumbs.insert(page_dir.clone(), build_thumb(page_dir, page_title));
                            }
                            if let Some(thumb) = self.thumbs.get(page_dir)
                                && draw_page_card(ui, page_title, thumb, card_size, preview_h, t)
                            {
                                out.open_page = Some(page_dir.clone());
                            }
                        }
                    });
                    ui.add_space(CARD_GAP);
                }
            });
        });
    }
}

fn sidebar_more_menu(ui: &mut Ui, sync_busy: bool, sync_configured: bool, out: &mut LibraryOut) {
    crate::ribbon::mark_popup_open(ui.ctx());
    ui.set_min_width(210.0);
    if ui.button(crate::tl_id!("ui.use_classic_tree")).clicked() {
        out.ribbon_acts.push(Act::LibraryView(LibraryView::ClassicTree));
        ui.close();
    }
    ui.separator();
    if sync_configured {
        let sync_lbl = if sync_busy { crate::tl_id!("ui.syncing_short") } else { crate::tl_id!("ui.kdrive_sync_now_btn") };
        if ui.add_enabled(!sync_busy, egui::Button::new(sync_lbl)).clicked() {
            out.sync_now = true;
            ui.close();
        }
    }
    if ui.button(crate::tl_id!("ui.kdrive_sync_open_btn")).clicked() {
        out.ribbon_acts.push(Act::OpenCloudSync);
        ui.close();
    }
    if ui.button(crate::tl_id!("ui.theme")).clicked() {
        out.ribbon_acts.push(Act::OpenThemes);
        ui.close();
    }
    if ui.button(crate::tl_id!("ui.file")).clicked() {
        out.ribbon_acts.push(Act::File);
        ui.close();
    }
    ui.separator();
    if ui.button(crate::tl_id!("ui.switch_to_classic_ribbon")).clicked() {
        out.ribbon_acts.push(Act::ToggleStudioLayout);
        ui.close();
    }
}

fn import_pill(ui: &mut Ui, target_section: &Option<PathBuf>, t: &Tokens, out: &mut LibraryOut) {
    egui::Frame::NONE
        .fill(t.panel)
        .stroke(Stroke::new(1.0, t.container_border))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (ico_rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                icons::paint(ui.painter(), ico_rect, Icon::Picture, t.accent);
                ui.menu_button(RichText::new(crate::tl_id!("ui.library_import")).color(t.text), |ui| {
                    crate::ribbon::mark_popup_open(ui.ctx());
                    ui.set_min_width(170.0);
                    if ui.button(crate::tl_id!("ui.library_import_document")).clicked() {
                        out.import = Some((target_section.clone(), ImportKind::Document));
                        ui.close();
                    }
                    if ui.button(crate::tl_id!("ui.library_import_picture")).clicked() {
                        out.import = Some((target_section.clone(), ImportKind::Picture));
                        ui.close();
                    }
                });
            });
        });
}

// i18n-ui-text: label
fn action_pill(ui: &mut Ui, icon: Icon, label: &str, t: &Tokens) -> bool {
    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(13.5), t.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 42.0, 34.0), Sense::click());
    let owned = label.to_string();
    resp.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &owned));
    ui.painter().rect_filled(rect, CornerRadius::same(16), if resp.hovered() { t.row_hover } else { t.panel });
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(16),
        Stroke::new(1.0, if resp.hovered() { t.accent } else { t.container_border }),
        StrokeKind::Inside,
    );
    let ico_rect = Rect::from_center_size(pos2(rect.min.x + 17.0, rect.center().y), vec2(16.0, 16.0));
    icons::paint(ui.painter(), ico_rect, icon, t.accent);
    ui.painter().galley(pos2(ico_rect.max.x + 7.0, rect.center().y - galley.size().y * 0.5), galley, t.text);
    resp.clicked()
}

fn default_scope(tree: &[Node]) -> LibraryScope {
    first_node_of_kind(tree, NodeKind::Section)
        .or_else(|| first_node_of_kind(tree, NodeKind::Notebook))
        .map_or(LibraryScope::Recents, LibraryScope::Node)
}

fn current_scope_title(tree: &[Node], scope: Option<&LibraryScope>) -> String {
    match scope {
        Some(LibraryScope::Recents) => crate::tl_id!("ui.library_recents").to_string(),
        Some(LibraryScope::Node(path)) => find_node(tree, path).map_or_else(|| crate::tl_id!("ui.notebooks").to_string(), |n| n.info.title.clone()),
        None => crate::tl_id!("ui.notebooks").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Theme, contrast};
    use nocturne_store::testing::TempDir;

    #[test]
    fn relocate_rewrites_recents_and_scope_across_reload() {
        let tmp = TempDir::new("library-grid-recents");
        let settings = Settings::at(tmp.path().join("settings.toml"));
        let mut grid = LibraryGrid::load(&settings);
        grid.record_recent_page(Path::new("/nb/atlas/lectures/p1"), &settings);
        grid.select_for_page(Path::new("/nb/atlas/lectures/p1"));
        assert_eq!(grid.scope(), Some(&LibraryScope::Node(PathBuf::from("/nb/atlas/lectures"))));

        grid.relocate(Path::new("/nb/atlas/lectures"), Path::new("/nb/atlas/archive"), &settings);
        assert_eq!(grid.scope(), Some(&LibraryScope::Node(PathBuf::from("/nb/atlas/archive"))));
        let reloaded = LibraryGrid::load(&settings);
        assert_eq!(reloaded.recent_pages, vec![PathBuf::from("/nb/atlas/archive/p1")]);
    }

    #[test]
    fn active_pill_and_secondary_text_tokens_pass_contrast_in_all_fourteen_themes() {
        for theme in Theme::all() {
            let t = theme.tokens();
            assert!(contrast(t.band_text, t.band) >= 4.5, "{} active pill contrast < 4.5", theme.id());
            assert!(contrast(t.muted, t.panel) >= 4.5, "{} muted sidebar contrast < 4.5", theme.id());
            let right_bg = mix(t.editor, t.panel, 0.35);
            assert!(contrast(t.text, right_bg) >= 4.5, "{} right pane text contrast < 4.5", theme.id());
        }
    }
}
