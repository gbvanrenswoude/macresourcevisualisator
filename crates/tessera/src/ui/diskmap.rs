//! The disk map: a zoomable treemap of where storage goes.
//!
//! The first level groups the disk into categories (apps, projects, …) with
//! their biggest items nested inside. Clicking zooms one level deeper —
//! a category, an item, then plain folders all the way down — with a camera
//! animation from the clicked tile. Breadcrumbs, Esc or the mouse back
//! button zoom out again.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use egui::{
    Align2, Color32, CornerRadius, Id, Key, LayerId, Order, Painter, PointerButton, Rect, Sense,
    Stroke, StrokeKind, TextureId, Ui, UiBuilder, pos2, vec2,
};
use tessera_core::disk::{Category, DiskIndex, Kind, NodeId, Overview, ScanPriority, flags};
use tessera_core::format;
use tessera_core::metrics::Snapshot;
use tessera_core::treemap::{self, squarify};

use super::paint::{self, shade, with_alpha};
use super::theme::{self, Theme};
use crate::macos;
use crate::state::Shared;

#[derive(Clone, Debug, PartialEq)]
pub enum Level {
    Overview,
    Group(Category),
    Node {
        id: NodeId,
        cat: Category,
        path: PathBuf,
    },
}

#[derive(Clone, Debug)]
struct Entry {
    label: String,
    size: u64,
    color: Color32,
    detail: Option<String>,
    target: Option<Level>,
    path: Option<PathBuf>,
    display_path: Option<String>,
    files: u64,
    mtime: u32,
    reclaimable: u64,
    synthetic: bool,
    children: Vec<Entry>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Tile {
    entry: usize,
    child: Option<usize>,
    rect: Rect,
    framed: bool,
}

#[derive(Clone)]
struct Layout {
    level: Level,
    rect: Rect,
    generation: u64,
    title: String,
    total: u64,
    entries: Vec<Entry>,
    tiles: Vec<Tile>,
}

struct Transition {
    start: f64,
    /// Where the new layout's frame sits at t = 0 (it ends at the full rect).
    new_from: Rect,
    /// The previous layout and where its frame ends up at t = 1.
    old: Option<(Layout, Rect)>,
}

const DURATION: f32 = 0.34;
const LIST_WIDTH: f32 = 300.0;

#[derive(Default)]
pub struct DiskMap {
    stack: Vec<Level>,
    layout: Option<Layout>,
    transition: Option<Transition>,
    hovered: Option<(usize, Option<usize>)>,
    list_hovered: Option<usize>,
    context: Option<(usize, Option<usize>)>,
    focus: bool,
    generation: u64,
    fda: Option<(u64, bool)>,
    pending_crumb: Option<usize>,
}

impl DiskMap {
    pub fn focused(&self) -> bool {
        self.focus
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        shared: &Arc<Shared>,
        snap: &Snapshot,
        t: &Theme,
        gradient: TextureId,
    ) {
        if self.stack.is_empty() {
            self.stack.push(Level::Overview);
        }
        let ctx = ui.ctx().clone();
        let p = ui.painter_at(rect);
        let view = shared.disk.view();

        // Header ------------------------------------------------------------
        let x = rect.left() + 18.0;
        let head_y = rect.top() + 14.0;
        paint::section(&p, pos2(x, head_y + 3.0), "Storage", t);
        let crumbs_x = x + 74.0;
        self.breadcrumbs(
            ui,
            &p,
            pos2(crumbs_x, head_y),
            view.overview.as_deref(),
            view.index.as_deref(),
            t,
        );
        self.header_right(ui, &p, rect, shared, &view, t);

        let body = Rect::from_min_max(
            pos2(x, rect.top() + 48.0),
            pos2(rect.right() - 18.0, rect.bottom() - 18.0),
        );
        let (Some(index), Some(overview)) = (view.index.clone(), view.overview.clone()) else {
            self.empty_state(ui, &p, body, shared, &view, t);
            return;
        };

        if view.generation != self.generation {
            self.generation = view.generation;
            self.revalidate(&index);
            self.layout = None;
            debug_initial_level(&mut self.stack, &index, &overview);
        }

        let map_rect = Rect::from_min_max(
            body.min,
            pos2(body.right() - LIST_WIDTH - 16.0, body.bottom()),
        );
        let list_rect = Rect::from_min_max(pos2(map_rect.right() + 16.0, body.top()), body.max);

        let level = self.stack.last().cloned().unwrap_or(Level::Overview);
        let stale = self.layout.as_ref().is_none_or(|l| {
            l.level != level || l.rect != map_rect || l.generation != view.generation
        });
        if stale {
            self.context = None;
            self.layout = Some(build_layout(
                &level,
                map_rect,
                &index,
                &overview,
                view.generation,
                snap,
                t,
            ));
        }

        // Input -----------------------------------------------------------------
        let resp = ui.interact(map_rect, ui.scope_id().with("treemap"), Sense::click());
        let now = ctx.input(|i| i.time);
        let animating = self
            .transition
            .as_ref()
            .is_some_and(|tr| (now - tr.start) < DURATION as f64);
        if !animating {
            self.transition = None;
        }

        if let Some(i) = self.pending_crumb.take()
            && !animating
        {
            self.go_to(
                i,
                &index,
                &overview,
                map_rect,
                view.generation,
                snap,
                t,
                now,
            );
        }

        let hover_pos = resp.hover_pos();
        let layout = self.layout.as_ref().unwrap();
        self.hovered = if animating {
            None
        } else {
            hover_pos.and_then(|pos| hit(layout, pos))
        };
        if let Some(i) = self.list_hovered.take()
            && self.hovered.is_none()
        {
            self.hovered = Some((i, None));
        }

        if resp.clicked()
            && !animating
            && let Some(h) = hit(layout, resp.interact_pointer_pos().unwrap_or_default())
        {
            self.open(
                h,
                &index,
                &overview,
                map_rect,
                view.generation,
                snap,
                t,
                now,
            );
        }
        if resp.secondary_clicked() {
            self.context = hover_pos.and_then(|pos| hit(self.layout.as_ref().unwrap(), pos));
        }
        let back = ctx.input(|i| {
            i.key_pressed(Key::Escape) || i.pointer.button_clicked(PointerButton::Extra1)
        }) || (ctx.input(|i| i.key_pressed(Key::ArrowUp) && i.modifiers.command));
        if back && self.stack.len() > 1 && !animating {
            self.go_to(
                self.stack.len() - 2,
                &index,
                &overview,
                map_rect,
                view.generation,
                snap,
                t,
                now,
            );
        }

        // Paint ---------------------------------------------------------------
        let painter = ui.painter_at(map_rect.expand(2.0));
        if let Some(tr) = &self.transition {
            let k = ease(((now - tr.start) as f32 / DURATION).clamp(0.0, 1.0));
            if let Some((old, old_to)) = &tr.old {
                let cam = lerp_rect(old.rect, *old_to, k);
                let mut faded = painter.clone();
                faded.set_opacity(1.0 - k);
                draw_layout(&faded, old, cam, None, false, t, gradient);
            }
            let cam = lerp_rect(tr.new_from, map_rect, k);
            let mut fading_in = painter.clone();
            fading_in.set_opacity(k.max(0.05));
            draw_layout(
                &fading_in,
                self.layout.as_ref().unwrap(),
                cam,
                None,
                k > 0.8,
                t,
                gradient,
            );
            ctx.request_repaint();
        } else {
            draw_layout(
                &painter,
                self.layout.as_ref().unwrap(),
                map_rect,
                self.hovered,
                true,
                t,
                gradient,
            );
        }

        // Tooltip
        if let (Some(h), Some(pos)) = (self.hovered, hover_pos) {
            tooltip(&ctx, self.layout.as_ref().unwrap(), h, pos, t);
            ctx.set_cursor_icon(
                if entry_of(self.layout.as_ref().unwrap(), h).is_some_and(|e| e.target.is_some()) {
                    egui::CursorIcon::PointingHand
                } else {
                    egui::CursorIcon::Default
                },
            );
        }

        // Context menu
        let context = self.context;
        let mut action: Option<MenuAction> = None;
        resp.context_menu(|ui| {
            ui.set_min_width(190.0);
            let Some(h) = context else {
                if ui.button("Go up").clicked() {
                    action = Some(MenuAction::Up);
                }
                return;
            };
            let Some(e) = entry_of(self.layout.as_ref().unwrap(), h).cloned() else {
                ui.close();
                return;
            };
            ui.label(egui::RichText::new(&e.label).strong());
            ui.separator();
            if e.target.is_some() && ui.button("Open").clicked() {
                action = Some(MenuAction::Open(h));
            }
            if let Some(path) = &e.path {
                if ui.button("Reveal in Finder").clicked() {
                    action = Some(MenuAction::Reveal(path.clone()));
                }
                if ui.button("Open with Default App").clicked() {
                    action = Some(MenuAction::OpenPath(path.clone()));
                }
                if ui.button("Copy Path").clicked() {
                    ui.ctx().copy_text(
                        e.display_path
                            .clone()
                            .unwrap_or_else(|| path.display().to_string()),
                    );
                    ui.close();
                }
            }
            if self.stack.len() > 1 {
                ui.separator();
                if ui.button("Go Up").clicked() {
                    action = Some(MenuAction::Up);
                }
            }
        });
        match action {
            Some(MenuAction::Open(h)) => self.open(
                h,
                &index,
                &overview,
                map_rect,
                view.generation,
                snap,
                t,
                now,
            ),
            Some(MenuAction::Reveal(path)) => macos::reveal_in_finder(&path),
            Some(MenuAction::OpenPath(path)) => macos::open_path(&path),
            Some(MenuAction::Up) if self.stack.len() > 1 => self.go_to(
                self.stack.len() - 2,
                &index,
                &overview,
                map_rect,
                view.generation,
                snap,
                t,
                now,
            ),
            _ => {}
        }

        // Side list
        self.side_list(
            ui,
            list_rect,
            &index,
            &overview,
            map_rect,
            view.generation,
            snap,
            t,
            now,
        );
    }

