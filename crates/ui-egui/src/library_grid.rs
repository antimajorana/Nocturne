//! Streamlined Tablet Studio two-pane library grid (`ui.library_view` = `studio_grid`).
//!
//! Left sidebar: `Recents`, open `Notebooks`, `SectionGroup`s, and `Sections` with rounded active
//! pills, `NodeKind::Page`-only counts, node management menus, and bottom `New notebook` /
//! `+ New section` actions. Narrow viewports (`< 600pt`) collapse the sidebar into a toggleable
//! overlay drawer. Right pane: scope header, inline title/body search, quick `Handwrite` / `Type`
//! / `Import` pills, and a responsive grid of cached miniature ink + text page thumbnail cards.

#[cfg(any(target_os = "android", target_os = "ios"))]
use crate::filepicker as rfd;
use crate::icons::{self, Icon};
use crate::library_thumbs::{
    CARD_GAP, ThumbCache, auto_expand_new_containers, card_grid_metrics, collect_pages, dedup_paths, draw_page_card, enclosing_container_path,
    find_node, first_node_of_kind, node_visuals, page_count, resolve_recents, resolve_search_cards, resolve_section_parent_for_path, rewrite_prefix,
    sidebar_pill, target_section_for_path,
};
pub use crate::library_thumbs::{CachedThumb, ImportKind, LibraryAction, ThumbLine, ThumbStroke};
use crate::ribbon::Act;
use crate::theme::{Tokens, mix};
use egui::{Align, CornerRadius, FontId, Id, Key, Layout, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use nocturne_engine::{LibraryView, Output, Params, Session};
use nocturne_model::NodeKind;
use nocturne_store::Settings;
use nocturne_store::tree::Node;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const LIBRARY_SEARCH_FIELD: &str = "library-grid-search-field";
pub const RECENT_PAGES_KEY: &str = "recent_pages";
pub const NARROW_BREAKPOINT: f32 = 600.0;
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

#[derive(Default)]
pub struct LibraryOut {
    pub open_page: Option<PathBuf>,
    pub new_page: Option<(Option<PathBuf>, NewPageMode)>,
    pub import: Option<(Option<PathBuf>, ImportKind)>,
    pub new_notebook: bool,
    pub new_section_in: Option<(PathBuf, NodeKind)>,
    pub actions: Vec<LibraryAction>,
    pub notice: Option<String>,
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
    search_cards: Vec<(PathBuf, String)>,
    thumbs: ThumbCache,
    drawer_open: bool,
}

impl LibraryGrid {
    #[must_use]
    pub fn load(settings: &Settings) -> Self {
        let mut recent_pages: Vec<PathBuf> = settings
            .get(RECENT_PAGES_KEY)
            .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| (!s.trim().is_empty()).then(|| PathBuf::from(s.trim())))
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
        self.thumbs.invalidate_page(page_dir);
        self.last_searched.clear();
        self.search_cards.clear();
    }
    pub fn invalidate_all(&mut self) {
        self.thumbs.invalidate_all();
        self.last_searched.clear();
        self.search_cards.clear();
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
        self.thumbs.drop_prefix(from);
        self.last_searched.clear();
        self.search_cards.clear();
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
        self.search_cards.clear();
    }
    #[must_use]
    pub fn scope(&self) -> Option<&LibraryScope> {
        self.scope.as_ref()
    }
    #[must_use]
    pub fn drawer_open(&self) -> bool {
        self.drawer_open
    }
    pub fn set_drawer_open(&mut self, open: bool) {
        self.drawer_open = open;
    }
    #[must_use]
    pub fn cached_counts(&self, page_dir: &Path) -> Option<(usize, usize)> {
        self.thumbs.cached_counts(page_dir)
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
        let (full, bg) = (ui.available_rect_before_wrap(), mix(t.editor, t.panel, 0.35));
        if full.width() >= NARROW_BREAKPOINT {
            self.drawer_open = false;
            let left_rect = Rect::from_min_max(full.min, pos2(full.min.x + SIDEBAR_WIDTH.min((full.width() * 0.36).max(200.0)), full.max.y));
            let right_rect = Rect::from_min_max(pos2(left_rect.max.x, full.min.y), full.max);
            ui.painter().rect_filled(left_rect, 0.0, t.panel);
            ui.painter().rect_filled(right_rect, 0.0, bg);
            ui.painter().line_segment([left_rect.right_top(), left_rect.right_bottom()], Stroke::new(1.0, t.container_border));
            self.show_sidebar(ui, session.tree(), &valid_recents, left_rect, t, (sync_busy, sync_configured, false), &mut out);
            self.show_right_pane(ui, session, &valid_recents, target_section, &scope_title, (right_rect, false), t, &mut out);
        } else {
            ui.painter().rect_filled(full, 0.0, bg);
            self.show_right_pane(ui, session, &valid_recents, target_section, &scope_title, (full, true), t, &mut out);
            if self.drawer_open {
                let ctx = ui.ctx();
                let no_overlay = !ctx.any_popup_open() && !egui::Popup::is_any_open(ctx) && ctx.memory(|m| m.top_modal_layer().is_none());
                let mut close = no_overlay && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
                egui::Area::new(Id::unique("studio-library-drawer")).order(egui::Order::Foreground).fixed_pos(full.min).show(ctx, |ui| {
                    ui.set_clip_rect(full);
                    let w = SIDEBAR_WIDTH.min((full.width() - 48.0).max(200.0)).min(full.width());
                    let drawer = Rect::from_min_max(full.min, pos2(full.min.x + w, full.max.y));
                    let scrim = Rect::from_min_max(pos2(drawer.max.x, full.min.y), full.max);
                    ui.painter().rect_filled(scrim, 0.0, t.scrim);
                    if scrim.width() > 0.0 && ui.interact(scrim, Id::unique("studio-library-drawer-scrim"), Sense::click_and_drag()).clicked() {
                        close = true;
                    }
                    ui.painter().rect_filled(drawer, 0.0, t.panel);
                    ui.painter().line_segment([drawer.right_top(), drawer.right_bottom()], Stroke::new(1.0, t.container_border));
                    self.show_sidebar(ui, session.tree(), &valid_recents, drawer, t, (sync_busy, sync_configured, true), &mut out);
                });
                self.drawer_open = !close && self.drawer_open;
            }
        }
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
        recents: &[(PathBuf, String)],
        rect: Rect,
        t: &Tokens,
        flags: (bool, bool, bool),
        out: &mut LibraryOut,
    ) {
        let (sync_busy, sync_configured, close_on_pick) = flags;
        let split_y = (rect.max.y - 52.0).max(rect.min.y);
        let top_rect = Rect::from_min_max(rect.min, pos2(rect.max.x, split_y)).shrink2(vec2(12.0, 12.0));
        let bottom_rect = Rect::from_min_max(pos2(rect.min.x, split_y), rect.max).shrink2(vec2(10.0, 8.0));
        ui.painter().line_segment([pos2(rect.min.x, split_y), pos2(rect.max.x, split_y)], Stroke::new(1.0, t.container_border));
        ui.scope_builder(egui::UiBuilder::new().id_salt("studio-library-sidebar").max_rect(top_rect), |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::tl_id!("ui.notebooks")).strong().size(18.0).color(t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.menu_button(RichText::new(crate::tl_id!("ui.more")).color(t.text), |ui| {
                        sidebar_more_menu(ui, sync_busy, sync_configured, out)
                    });
                });
            });
            ui.add_space(10.0);
            let recents_row = (Path::new("__recents__"), crate::tl_id!("ui.library_recents"), recents.len());
            let recents_layout = (matches!(self.scope, Some(LibraryScope::Recents)), 0, None);
            if sidebar_pill(ui, recents_row, (Icon::Clock, t.accent), recents_layout, None, t, &mut out.actions).0 {
                self.scope = Some(LibraryScope::Recents);
                if close_on_pick {
                    self.drawer_open = false;
                }
            }
            ui.add_space(6.0);
            egui::ScrollArea::vertical().id_salt("studio-library-tree-scroll").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                if tree.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new(crate::tl_id!("ui.no_notebooks_yet_use_new_notebook")).color(t.muted));
                } else {
                    tree.iter().for_each(|nb| self.render_tree_node(ui, nb, 0, close_on_pick, t, &mut out.actions));
                }
            });
        });
        ui.scope_builder(egui::UiBuilder::new().id_salt("studio-library-bottom").max_rect(bottom_rect), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let half = ((ui.available_width() - 6.0) * 0.5).max(72.0);
                let btn = |label| egui::Button::new(RichText::new(label).size(12.5).color(t.text)).corner_radius(8.0).min_size(vec2(half, 34.0));
                if ui.add(btn(crate::tl_id!("ui.new_notebook"))).clicked() {
                    out.new_notebook = true;
                }
                if ui.add(btn(crate::tl_id!("ui.new_section_button"))).clicked() {
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

    fn render_tree_node(&mut self, ui: &mut Ui, node: &Node, depth: usize, close_on_pick: bool, t: &Tokens, actions: &mut Vec<LibraryAction>) {
        if node.info.kind == NodeKind::Page {
            return;
        }
        let has_children = node.children.iter().any(|c| c.info.kind != NodeKind::Page);
        let is_expanded = self.expanded.contains(&node.path);
        let active = self.scope.as_ref() == Some(&LibraryScope::Node(node.path.clone()));
        let row = (node.path.as_path(), node.info.title.as_str(), page_count(node));
        let layout = (active, depth, has_children.then_some(is_expanded));
        let (clicked_row, clicked_disc) =
            sidebar_pill(ui, row, node_visuals(node, is_expanded, t), layout, Some((node.info.kind, node.count())), t, actions);
        if clicked_disc {
            if !self.expanded.remove(&node.path) {
                self.expanded.insert(node.path.clone());
            }
        } else if clicked_row {
            self.scope = Some(LibraryScope::Node(node.path.clone()));
            if has_children {
                self.expanded.insert(node.path.clone());
            }
            if close_on_pick {
                self.drawer_open = false;
            }
        }
        if has_children && self.expanded.contains(&node.path) {
            node.children.iter().for_each(|c| self.render_tree_node(ui, c, depth + 1, close_on_pick, t, actions));
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
        pane: (Rect, bool),
        t: &Tokens,
        out: &mut LibraryOut,
    ) {
        let (right_rect, narrow) = pane;
        let inner = right_rect.shrink2(vec2(if narrow { 14.0 } else { 20.0 }, 16.0));
        ui.scope_builder(egui::UiBuilder::new().id_salt("studio-library-right").max_rect(inner), |ui| {
            self.show_header_and_search(ui, scope_title, narrow, t);
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 8.0);
                if paint_action_pill(ui, Icon::Pen, crate::tl_id!("ui.library_handwrite"), t).clicked() {
                    out.new_page = Some((target_section.clone(), NewPageMode::Handwrite));
                }
                if paint_action_pill(ui, Icon::DirLtr, crate::tl_id!("ui.library_type"), t).clicked() {
                    out.new_page = Some((target_section.clone(), NewPageMode::Type));
                }
                import_pill(ui, &target_section, t, out);
            });
            ui.add_space(14.0);
            let trimmed_q = self.search_query.trim().to_string();
            let cards = if !trimmed_q.is_empty() {
                if trimmed_q != self.last_searched {
                    let p = Params::from([("q".to_string(), trimmed_q.clone()), ("limit".to_string(), "100".to_string())]);
                    let hits = match session.run("search.query", &p) {
                        Ok(Output::Hits(h)) => h.into_iter().map(|h| h.path).collect(),
                        _ => Vec::new(),
                    };
                    self.search_cards = resolve_search_cards(session.tree(), &hits);
                    self.last_searched = trimmed_q.clone();
                }
                self.search_cards.clone()
            } else if matches!(self.scope, Some(LibraryScope::Recents)) {
                valid_recents.to_vec()
            } else {
                self.scope_node_path().and_then(|p| find_node(session.tree(), p)).map_or_else(Vec::new, |n| {
                    let mut pages = Vec::new();
                    collect_pages(n, &mut pages);
                    pages
                })
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
                            let count = find_node(session.tree(), page_dir).map_or(1, Node::count);
                            let thumb = self.thumbs.get_or_build(page_dir, page_title);
                            if draw_page_card(ui, (page_dir, page_title, count), thumb, (card_size, preview_h), t, &mut out.actions) {
                                out.open_page = Some(page_dir.clone());
                            }
                        }
                    });
                    ui.add_space(CARD_GAP);
                }
            });
        });
    }

    fn show_header_and_search(&mut self, ui: &mut Ui, scope_title: &str, narrow: bool, t: &Tokens) {
        let mut cleared = false;
        let mut draw_search = |ui: &mut Ui, query: &mut String, full_w: bool| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if !query.is_empty() && ui.button(crate::tl_id!("ui.library_clear_search")).clicked() {
                cleared = true;
            }
            let w = if full_w { (ui.available_width() - 26.0).max(120.0) } else { ui.available_width().clamp(150.0, 270.0) };
            let edit = egui::TextEdit::singleline(query).id(Id::unique(LIBRARY_SEARCH_FIELD)).hint_text(crate::tl_id!("ui.search_all_pages"));
            ui.add(edit.desired_width(w));
            let (ico_rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
            icons::paint(ui.painter(), ico_rect, Icon::Search, t.muted);
        };
        ui.horizontal(|ui| {
            if narrow {
                let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
                let lbl = crate::tl_id!("ui.notebooks").to_string();
                resp.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &lbl));
                ui.painter().rect_filled(r, CornerRadius::same(8), if self.drawer_open || resp.hovered() { t.row_hover } else { t.panel });
                ui.painter().rect_stroke(r, CornerRadius::same(8), Stroke::new(1.0, t.container_border), StrokeKind::Inside);
                icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(16.0, 16.0)), Icon::Bars, t.text);
                if resp.clicked() {
                    self.drawer_open = !self.drawer_open;
                }
            }
            let title_w = if narrow { ui.available_width().max(120.0) } else { (ui.available_width() * 0.44).max(120.0) };
            let (tr, _) = ui.allocate_exact_size(vec2(title_w, 32.0), Sense::hover());
            let g = ui.painter().layout_no_wrap(scope_title.to_string(), FontId::proportional(20.0), t.text);
            ui.painter_at(tr).galley(pos2(tr.min.x, tr.center().y - g.size().y * 0.5), g, t.text);
            if !narrow {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| draw_search(ui, &mut self.search_query, false));
            }
        });
        if narrow {
            ui.add_space(6.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| draw_search(ui, &mut self.search_query, true));
        }
        if cleared {
            self.clear_search();
        }
    }
}