    fn revalidate(&mut self, index: &DiskIndex) {
        // Node ids change between scans; re-resolve levels by path.
        let mut keep = Vec::with_capacity(self.stack.len());
        for level in self.stack.drain(..) {
            match level {
                Level::Node { cat, path, .. } => match index.find(&path) {
                    Some(id) => keep.push(Level::Node { id, cat, path }),
                    None => break,
                },
                other => keep.push(other),
            }
        }
        if keep.is_empty() {
            keep.push(Level::Overview);
        }
        self.stack = keep;
    }

    #[allow(clippy::too_many_arguments)]
    fn open(
        &mut self,
        h: (usize, Option<usize>),
        index: &DiskIndex,
        overview: &Overview,
        map_rect: Rect,
        generation: u64,
        snap: &Snapshot,
        t: &Theme,
        now: f64,
    ) {
        self.hovered = None;
        self.list_hovered = None;
        let Some(layout) = self.layout.take() else {
            return;
        };
        let Some(entry) = layout.entries.get(h.0) else {
            self.layout = Some(layout);
            return;
        };
        let mut push = Vec::new();
        let origin = match h.1 {
            Some(c) => {
                let Some(child) = entry.children.get(c) else {
                    self.layout = Some(layout);
                    return;
                };
                if child.target.is_none() {
                    self.layout = Some(layout);
                    return;
                }
                if let Some(t) = &entry.target {
                    push.push(t.clone());
                }
                push.push(child.target.clone().unwrap());
                tile_rect(&layout, h).unwrap_or(map_rect)
            }
            None => {
                let Some(target) = &entry.target else {
                    self.layout = Some(layout);
                    return;
                };
                push.push(target.clone());
                tile_rect(&layout, h).unwrap_or(map_rect)
            }
        };
        self.stack.extend(push);
        let level = self.stack.last().cloned().unwrap();
        let new = build_layout(&level, map_rect, index, overview, generation, snap, t);
        self.transition = Some(Transition {
            start: now,
            new_from: origin,
            old: Some((layout, expand_to(map_rect, origin))),
        });
        self.layout = Some(new);
        self.context = None;
    }

    #[allow(clippy::too_many_arguments)]
    fn go_to(
        &mut self,
        depth: usize,
        index: &DiskIndex,
        overview: &Overview,
        map_rect: Rect,
        generation: u64,
        snap: &Snapshot,
        t: &Theme,
        now: f64,
    ) {
        if depth + 1 >= self.stack.len() {
            return;
        }
        self.hovered = None;
        self.list_hovered = None;
        let child_level = self.stack[depth + 1].clone();
        self.stack.truncate(depth + 1);
        let level = self.stack.last().cloned().unwrap();
        let new = build_layout(&level, map_rect, index, overview, generation, snap, t);
        // Where does the level we are leaving sit in the parent layout?
        let origin = new
            .tiles
            .iter()
            .find(|tile| {
                let e = &new.entries[tile.entry];
                let target = match tile.child {
                    Some(c) => e.children[c].target.as_ref(),
                    None => e.target.as_ref(),
                };
                target.is_some_and(|tg| same_level(tg, &child_level))
            })
            .map(|tile| tile.rect)
            .unwrap_or_else(|| map_rect.shrink(map_rect.width().min(map_rect.height()) * 0.3));
        let old = self.layout.take().map(|l| (l, origin));
        self.transition = Some(Transition {
            start: now,
            new_from: expand_to(map_rect, origin),
            old,
        });
        self.layout = Some(new);
        self.context = None;
    }