fn sidebar_more_menu(ui: &mut Ui, sync_busy: bool, sync_configured: bool, out: &mut LibraryOut) {
    crate::ribbon::mark_popup_open(ui.ctx());
    ui.set_min_width(210.0);
    let mut act_btn = |ui: &mut Ui, label: &str, act: Act| {
        if ui.button(label).clicked() {
            out.ribbon_acts.push(act);
            ui.close();
        }
    };
    act_btn(ui, crate::tl_id!("ui.use_classic_tree"), Act::LibraryView(LibraryView::ClassicTree));
    ui.separator();
    if sync_configured {
        let lbl = if sync_busy { crate::tl_id!("ui.syncing_short") } else { crate::tl_id!("ui.kdrive_sync_now_btn") };
        if ui.add_enabled(!sync_busy, egui::Button::new(lbl)).clicked() {
            out.sync_now = true;
            ui.close();
        }
    }
    act_btn(ui, crate::tl_id!("ui.kdrive_sync_open_btn"), Act::OpenCloudSync);
    act_btn(ui, crate::tl_id!("ui.theme"), Act::OpenThemes);
    act_btn(ui, crate::tl_id!("ui.settings"), Act::Options);
    act_btn(ui, crate::tl_id!("ui.file"), Act::File);
    ui.separator();
    act_btn(ui, crate::tl_id!("ui.switch_to_classic_ribbon"), Act::ToggleStudioLayout);
}