    fn breadcrumbs(
        &mut self,
        ui: &mut Ui,
        p: &Painter,
        pos: egui::Pos2,
        overview: Option<&Overview>,
        index: Option<&DiskIndex>,
        t: &Theme,
    ) {
        let mut x = pos.x;
        let n = self.stack.len();
        let mut clicked = None;
        for (i, level) in self.stack.iter().enumerate() {
            let label = match level {
                Level::Overview => "Macintosh HD".to_string(),
                Level::Group(c) => c.label().to_string(),
                Level::Node { id, .. } => index
                    .filter(|ix| (*id as usize) < ix.len())
                    .map(|ix| ix.name(*id).trim_end_matches(".app").to_string())
                    .unwrap_or_default(),
            };
            let last = i + 1 == n;
            let galley = p.layout_no_wrap(
                label,
                theme::font(13.0, if last { theme::SEMIBOLD } else { theme::MEDIUM }),
                t.text,
            );
            let r =
                Rect::from_min_size(pos2(x - 6.0, pos.y - 3.0), galley.size() + vec2(12.0, 6.0));
            let resp = ui.interact(r, ui.scope_id().with(("crumb", i)), Sense::click());
            if !last && resp.hovered() {
                p.rect_filled(r, CornerRadius::same(7), t.track);
            }
            if !last && resp.clicked() {
                clicked = Some(i);
            }
            let color = if last { t.text } else { t.text2 };
            p.galley(pos2(x, pos.y), galley.clone(), color);
            x += galley.size().x + 6.0;
            if !last {
                p.text(
                    pos2(x + 2.0, pos.y + galley.size().y / 2.0),
                    Align2::LEFT_CENTER,
                    "›",
                    theme::font(14.0, theme::MEDIUM),
                    t.text3,
                );
                x += 14.0;
            }
        }
        let _ = overview;
        if let Some(i) = clicked {
            self.pending_crumb = Some(i);
        }
    }
}

enum MenuAction {
    Open((usize, Option<usize>)),
    Reveal(PathBuf),
    OpenPath(PathBuf),
    Up,
}

fn same_level(a: &Level, b: &Level) -> bool {
    match (a, b) {
        (Level::Node { path: pa, .. }, Level::Node { path: pb, .. }) => pa == pb,
        _ => a == b,
    }
}

fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn lerp_rect(a: Rect, b: Rect, k: f32) -> Rect {
    Rect::from_min_max(a.min + (b.min - a.min) * k, a.max + (b.max - a.max) * k)
}

/// The frame rect at which `inner` (a rect inside `full`) would fill `full`.
fn expand_to(full: Rect, inner: Rect) -> Rect {
    let sx = full.width() / inner.width().max(1.0);
    let sy = full.height() / inner.height().max(1.0);
    let min = pos2(
        full.left() - (inner.left() - full.left()) * sx,
        full.top() - (inner.top() - full.top()) * sy,
    );
    Rect::from_min_size(min, vec2(full.width() * sx, full.height() * sy))
}

fn map_rect_through(r: Rect, from: Rect, to: Rect) -> Rect {
    let sx = to.width() / from.width().max(1e-3);
    let sy = to.height() / from.height().max(1e-3);
    Rect::from_min_max(
        pos2(
            to.left() + (r.left() - from.left()) * sx,
            to.top() + (r.top() - from.top()) * sy,
        ),
        pos2(
            to.left() + (r.right() - from.left()) * sx,
            to.top() + (r.bottom() - from.top()) * sy,
        ),
    )
}

fn hit(layout: &Layout, pos: egui::Pos2) -> Option<(usize, Option<usize>)> {
    layout
        .tiles
        .iter()
        .rev()
        .find(|t| t.rect.contains(pos))
        .map(|t| (t.entry, t.child))
}

fn tile_rect(layout: &Layout, h: (usize, Option<usize>)) -> Option<Rect> {
    layout
        .tiles
        .iter()
        .find(|t| t.entry == h.0 && t.child == h.1)
        .map(|t| t.rect)
}

fn entry_of(layout: &Layout, h: (usize, Option<usize>)) -> Option<&Entry> {
    let e = layout.entries.get(h.0)?;
    match h.1 {
        Some(c) => e.children.get(c),
        None => Some(e),
    }
}

// ---------------------------------------------------------------------------
// Building entries for a level

const SHADES: [f32; 8] = [0.0, -0.12, 0.08, -0.2, 0.14, -0.06, -0.26, 0.04];

fn vary(base: Color32, i: usize) -> Color32 {
    shade(base, SHADES[i % SHADES.len()])
}

fn node_entry(
    index: &DiskIndex,
    id: NodeId,
    cat: Category,
    base: Color32,
    i: usize,
    t: &Theme,
    with_children: bool,
) -> Entry {
    let n = *index.node(id);
    let color = t.kind_color(n.kind, vary(base, i));
    let path = index.path(id);
    let detail = match n.kind {
        Kind::Project => Some(format!("{} project", n.lang().label())),
        Kind::Folder if n.has(flags::DENIED) => {
            Some("Not readable without Full Disk Access".into())
        }
        Kind::Folder if n.has(flags::DATALESS) => Some("Stored in the cloud".into()),
        Kind::Folder => None,
        Kind::Artifacts => Some("Build artifacts: safe to delete, rebuilt on demand".into()),
        Kind::Cache => Some("Cache: safe to delete".into()),
        k => Some(k.label().to_string()),
    };
    let reclaimable = if n.kind.is_reclaimable() {
        n.size
    } else {
        tessera_core::disk::overview::reclaimable_inside(index, id)
    };
    let target = (n.is_dir() && n.child_count > 0).then(|| Level::Node {
        id,
        cat,
        path: path.clone(),
    });
    let children = if with_children && n.is_dir() {
        node_children(index, id, cat, color, t, 48, false)
    } else {
        Vec::new()
    };
    Entry {
        label: index.name(id).to_string(),
        size: n.size,
        color,
        detail,
        target,
        display_path: Some(index.display_path(id)),
        path: Some(path),
        files: n.files as u64,
        mtime: n.mtime,
        reclaimable,
        synthetic: false,
        children,
    }
}

fn node_children(
    index: &DiskIndex,
    id: NodeId,
    cat: Category,
    base: Color32,
    t: &Theme,
    cap: usize,
    nested: bool,
) -> Vec<Entry> {
    let mut out: Vec<Entry> = index
        .children(id)
        .take(cap)
        .enumerate()
        .map(|(i, c)| node_entry(index, c, cat, base, i, t, nested))
        .collect();
    let shown: u64 = out.iter().map(|e| e.size).sum();
    let (rest, rest_files) = {
        let (bytes, files) = index.remainder(id);
        let hidden: u64 = index
            .children(id)
            .skip(cap)
            .map(|c| index.node(c).size)
            .sum();
        let hidden_files: u64 = index
            .children(id)
            .skip(cap)
            .map(|c| index.node(c).files as u64)
            .sum();
        (bytes + hidden, files + hidden_files)
    };
    let _ = shown;
    if rest > 0 {
        out.push(Entry {
            label: if rest_files > 0 {
                format!("{} smaller items", format::count(rest_files))
            } else {
                "Other files".into()
            },
            size: rest,
            color: with_alpha(shade(base, -0.35), 0.55),
            detail: Some("Files and folders too small to show individually".into()),
            target: None,
            path: None,
            display_path: None,
            files: rest_files,
            mtime: 0,
            reclaimable: 0,
            synthetic: true,
            children: Vec::new(),
        });
    }
    out
}

fn item_entries(
    group: &tessera_core::disk::Group,
    index: &DiskIndex,
    t: &Theme,
    cap: usize,
    nested: bool,
) -> Vec<Entry> {
    let base = t.category(group.category);
    let mut out: Vec<Entry> = group
        .items
        .iter()
        .take(cap)
        .enumerate()
        .map(|(i, item)| {
            let color = match item.node {
                Some(id) => t.kind_color(index.node(id).kind, vary(base, i)),
                None => vary(shade(base, -0.15), i),
            };
            let children = match (nested, item.node) {
                (true, Some(id)) => node_children(index, id, group.category, color, t, 40, false),
                _ => Vec::new(),
            };
            Entry {
                label: item.label.clone(),
                size: item.size,
                color,
                detail: item.detail.clone(),
                target: item
                    .node
                    .filter(|id| index.node(*id).child_count > 0)
                    .map(|id| Level::Node {
                        id,
                        cat: group.category,
                        path: index.path(id),
                    }),
                path: item.node.map(|id| index.path(id)),
                display_path: item.node.map(|id| index.display_path(id)),
                files: item.files,
                mtime: item.mtime,
                reclaimable: item.reclaimable,
                synthetic: item.node.is_none(),
                children,
            }
        })
        .collect();
    let rest: u64 = group.items.iter().skip(cap).map(|i| i.size).sum();
    let rest_n = group.items.len().saturating_sub(cap);
    if rest > 0 {
        out.push(Entry {
            label: format!("{rest_n} more"),
            size: rest,
            color: with_alpha(shade(base, -0.35), 0.6),
            detail: Some(format!(
                "{rest_n} smaller items in {}",
                group.category.label()
            )),
            target: None,
            path: None,
            display_path: None,
            files: 0,
            mtime: 0,
            reclaimable: 0,
            synthetic: true,
            children: Vec::new(),
        });
    }
    out
}

fn build_layout(
    level: &Level,
    rect: Rect,
    index: &DiskIndex,
    overview: &Overview,
    generation: u64,
    snap: &Snapshot,
    t: &Theme,
) -> Layout {
    let _ = snap;
    let (title, entries) = match level {
        Level::Overview => {
            let entries = overview
                .groups
                .iter()
                .map(|g| Entry {
                    label: g.category.label().to_string(),
                    size: g.size,
                    color: t.category(g.category),
                    detail: Some(g.category.blurb().to_string()),
                    target: Some(Level::Group(g.category)),
                    path: None,
                    display_path: None,
                    files: g.items.iter().map(|i| i.files).sum(),
                    mtime: 0,
                    reclaimable: g.items.iter().map(|i| i.reclaimable).sum(),
                    synthetic: false,
                    children: item_entries(g, index, t, 60, false),
                })
                .collect();
            ("Macintosh HD".to_string(), entries)
        }
        Level::Group(cat) => {
            let entries = overview
                .group(*cat)
                .map(|g| item_entries(g, index, t, 150, true))
                .unwrap_or_default();
            (cat.label().to_string(), entries)
        }
        Level::Node { id, cat, .. } => {
            let base = t.category(*cat);
            let color = t.kind_color(index.node(*id).kind, base);
            (
                index.name(*id).to_string(),
                node_children(index, *id, *cat, color, t, 200, true),
            )
        }
    };
    let total = entries.iter().map(|e| e.size).sum();
    let tiles = compute_tiles(&entries, rect);
    Layout {
        level: level.clone(),
        rect,
        generation,
        title,
        total,
        entries,
        tiles,
    }
}

fn to_t(r: Rect) -> treemap::Rect {
    treemap::Rect::new(r.left(), r.top(), r.width(), r.height())
}

fn from_t(r: treemap::Rect) -> Rect {
    Rect::from_min_size(pos2(r.x, r.y), vec2(r.w, r.h))
}

fn compute_tiles(entries: &[Entry], rect: Rect) -> Vec<Tile> {
    let weights: Vec<f64> = entries.iter().map(|e| e.size as f64).collect();
    let rects = squarify(&weights, to_t(rect));
    let mut tiles = Vec::new();
    for (i, r) in rects.into_iter().enumerate() {
        let r = from_t(r).shrink(1.5);
        if r.width() < 1.5 || r.height() < 1.5 {
            continue;
        }
        let e = &entries[i];
        if !e.children.is_empty() && r.width() >= 86.0 && r.height() >= 60.0 {
            tiles.push(Tile {
                entry: i,
                child: None,
                rect: r,
                framed: true,
            });
            let content = Rect::from_min_max(
                pos2(r.left() + 4.0, r.top() + 24.0),
                pos2(r.right() - 4.0, r.bottom() - 4.0),
            );
            let cw: Vec<f64> = e.children.iter().map(|c| c.size as f64).collect();
            for (j, cr) in squarify(&cw, to_t(content)).into_iter().enumerate() {
                let cr = from_t(cr).shrink(1.0);
                if cr.width() >= 2.0 && cr.height() >= 2.0 {
                    tiles.push(Tile {
                        entry: i,
                        child: Some(j),
                        rect: cr,
                        framed: false,
                    });
                }
            }
        } else {
            tiles.push(Tile {
                entry: i,
                child: None,
                rect: r,
                framed: false,
            });
        }
    }
    tiles
}

// ---------------------------------------------------------------------------
// Painting

fn draw_layout(
    p: &Painter,
    layout: &Layout,
    camera: Rect,
    hovered: Option<(usize, Option<usize>)>,
    text: bool,
    t: &Theme,
    gradient: TextureId,
) {
    for tile in &layout.tiles {
        let r = map_rect_through(tile.rect, layout.rect, camera);
        if !r.intersects(p.clip_rect()) {
            continue;
        }
        let Some(e) = entry_of(layout, (tile.entry, tile.child)) else {
            continue;
        };
        let is_hovered = hovered == Some((tile.entry, tile.child));
        let parent_hovered =
            hovered.is_some_and(|h| h.0 == tile.entry && h.1.is_none()) && tile.child.is_some();
        let radius = CornerRadius::from((r.width().min(r.height()) / 3.0).min(if tile.framed {
            9.0
        } else {
            6.0
        }));

        if tile.framed {
            let fill = with_alpha(e.color, if t.dark { 0.17 } else { 0.14 });
            p.rect_filled(r, radius, fill);
            p.rect_stroke(
                r,
                radius,
                Stroke::new(1.0, with_alpha(e.color, if is_hovered { 0.9 } else { 0.4 })),
                StrokeKind::Inside,
            );
            if text && r.width() > 60.0 {
                let size = format::bytes_short(e.size);
                let size_g = p.layout_no_wrap(size, theme::font(11.0, theme::MEDIUM), t.text2);
                let max_label = r.width() - size_g.size().x - 22.0;
                if max_label > 24.0 {
                    let label = paint::elided(
                        p,
                        &e.label,
                        theme::font(12.0, theme::SEMIBOLD),
                        t.text,
                        max_label,
                    );
                    p.galley(pos2(r.left() + 9.0, r.top() + 5.0), label, t.text);
                    p.galley(
                        pos2(r.right() - 9.0 - size_g.size().x, r.top() + 6.0),
                        size_g,
                        t.text2,
                    );
                }
            }
            continue;
        }

        if e.synthetic {
            p.rect_filled(r, radius, e.color);
        } else {
            paint::gradient_rect(p, r, radius, e.color, gradient);
            if r.height() > 10.0 && r.width() > 10.0 {
                let top = Rect::from_min_size(
                    r.min + vec2(radius.nw as f32 * 0.6, 0.0),
                    vec2(r.width() - radius.nw as f32 * 1.2, 1.0),
                );
                p.rect_filled(top, 0.0, Color32::from_white_alpha(46));
            }
        }
        if parent_hovered {
            p.rect_filled(r, radius, Color32::from_white_alpha(14));
        }
        if is_hovered {
            p.rect_filled(r, radius, Color32::from_white_alpha(30));
            p.rect_stroke(
                r,
                radius,
                Stroke::new(1.5, Color32::from_white_alpha(230)),
                StrokeKind::Inside,
            );
        }

        if text && r.width() > 44.0 && r.height() > 22.0 {
            let big = r.width() > 120.0 && r.height() > 52.0;
            let label_font = theme::font(if big { 13.0 } else { 11.5 }, theme::SEMIBOLD);
            let label = paint::elided(p, &e.label, label_font, t.tile_text, r.width() - 14.0);
            let lh = label.size().y;
            paint::shadowed(p, label, pos2(r.left() + 7.0, r.top() + 5.0), t.tile_text);
            if r.height() > 36.0 {
                let size = p.layout_no_wrap(
                    format::bytes_short(e.size),
                    theme::font(if big { 12.0 } else { 10.5 }, theme::MEDIUM),
                    t.tile_text,
                );
                paint::shadowed(
                    p,
                    size,
                    pos2(r.left() + 7.0, r.top() + 6.0 + lh),
                    Color32::from_white_alpha(205),
                );
            }
        }
    }
}

fn tooltip(
    ctx: &egui::Context,
    layout: &Layout,
    h: (usize, Option<usize>),
    pos: egui::Pos2,
    t: &Theme,
) {
    let (Some(e), Some(parent)) = (entry_of(layout, h), layout.entries.get(h.0)) else {
        return;
    };
    let (parent_label, parent_size) = match h.1 {
        Some(_) => (parent.label.as_str(), parent.size),
        None => (layout.title.as_str(), layout.total),
    };
    let p = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::unique("tessera-tile-tip")));
    let w = 300.0;
    let pad = 12.0;

    let mut lines: Vec<(String, egui::FontId, Color32)> = Vec::new();
    lines.push((e.label.clone(), theme::font(13.5, theme::SEMIBOLD), t.text));
    if let Some(dp) = &e.display_path {
        lines.push((dp.clone(), theme::font(11.0, theme::REGULAR), t.text3));
    }
    let pct = if parent_size > 0 {
        e.size as f64 / parent_size as f64 * 100.0
    } else {
        0.0
    };
    lines.push((
        format!(
            "{}  ·  {:.1}% of {}",
            format::bytes(e.size),
            pct,
            parent_label
        ),
        theme::font(12.0, theme::MEDIUM),
        t.text,
    ));
    let mut meta = Vec::new();
    if e.files > 0 {
        meta.push(format!("{} files", format::count(e.files)));
    }
    if e.mtime > 0 {
        meta.push(format!("modified {}", format::ago_unix(e.mtime)));
    }
    if !meta.is_empty() {
        lines.push((
            meta.join("  ·  "),
            theme::font(11.0, theme::REGULAR),
            t.text2,
        ));
    }
    if let Some(d) = &e.detail {
        lines.push((d.clone(), theme::font(11.0, theme::REGULAR), t.text2));
    }
    if e.reclaimable > 0 {
        lines.push((
            format!("{} reclaimable", format::bytes(e.reclaimable)),
            theme::font(11.0, theme::SEMIBOLD),
            t.red,
        ));
    }
    if e.target.is_some() {
        lines.push((
            "Click to open  ·  right-click for options".into(),
            theme::font(10.5, theme::REGULAR),
            t.text3,
        ));
    } else if e.path.is_some() {
        lines.push((
            "Right-click to reveal in Finder".into(),
            theme::font(10.5, theme::REGULAR),
            t.text3,
        ));
    }

    let galleys: Vec<_> = lines
        .into_iter()
        .map(|(s, f, c)| paint::elided(&p, &s, f, c, w - 2.0 * pad - 14.0))
        .collect();
    let height: f32 = galleys.iter().map(|g| g.size().y + 3.0).sum::<f32>() + 2.0 * pad - 3.0;
    let screen = ctx.content_rect();
    let mut origin = pos + vec2(18.0, 20.0);
    if origin.x + w > screen.right() - 8.0 {
        origin.x = pos.x - w - 14.0;
    }
    if origin.y + height > screen.bottom() - 8.0 {
        origin.y = pos.y - height - 12.0;
    }
    let rect = Rect::from_min_size(origin, vec2(w, height));
    let shadow = egui::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(if t.dark { 120 } else { 40 }),
    };
    p.add(shadow.as_shape(rect, CornerRadius::same(12)));
    let bg = if t.dark {
        Color32::from_rgb(0x1A, 0x1F, 0x29)
    } else {
        Color32::WHITE
    };
    p.rect_filled(rect, CornerRadius::same(12), bg);
    p.rect_stroke(
        rect,
        CornerRadius::same(12),
        Stroke::new(1.0, t.border),
        StrokeKind::Inside,
    );
    p.circle_filled(
        pos2(rect.left() + pad + 4.0, rect.top() + pad + 9.0),
        4.5,
        e.color,
    );
    let mut y = rect.top() + pad;
    for (i, g) in galleys.into_iter().enumerate() {
        let x = rect.left() + pad + if i == 0 { 14.0 } else { 0.0 };
        let gh = g.size().y;
        p.galley(pos2(x, y), g, t.text);
        y += gh + 3.0;
    }
}

impl DiskMap {
    fn header_right(
        &mut self,
        ui: &mut Ui,
        p: &Painter,
        rect: Rect,
        shared: &Arc<Shared>,
        view: &crate::disk::DiskView,
        t: &Theme,
    ) {
        let y = rect.top() + 23.0;
        let mut x = rect.right() - 18.0;

        // Focus toggle.
        let label = if self.focus { "Show vitals" } else { "Focus" };
        if let Some(r) = button(ui, p, pos2(x, y), label, t, ("map-focus", 0)) {
            self.focus = !self.focus;
            x = r.left() - 8.0;
        } else {
            x -= text_button_width(p, label) + 8.0;
        }

        // Rescan / progress.
        match &view.scan {
            Some(scan) => {
                let files = scan.progress.files.load(Ordering::Relaxed);
                let bytes = scan.progress.bytes.load(Ordering::Relaxed);
                let text = format!(
                    "Scanning  {} files  ·  {}",
                    format::count(files),
                    format::bytes_short(bytes)
                );
                let g = p.layout_no_wrap(text, theme::font(12.0, theme::MEDIUM), t.text2);
                let gx = x - g.size().x;
                p.galley(pos2(gx, y - g.size().y / 2.0), g, t.text2);
                spinner(
                    p,
                    pos2(gx - 14.0, y),
                    6.0,
                    t.blue,
                    ui.ctx().input(|i| i.time),
                );
                x = gx - 30.0;
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(50));
            }
            None => {
                if button(ui, p, pos2(x, y), "Rescan", t, ("map-rescan", 0)).is_some() {
                    crate::statusbar::start_scan(shared, ScanPriority::Interactive);
                }
                x -= text_button_width(p, "Rescan") + 12.0;
                if let Some(index) = &view.index {
                    let text = format!(
                        "Scanned {}  ·  {} files",
                        format::ago(index.scanned_at),
                        format::count(index.stats.files)
                    );
                    let g = p.layout_no_wrap(text, theme::font(12.0, theme::REGULAR), t.text3);
                    let gx = x - g.size().x;
                    p.galley(pos2(gx, y - g.size().y / 2.0), g, t.text3);
                    x = gx - 14.0;
                }
            }
        }