/// Opens the native file picker for a library import without creating any page first.
#[must_use]
pub fn pick_import_file(kind: ImportKind) -> Option<PathBuf> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = kind;
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    match kind {
        ImportKind::Document => rfd::FileDialog::new().set_title(crate::tl_id!("ui.choose_a_pdf_or_office_document_to_print_out")).add_filter(
            crate::tl_id!("ui.pdf_and_office_documents"),
            &["pdf", "doc", "docx", "ppt", "pptx", "xls", "xlsx", "odt", "odp", "ods", "rtf"],
        ),
        ImportKind::Picture => rfd::FileDialog::new()
            .set_title(crate::tl_id!("ui.choose_a_picture"))
            .add_filter(crate::tl_id!("ui.pictures"), &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "jxl"]),
    }
    .pick_file()
}
fn trigger_import(ctx: &egui::Context, target_section: &Option<PathBuf>, kind: ImportKind, out: &mut LibraryOut) {
    let Some(section) = target_section.as_ref() else {
        out.new_notebook = true;
        return;
    };
    if !cfg!(target_os = "ios") && ctx.os() == egui::os::OperatingSystem::IOS {
        out.notice = Some(crate::tl_id!("ui.ios_no_file_picker").to_string());
    } else if let Some(file) = pick_import_file(kind) {
        out.actions.push(LibraryAction::ImportFile { section: section.clone(), kind, file });
    }
}
fn import_pill(ui: &mut Ui, target_section: &Option<PathBuf>, t: &Tokens, out: &mut LibraryOut) {
    let resp = paint_action_pill(ui, Icon::Picture, crate::tl_id!("ui.library_import"), t);
    egui::Popup::menu(&resp).show(|ui| {
        crate::ribbon::mark_popup_open(ui.ctx());
        ui.set_min_width(170.0);
        for (lbl, kind) in
            [(crate::tl_id!("ui.library_import_document"), ImportKind::Document), (crate::tl_id!("ui.library_import_picture"), ImportKind::Picture)]
        {
            if ui.button(lbl).clicked() {
                trigger_import(ui.ctx(), target_section, kind, out);
                ui.close();
            }
        }
    });
}
// i18n-ui-text: label
fn paint_action_pill(ui: &mut Ui, icon: Icon, label: &str, t: &Tokens) -> egui::Response {
    let g = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(13.5), t.text);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 42.0, 34.0), Sense::click());
    let owned = label.to_string();
    resp.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &owned));
    ui.painter().rect_filled(r, CornerRadius::same(16), if resp.hovered() { t.row_hover } else { t.panel });
    ui.painter().rect_stroke(
        r,
        CornerRadius::same(16),
        Stroke::new(1.0, if resp.hovered() { t.accent } else { t.container_border }),
        StrokeKind::Inside,
    );
    let ico = Rect::from_center_size(pos2(r.min.x + 17.0, r.center().y), vec2(16.0, 16.0));
    icons::paint(ui.painter(), ico, icon, t.accent);
    ui.painter().galley(pos2(ico.max.x + 7.0, r.center().y - g.size().y * 0.5), g, t.text);
    resp
}
fn default_scope(tree: &[Node]) -> LibraryScope {
    first_node_of_kind(tree, NodeKind::Section)
        .or_else(|| first_node_of_kind(tree, NodeKind::Notebook))
        .map_or(LibraryScope::Recents, LibraryScope::Node)
}
fn current_scope_title(tree: &[Node], scope: Option<&LibraryScope>) -> String {
    match scope {
        Some(LibraryScope::Recents) => crate::tl_id!("ui.library_recents").to_string(),
        Some(LibraryScope::Node(p)) => find_node(tree, p).map_or_else(|| crate::tl_id!("ui.notebooks").to_string(), |n| n.info.title.clone()),
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
        assert_eq!(LibraryGrid::load(&settings).recent_pages, vec![PathBuf::from("/nb/atlas/archive/p1")]);
    }

    #[test]
    fn active_pill_and_secondary_text_tokens_pass_contrast_in_all_fourteen_themes() {
        for theme in Theme::all() {
            let t = theme.tokens();
            assert!(contrast(t.band_text, t.band) >= 4.5 && contrast(t.muted, t.panel) >= 4.5);
            assert!(contrast(t.text, mix(t.editor, t.panel, 0.35)) >= 4.5, "{} contrast < 4.5", theme.id());
        }
    }

    #[test]
    fn ios_import_and_narrow_drawer_behave_safely() {
        let ctx = egui::Context::default();
        ctx.set_os(egui::os::OperatingSystem::IOS);
        let mut out = LibraryOut::default();
        trigger_import(&ctx, &Some(PathBuf::from("/nb/sec")), ImportKind::Document, &mut out);
        assert!(out.import.is_none() && out.new_page.is_none() && out.actions.is_empty() && out.notice.is_some());

        let tmp = TempDir::new("library-grid-narrow");
        let mut session = Session::new(&tmp.path().join("nb"), Settings::at(tmp.path().join("cfg/settings.toml"))).expect("session");
        let (mut grid, t) = (LibraryGrid::default(), Theme::DEFAULT.tokens());
        grid.set_drawer_open(true);
        for (w, want_open) in [(460.0, true), (800.0, false)] {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(w, 700.0))), ..Default::default() };
            let mut r = ctx.run_ui(input, |ui| {
                grid.show(ui, &mut session, &t, false, false);
            });
            r.textures_delta.clear();
            assert_eq!(grid.drawer_open(), want_open, "width={w}");
        }
    }
}