        // Limited access hint.
        if let Some(index) = &view.index
            && index.stats.denied > 0
        {
            let granted = match self.fda {
                Some((g, v)) if g == view.generation => v,
                _ => {
                    let v = macos::has_full_disk_access();
                    self.fda = Some((view.generation, v));
                    v
                }
            };
            if !granted {
                let label = "Grant Full Disk Access";
                let w = text_button_width(p, label);
                if x - w > rect.left() + 420.0
                    && button(ui, p, pos2(x, y), label, t, ("map-fda", 0)).is_some()
                {
                    macos::open_full_disk_access_settings();
                }
            }
        }
        if let Some(err) = &view.error {
            p.text(
                pos2(rect.left() + 18.0, rect.bottom() - 10.0),
                Align2::LEFT_BOTTOM,
                format!("Scan failed: {err}"),
                theme::font(11.0, theme::REGULAR),
                t.red,
            );
        }
    }

    fn empty_state(
        &mut self,
        ui: &mut Ui,
        p: &Painter,
        body: Rect,
        shared: &Arc<Shared>,
        view: &crate::disk::DiskView,
        t: &Theme,
    ) {
        let time = ui.ctx().input(|i| i.time);
        // Shimmering placeholder mosaic.
        let weights = [34.0, 21.0, 13.0, 8.0, 5.0, 3.0, 2.0, 1.0];
        let rects = squarify(&weights, to_t(body));
        let palette = [
            t.violet, t.blue, t.teal, t.pink, t.amber, t.indigo, t.orange, t.cyan,
        ];
        for (i, r) in rects.into_iter().enumerate() {
            let r = from_t(r).shrink(2.0);
            let phase = ((time * 1.6 - i as f64 * 0.35).sin() * 0.5 + 0.5) as f32;
            let alpha = if view.scan.is_some() {
                0.10 + 0.10 * phase
            } else {
                0.10
            };
            p.rect_filled(
                r,
                CornerRadius::same(8),
                with_alpha(palette[i % palette.len()], alpha),
            );
        }
        let c = body.center();
        let panel = Rect::from_center_size(c, vec2(380.0, 128.0));
        let bg = if t.dark {
            Color32::from_rgba_unmultiplied(16, 19, 26, 230)
        } else {
            Color32::from_rgba_unmultiplied(255, 255, 255, 235)
        };
        p.rect_filled(panel, CornerRadius::same(16), bg);
        p.rect_stroke(
            panel,
            CornerRadius::same(16),
            Stroke::new(1.0, t.border),
            StrokeKind::Inside,
        );
        match &view.scan {
            Some(scan) => {
                let files = scan.progress.files.load(Ordering::Relaxed);
                let bytes = scan.progress.bytes.load(Ordering::Relaxed);
                p.text(
                    c + vec2(0.0, -30.0),
                    Align2::CENTER_CENTER,
                    "Mapping your disk…",
                    theme::font(17.0, theme::SEMIBOLD),
                    t.text,
                );
                p.text(
                    c + vec2(0.0, -4.0),
                    Align2::CENTER_CENTER,
                    format!(
                        "{} files  ·  {}  ·  {:.0}s",
                        format::count(files),
                        format::bytes_short(bytes),
                        scan.started.elapsed().as_secs_f32()
                    ),
                    theme::font(12.5, theme::MEDIUM),
                    t.text2,
                );
                p.text(
                    c + vec2(0.0, 22.0),
                    Align2::CENTER_CENTER,
                    "The first scan takes a moment; later launches open instantly.",
                    theme::font(11.5, theme::REGULAR),
                    t.text3,
                );
                spinner(p, c + vec2(0.0, 44.0), 6.0, t.blue, time);
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(50));
            }
            None => {
                p.text(
                    c + vec2(0.0, -30.0),
                    Align2::CENTER_CENTER,
                    "No disk map yet",
                    theme::font(17.0, theme::SEMIBOLD),
                    t.text,
                );
                p.text(
                    c + vec2(0.0, -6.0),
                    Align2::CENTER_CENTER,
                    view.error
                        .clone()
                        .unwrap_or_else(|| "Scan the disk to see where your space goes.".into()),
                    theme::font(12.0, theme::REGULAR),
                    t.text2,
                );
                if button(
                    ui,
                    p,
                    pos2(c.x + 40.0, c.y + 30.0),
                    "Scan now",
                    t,
                    ("empty-scan", 0),
                )
                .is_some()
                {
                    crate::statusbar::start_scan(shared, ScanPriority::Interactive);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn side_list(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        index: &DiskIndex,
        overview: &Overview,
        map_rect: Rect,
        generation: u64,
        snap: &Snapshot,
        t: &Theme,
        now: f64,
    ) {
        let Some(layout) = self.layout.as_ref() else {
            return;
        };
        let p = ui.painter_at(rect);
        let title = layout.title.clone();
        let total = layout.total;
        let count = layout.entries.len();
        p.text(
            pos2(rect.left(), rect.top() - 1.0),
            Align2::LEFT_TOP,
            paint::elided(
                &p,
                &title,
                theme::font(15.0, theme::SEMIBOLD),
                t.text,
                rect.width() - 80.0,
            )
            .text(),
            theme::font(15.0, theme::SEMIBOLD),
            t.text,
        );
        p.text(
            pos2(rect.right(), rect.top() + 1.0),
            Align2::RIGHT_TOP,
            format::bytes_short(total),
            theme::font(13.0, theme::SEMIBOLD),
            t.text2,
        );
        let sub = match self.stack.last() {
            Some(Level::Node { id, .. }) => index.display_path(*id),
            Some(Level::Group(c)) => c.blurb().to_string(),
            _ => {
                let r = overview.reclaimable();
                if r > 0 {
                    format!(
                        "{} reclaimable in caches and build output",
                        format::bytes_short(r)
                    )
                } else {
                    "Everything on this Mac".into()
                }
            }
        };
        let sub_g = paint::elided(
            &p,
            &sub,
            theme::font(11.0, theme::REGULAR),
            t.text3,
            rect.width(),
        );
        p.galley(pos2(rect.left(), rect.top() + 21.0), sub_g, t.text3);

        let list = Rect::from_min_max(
            pos2(rect.left() - 6.0, rect.top() + 44.0),
            pos2(rect.right() + 6.0, rect.bottom()),
        );
        let mut clicked: Option<usize> = None;
        let hovered_entry = self.hovered.map(|h| h.0);
        let mut list_hover = None;
        let entries: Vec<(String, u64, Color32, bool)> = layout
            .entries
            .iter()
            .map(|e| (e.label.clone(), e.size, e.color, e.target.is_some()))
            .collect();
        ui.scope_builder(UiBuilder::new().max_rect(list), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("disk-list")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (i, (label, size, color, openable)) in entries.iter().enumerate() {
                        let (r, resp) = ui
                            .allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                        let p = ui.painter();
                        let active = resp.hovered() || hovered_entry == Some(i);
                        if active {
                            p.rect_filled(r, CornerRadius::same(8), t.track);
                        }
                        if resp.hovered() {
                            list_hover = Some(i);
                        }
                        if resp.clicked() && *openable {
                            clicked = Some(i);
                        }
                        let sq = Rect::from_min_size(
                            pos2(r.left() + 8.0, r.top() + 9.0),
                            vec2(10.0, 10.0),
                        );
                        p.rect_filled(sq, CornerRadius::same(3), *color);
                        let pct = if total > 0 {
                            *size as f32 / total as f32
                        } else {
                            0.0
                        };
                        let size_g = p.layout_no_wrap(
                            format::bytes_short(*size),
                            theme::font(12.0, theme::MEDIUM),
                            t.text,
                        );
                        let name_w = r.width() - 34.0 - size_g.size().x - 10.0;
                        let name = paint::elided(
                            p,
                            label,
                            theme::font(12.5, theme::MEDIUM),
                            t.text,
                            name_w,
                        );
                        p.galley(pos2(r.left() + 26.0, r.top() + 6.0), name, t.text);
                        p.galley(
                            pos2(r.right() - 8.0 - size_g.size().x, r.top() + 7.0),
                            size_g,
                            t.text,
                        );
                        let bar = Rect::from_min_size(
                            pos2(r.left() + 26.0, r.top() + 24.0),
                            vec2(r.width() - 34.0, 3.0),
                        );
                        paint::bar(p, bar, &[(pct, with_alpha(*color, 0.9))], t);
                    }
                    if count == 0 {
                        ui.label(egui::RichText::new("Nothing here").color(t.text3));
                    }
                });
        });
        self.list_hovered = list_hover;
        if let Some(i) = clicked {
            self.open(
                (i, None),
                index,
                overview,
                map_rect,
                generation,
                snap,
                t,
                now,
            );
        }
    }
}

fn text_button_width(p: &Painter, label: &str) -> f32 {
    p.layout_no_wrap(
        label.to_string(),
        theme::font(12.0, theme::SEMIBOLD),
        Color32::WHITE,
    )
    .size()
    .x + 22.0
}

/// A small pill button right-aligned at `anchor`. Returns its rect if clicked.
fn button(
    ui: &mut Ui,
    p: &Painter,
    anchor: egui::Pos2,
    label: &str,
    t: &Theme,
    id: (&str, usize),
) -> Option<Rect> {
    let g = p.layout_no_wrap(
        label.to_string(),
        theme::font(12.0, theme::SEMIBOLD),
        t.text,
    );
    let rect = Align2::RIGHT_CENTER.anchor_size(anchor, vec2(g.size().x + 22.0, 26.0));
    let resp = ui.interact(rect, ui.scope_id().with(id), Sense::click());
    let fill = if resp.is_pointer_button_down_on() {
        with_alpha(t.blue, 0.35)
    } else if resp.hovered() {
        with_alpha(t.blue, 0.24)
    } else {
        with_alpha(t.blue, 0.15)
    };
    p.rect_filled(rect, CornerRadius::same(13), fill);
    p.rect_stroke(
        rect,
        CornerRadius::same(13),
        Stroke::new(1.0, with_alpha(t.blue, 0.35)),
        StrokeKind::Inside,
    );
    p.galley(
        pos2(rect.left() + 11.0, rect.center().y - g.size().y / 2.0),
        g,
        t.text,
    );
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked().then_some(rect)
}

fn spinner(p: &Painter, center: egui::Pos2, radius: f32, color: Color32, time: f64) {
    let start = (time * 4.0) as f32 % std::f32::consts::TAU;
    paint::arc(p, center, radius, start, start + 4.2, 2.0, color);
}

/// `TESSERA_OPEN=projects` or `TESSERA_OPEN=projects:/Users/me/code/app`
/// opens the map at that level (development aid for screenshots).
fn debug_initial_level(stack: &mut Vec<Level>, index: &DiskIndex, overview: &Overview) {
    let Ok(spec) = std::env::var("TESSERA_OPEN") else {
        return;
    };
    if stack.len() > 1 {
        return;
    }
    let (cat, path) = spec
        .split_once(':')
        .map(|(c, p)| (c, Some(p)))
        .unwrap_or((spec.as_str(), None));
    let Some(group) = overview.groups.iter().find(|g| g.category.id() == cat) else {
        return;
    };
    stack.push(Level::Group(group.category));
    if let Some(path) = path
        && let Some(id) = index.find(std::path::Path::new(path))
    {
        stack.push(Level::Node {
            id,
            cat: group.category,
            path: index.path(id),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, RawInput};
    use tessera_core::disk::{ScanConfig, ScanProgress, scan};

    fn fixture() -> (std::path::PathBuf, DiskIndex) {
        let dir = std::env::temp_dir().join(format!("tessera-map-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (proj, mb) in [("alpha", 6), ("beta", 3), ("gamma", 2)] {
            let p = dir.join(proj);
            std::fs::create_dir_all(p.join("src")).unwrap();
            std::fs::create_dir_all(p.join("target")).unwrap();
            std::fs::write(p.join("Cargo.toml"), "[package]").unwrap();
            std::fs::write(p.join("src/data.bin"), vec![1u8; mb << 20]).unwrap();
            std::fs::write(p.join("target/out.bin"), vec![2u8; mb << 19]).unwrap();
        }
        let cfg = ScanConfig {
            min_dir: 0,
            min_file: 1 << 30,
            ..ScanConfig::for_path(&dir)
        };
        let index = scan(&cfg, &ScanProgress::default()).unwrap();
        (dir, index)
    }

    #[test]
    fn click_drills_in_and_escape_goes_back() {
        let (dir, index) = fixture();
        let shared = Shared::new(crate::settings::Settings::default());
        shared.disk.set_index(index);
        let ctx = egui::Context::default();
        super::super::theme::install_fonts(&ctx);
        let t = Theme::dark();
        let snap = Snapshot::default();
        let mut map = DiskMap::default();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0));
        let area = Rect::from_min_size(pos2(20.0, 20.0), vec2(1160.0, 760.0));

        let mut time = 0.0;
        let mut frame = |map: &mut DiskMap, events: Vec<Event>| {
            time += 1.0 / 60.0;
            let input = RawInput {
                screen_rect: Some(screen),
                events,
                time: Some(time),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                map.ui(ui, area, &shared, &snap, &t, TextureId::default())
            });
            out.textures_delta.clear();
        };
        frame(&mut map, vec![]);
        frame(&mut map, vec![]);
        assert_eq!(map.stack, vec![Level::Overview]);

        // The overview has one group (Projects) with the three projects nested.
        let layout = map.layout.as_ref().unwrap();
        let alpha = layout
            .tiles
            .iter()
            .find(|tile| {
                tile.child.is_some()
                    && entry_of(layout, (tile.entry, tile.child))
                        .is_some_and(|e| e.label == "alpha")
            })
            .copied()
            .expect("alpha tile");
        let pos = alpha.rect.center();
        let press = |pressed| Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(&mut map, vec![Event::PointerMoved(pos)]);
        assert_eq!(map.hovered, Some((alpha.entry, alpha.child)));
        frame(&mut map, vec![press(true)]);
        frame(&mut map, vec![press(false)]);
        assert_eq!(
            map.stack.len(),
            3,
            "group + project pushed: {:?}",
            map.stack
        );
        assert!(matches!(&map.stack[2], Level::Node { path, .. } if path.ends_with("alpha")));
        assert_eq!(map.layout.as_ref().unwrap().title, "alpha");
        assert!(map.transition.is_some());

        // Let the animation finish, then go back up with Escape.
        for _ in 0..40 {
            frame(&mut map, vec![]);
        }
        let esc = Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        frame(&mut map, vec![esc]);
        assert_eq!(map.stack.len(), 2);
        assert_eq!(map.stack[1], Level::Group(Category::Projects));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
