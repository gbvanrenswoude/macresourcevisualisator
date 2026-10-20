//! The dashboard window: live vitals on top, the disk map below.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{
    Align2, Color32, ColorImage, CornerRadius, Rect, Sense, TextureHandle, TextureOptions, Ui,
    ViewportCommand, pos2, vec2,
};
use tessera_core::format;
use tessera_core::metrics::{AppUsage, CoreKind, Pressure, Snapshot};

use super::diskmap::DiskMap;
use super::paint::{self, with_alpha};
use super::theme::{self, Theme};
use crate::macos;
use crate::state::{Busyness, History, Shared};

pub struct Dashboard {
    disk_map: DiskMap,
    icons: HashMap<String, Option<TextureHandle>>,
    gradient: Option<TextureHandle>,
    apps_by_memory: bool,
    applied_dark: Option<bool>,
}

struct Data {
    snap: Snapshot,
    history: History,
    apps: Vec<AppUsage>,
}

const MARGIN: f32 = 24.0;
const GAP: f32 = 14.0;

impl Dashboard {
    pub fn new() -> Self {
        Self {
            disk_map: DiskMap::default(),
            icons: HashMap::new(),
            gradient: None,
            apps_by_memory: false,
            applied_dark: None,
        }
    }

    fn gradient(&mut self, ctx: &egui::Context) -> egui::TextureId {
        self.gradient
            .get_or_insert_with(|| {
                // Vertical ramp: full colour at the top, ~80% at the bottom.
                let pixels = (0..64)
                    .map(|i| {
                        let k = i as f32 / 63.0;
                        let v = (255.0 * (1.0 - 0.22 * k)) as u8;
                        Color32::from_gray(v)
                    })
                    .collect();
                ctx.load_texture(
                    "tile-gradient",
                    ColorImage::new([1, 64], pixels),
                    TextureOptions::LINEAR,
                )
            })
            .id()
    }

    fn icon(&mut self, ctx: &egui::Context, app: &AppUsage) -> Option<egui::TextureId> {
        let key = app.bundle.clone().unwrap_or_else(|| "/bin/zsh".into());
        self.icons
            .entry(key.clone())
            .or_insert_with(|| {
                let path = if app.bundle.is_some() {
                    key.clone()
                } else {
                    "/bin/zsh".into()
                };
                macos::icon_rgba(&path, 64).map(|(w, h, rgba)| {
                    ctx.load_texture(
                        format!("icon:{key}"),
                        ColorImage::from_rgba_premultiplied([w, h], &rgba),
                        TextureOptions::LINEAR,
                    )
                })
            })
            .as_ref()
            .map(|t| t.id())
    }

    pub fn ui(&mut self, ui: &mut Ui, shared: &Arc<Shared>) {
        let ctx = ui.ctx().clone();
        let t = Theme::for_ctx(&ctx);
        if self.applied_dark != Some(t.dark) {
            t.apply(&ctx);
            self.applied_dark = Some(t.dark);
        }
        let gradient = self.gradient(&ctx);
        let screen = ui.max_rect();
        background(ui.painter(), screen, &t);

        let data = {
            let m = shared.metrics.lock().unwrap();
            Data {
                snap: m.latest.clone(),
                history: m.history.clone(),
                apps: m.apps.iter().take(8).cloned().collect(),
            }
        };

        let min_height = 880.0;
        let focus = self.disk_map.focused();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let height = if focus {
                    screen.height()
                } else {
                    screen.height().max(min_height)
                };
                let (rect, _) =
                    ui.allocate_exact_size(vec2(screen.width(), height), Sense::hover());
                self.layout(ui, rect, shared, &data, &t, gradient);
            });
    }

    fn layout(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        shared: &Arc<Shared>,
        d: &Data,
        t: &Theme,
        gradient: egui::TextureId,
    ) {
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();

        // Title bar strip + header double as the window's drag handle.
        let drag_rect = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 88.0));
        let drag = ui.interact(
            drag_rect,
            ui.scope_id().with("drag"),
            Sense::click_and_drag(),
        );
        if drag.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }

        let left = rect.left() + MARGIN;
        let right = rect.right() - MARGIN;
        let width = right - left;

        // Header -------------------------------------------------------------
        let header = Rect::from_min_max(
            pos2(left, rect.top() + 34.0),
            pos2(right, rect.top() + 82.0),
        );
        self.header(&p, header, shared, d, t, gradient);

        let focus = self.disk_map.focused();
        let mut y = header.bottom() + 12.0;
        if !focus {
            // Vitals row.
            let row = Rect::from_min_size(pos2(left, y), vec2(width, 176.0));
            let cols = split(row, &[1.3, 1.2, 1.0, 1.15], GAP);
            self.cpu_card(&p, cols[0], d, shared, t);
            self.memory_card(&p, cols[1], d, t);
            self.pressure_card(&p, cols[2], d, t);
            self.disk_card(&p, cols[3], d, shared, t);
            y = row.bottom() + GAP;

            // Second row.
            let row = Rect::from_min_size(pos2(left, y), vec2(width, 196.0));
            let cols = split(row, &[1.75, 1.0, 1.2], GAP);
            self.cores_card(&p, cols[0], d, shared, t);
            self.io_card(&p, cols[1], d, t);
            self.apps_card(ui, &p, cols[2], d, t);
            y = row.bottom() + GAP;
        }

        // Disk map.
        let map = Rect::from_min_max(pos2(left, y), pos2(right, rect.bottom() - MARGIN + 4.0));
        paint::card(&p, map, t);
        self.disk_map.ui(ui, map, shared, &d.snap, t, gradient);
    }

    fn header(
        &self,
        p: &egui::Painter,
        rect: Rect,
        shared: &Shared,
        d: &Data,
        t: &Theme,
        gradient: egui::TextureId,
    ) {
        let logo = Rect::from_min_size(pos2(rect.left(), rect.center().y - 20.0), vec2(40.0, 40.0));
        super::logo::paint(p, logo, gradient);
        let tx = logo.right() + 14.0;
        p.text(
            pos2(tx, rect.center().y - 2.0),
            Align2::LEFT_BOTTOM,
            "Tessera",
            theme::font(22.0, theme::BOLD),
            t.text,
        );
        let info = &shared.info;
        let mem = format::bytes_short(info.memory_total);
        let subtitle = format!(
            "{}  ·  {}  ·  {} memory  ·  macOS {}",
            info.model_name, info.chip, mem, info.os_version
        );
        p.text(
            pos2(tx, rect.center().y + 3.0),
            Align2::LEFT_TOP,
            subtitle,
            theme::font(12.5, theme::REGULAR),
            t.text2,
        );

        // Right: overall busyness + uptime.
        let busy = Busyness::of(&d.snap);
        let pill = paint::pill(
            p,
            pos2(rect.right(), rect.center().y),
            Align2::RIGHT_CENTER,
            busy.label(),
            t.severity(busy.severity()),
            t,
        );
        p.text(
            pos2(pill.left() - 14.0, rect.center().y),
            Align2::RIGHT_CENTER,
            format!("Up {}", format::duration(d.snap.uptime)),
            theme::font(12.0, theme::MEDIUM),
            t.text3,
        );
    }

    fn cpu_card(&self, p: &egui::Painter, rect: Rect, d: &Data, shared: &Shared, t: &Theme) {
        paint::card(p, rect, t);
        let x = rect.left() + 18.0;
        paint::section(p, pos2(x, rect.top() + 16.0), "CPU", t);
        let cores = shared.info.cores.len();
        p.text(
            pos2(rect.right() - 18.0, rect.top() + 15.0),
            Align2::RIGHT_TOP,
            format!("{cores} cores"),
            theme::font(11.5, theme::MEDIUM),
            t.text3,
        );
        big_number(
            p,
            pos2(x, rect.top() + 34.0),
            &format!("{:.0}", d.snap.cpu.total * 100.0),
            "%",
            t.text,
            t,
        );
        p.text(
            pos2(rect.right() - 18.0, rect.top() + 44.0),
            Align2::RIGHT_TOP,
            format!("user {:.0}%", d.snap.cpu.user * 100.0),
            theme::font(12.0, theme::MEDIUM),
            t.blue,
        );
        p.text(
            pos2(rect.right() - 18.0, rect.top() + 62.0),
            Align2::RIGHT_TOP,
            format!("system {:.0}%", d.snap.cpu.system * 100.0),
            theme::font(12.0, theme::MEDIUM),
            t.violet,
        );
        let chart = Rect::from_min_max(
            pos2(x, rect.top() + 92.0),
            pos2(rect.right() - 18.0, rect.bottom() - 16.0),
        );
        paint::gridlines(p, chart, 4, t);
        let total: Vec<f32> = d.history.cpu.iter().collect();
        let user: Vec<f32> = d.history.cpu_user.iter().collect();
        paint::area_chart(
            p,
            chart,
            &total,
            d.history.cpu.capacity(),
            1.0,
            t.violet,
            0.30,
            1.6,
        );
        paint::area_chart(
            p,
            chart,
            &user,
            d.history.cpu_user.capacity(),
            1.0,
            t.blue,
            0.38,
            1.8,
        );
    }

    fn memory_card(&self, p: &egui::Painter, rect: Rect, d: &Data, t: &Theme) {
        paint::card(p, rect, t);
        let m = &d.snap.memory;
        let x = rect.left() + 18.0;
        paint::section(p, pos2(x, rect.top() + 16.0), "Memory", t);
        let total = m.total.max(1) as f32;
        let segments = [
            (m.app as f32 / total, t.blue),
            (m.wired as f32 / total, t.orange),
            (m.compressed as f32 / total, t.violet),
            (m.cached as f32 / total, with_alpha(t.text3, 0.55)),
        ];
        let center = pos2(x + 50.0, rect.top() + 100.0);
        paint::ring(p, center, 44.0, 11.0, &segments, t);
        p.text(
            center + vec2(0.0, -3.0),
            Align2::CENTER_BOTTOM,
            format::bytes_short(m.used).replace(" GB", ""),
            theme::font(22.0, theme::ROUNDED),
            t.text,
        );
        p.text(
            center + vec2(0.0, 1.0),
            Align2::CENTER_TOP,
            format!("of {}", format::bytes_short(m.total)),
            theme::font(11.0, theme::MEDIUM),
            t.text3,
        );

        let lx = x + 116.0;
        let rows = [
            ("App", m.app, t.blue),
            ("Wired", m.wired, t.orange),
            ("Compressed", m.compressed, t.violet),
            ("Cached", m.cached, with_alpha(t.text3, 0.8)),
            ("Free", m.free, t.track),
        ];
        let mut y = rect.top() + 46.0;
        for (label, bytes, color) in rows {
            p.circle_filled(pos2(lx + 4.0, y + 7.0), 4.0, color);
            p.text(
                pos2(lx + 14.0, y),
                Align2::LEFT_TOP,
                label,
                theme::font(12.0, theme::REGULAR),
                t.text2,
            );
            p.text(
                pos2(rect.right() - 18.0, y),
                Align2::RIGHT_TOP,
                format::bytes_short(bytes),
                theme::font(12.0, theme::MEDIUM),
                t.text,
            );
            y += 23.0;
        }
    }

    fn pressure_card(&self, p: &egui::Painter, rect: Rect, d: &Data, t: &Theme) {
        paint::card(p, rect, t);
        let m = &d.snap.memory;
        let x = rect.left() + 18.0;
        paint::section(p, pos2(x, rect.top() + 16.0), "Pressure", t);
        let (color, label) = match m.pressure {
            Pressure::Normal => (t.green, "Normal"),
            Pressure::Warning => (t.amber, "Elevated"),
            Pressure::Critical => (t.red, "Critical"),
        };
        paint::pill(
            p,
            pos2(rect.right() - 14.0, rect.top() + 21.0),
            Align2::RIGHT_CENTER,
            label,
            color,
            t,
        );
        big_number(
            p,
            pos2(x, rect.top() + 34.0),
            &format!("{:.0}", m.pressure_pct),
            "%",
            t.text,
            t,
        );
        let chart = Rect::from_min_max(
            pos2(x, rect.top() + 84.0),
            pos2(rect.right() - 18.0, rect.top() + 116.0),
        );
        let values: Vec<f32> = d.history.pressure.iter().collect();
        paint::area_chart(
            p,
            chart,
            &values,
            d.history.pressure.capacity(),
            1.0,
            color,
            0.35,
            1.6,
        );

        let s = &d.snap.swap;
        let sy = rect.top() + 128.0;
        paint::section(p, pos2(x, sy), "Swap", t);
        let swap_text = if s.total > 0 {
            format!(
                "{} / {}",
                format::bytes_short(s.used),
                format::bytes_short(s.total)
            )
        } else {
            "Not in use".into()
        };
        p.text(
            pos2(rect.right() - 18.0, sy - 1.0),
            Align2::RIGHT_TOP,
            swap_text,
            theme::font(12.0, theme::MEDIUM),
            t.text,
        );
        let frac = if s.total > 0 {
            s.used as f32 / s.total as f32
        } else {
            0.0
        };
        paint::bar(
            p,
            Rect::from_min_size(pos2(x, sy + 21.0), vec2(rect.right() - 18.0 - x, 7.0)),
            &[(frac, t.pink)],
            t,
        );
    }

    fn disk_card(&self, p: &egui::Painter, rect: Rect, d: &Data, shared: &Shared, t: &Theme) {
        paint::card(p, rect, t);
        let v = &d.snap.volume;
        let x = rect.left() + 18.0;
        paint::section(p, pos2(x, rect.top() + 16.0), "Disk", t);
        p.text(
            pos2(rect.right() - 18.0, rect.top() + 15.0),
            Align2::RIGHT_TOP,
            &v.name,
            theme::font(11.5, theme::MEDIUM),
            t.text3,
        );
        let used = if v.total > 0 {
            v.used() as f32 / v.total as f32
        } else {
            0.0
        };
        let color = if used > 0.9 { t.orange } else { t.cyan };
        let center = pos2(x + 50.0, rect.top() + 100.0);
        paint::ring(p, center, 44.0, 11.0, &[(used, color)], t);
        p.text(
            center + vec2(0.0, -3.0),
            Align2::CENTER_BOTTOM,
            format!("{:.0}%", used * 100.0),
            theme::font(22.0, theme::ROUNDED),
            t.text,
        );
        p.text(
            center + vec2(0.0, 1.0),
            Align2::CENTER_TOP,
            "used",
            theme::font(11.0, theme::MEDIUM),
            t.text3,
        );

        let lx = x + 116.0;
        p.text(
            pos2(lx, rect.top() + 44.0),
            Align2::LEFT_TOP,
            format::bytes_short(v.available),
            theme::font(24.0, theme::ROUNDED),
            t.text,
        );
        p.text(
            pos2(lx, rect.top() + 74.0),
            Align2::LEFT_TOP,
            format!("free of {}", format::bytes_short(v.total)),
            theme::font(12.0, theme::REGULAR),
            t.text2,
        );
        let io = [
            ("Read", d.snap.disk_io.read_per_sec, t.cyan),
            ("Write", d.snap.disk_io.write_per_sec, t.amber),
        ];
        let mut y = rect.top() + 102.0;
        for (label, rate, c) in io {
            p.circle_filled(pos2(lx + 3.0, y + 7.0), 3.0, c);
            p.text(
                pos2(lx + 11.0, y),
                Align2::LEFT_TOP,
                label,
                theme::font(12.0, theme::REGULAR),
                t.text2,
            );
            p.text(
                pos2(rect.right() - 18.0, y),
                Align2::RIGHT_TOP,
                format::rate(rate),
                theme::font(12.0, theme::MEDIUM),
                t.text,
            );
            y += 21.0;
        }
        if let Some(o) = shared.disk.view().overview {
            let r = o.reclaimable();
            if r > 0 {
                p.text(
                    pos2(lx, y + 3.0),
                    Align2::LEFT_TOP,
                    format!("{} reclaimable", format::bytes_short(r)),
                    theme::font(11.5, theme::SEMIBOLD),
                    t.red,
                );
            }
        }
    }

    fn cores_card(&self, p: &egui::Painter, rect: Rect, d: &Data, shared: &Shared, t: &Theme) {
        paint::card(p, rect, t);
        let x = rect.left() + 18.0;
        paint::section(p, pos2(x, rect.top() + 16.0), "Cores · last 60 s", t);
        let info = &shared.info;
        let (e, pc) = (info.efficiency_cores(), info.performance_cores());
        let caption = if e + pc > 0 {
            format!("{e} efficiency · {pc} performance")
        } else {
            format!("{} cores", info.cores.len())
        };
        p.text(
            pos2(rect.right() - 18.0, rect.top() + 15.0),
            Align2::RIGHT_TOP,
            caption,
            theme::font(11.5, theme::MEDIUM),
            t.text3,
        );

        let cores = &info.cores;
        let n = cores.len().max(1);
        let groups = cores.windows(2).filter(|w| w[0] != w[1]).count();
        let area = Rect::from_min_max(
            pos2(x, rect.top() + 40.0),
            pos2(rect.right() - 18.0, rect.bottom() - 16.0),
        );
        let group_gap = 8.0;
        let row_h = ((area.height() - group_gap * groups as f32) / n as f32).max(4.0);
        let label_w = 26.0;
        let value_w = 92.0;
        let grid = Rect::from_min_max(
            pos2(area.left() + label_w, area.top()),
            pos2(area.right() - value_w, area.bottom()),
        );
        let cols = 60usize;
        let cell_w = grid.width() / cols as f32;

        let mut y = area.top();
        let mut idx_in_kind = 0;
        for (i, kind) in cores.iter().enumerate() {
            if i > 0 && cores[i - 1] != *kind {
                y += group_gap;
                idx_in_kind = 0;
            }
            idx_in_kind += 1;
            let base = match kind {
                CoreKind::Efficiency => t.teal,
                _ => t.blue,
            };
            p.text(
                pos2(area.left(), y + row_h / 2.0),
                Align2::LEFT_CENTER,
                format!("{}{}", kind.short(), idx_in_kind),
                theme::font(10.0, theme::SEMIBOLD),
                t.text3,
            );
            if let Some(ring) = d.history.cores.get(i) {
                let values: Vec<f32> = ring.iter().collect();
                let start = values.len().saturating_sub(cols);
                let shown = &values[start..];
                let offset = cols - shown.len();
                for (k, v) in shown.iter().enumerate() {
                    let cx = grid.left() + (offset + k) as f32 * cell_w;
                    let cell = Rect::from_min_size(
                        pos2(cx + 0.6, y + 0.8),
                        vec2(cell_w - 1.2, row_h - 1.6),
                    );
                    p.rect_filled(cell, CornerRadius::same(2), heat(*v, base, t));
                }
                for k in 0..offset {
                    let cx = grid.left() + k as f32 * cell_w;
                    let cell = Rect::from_min_size(
                        pos2(cx + 0.6, y + 0.8),
                        vec2(cell_w - 1.2, row_h - 1.6),
                    );
                    p.rect_filled(cell, CornerRadius::same(2), t.grid);
                }
            }
            let load = d.snap.cpu.per_core.get(i).copied().unwrap_or(0.0);
            let bar = Rect::from_min_size(
                pos2(grid.right() + 12.0, y + row_h / 2.0 - 2.5),
                vec2(value_w - 50.0, 5.0),
            );
            paint::bar(p, bar, &[(load, heat(load.max(0.35), base, t))], t);
            p.text(
                pos2(area.right(), y + row_h / 2.0),
                Align2::RIGHT_CENTER,
                format!("{:.0}%", load * 100.0),
                theme::font(10.5, theme::MEDIUM),
                t.text2,
            );
            y += row_h;
        }
    }

    fn io_card(&self, p: &egui::Painter, rect: Rect, d: &Data, t: &Theme) {
        paint::card(p, rect, t);
        let x = rect.left() + 18.0;
        let w = rect.right() - 18.0 - x;
        paint::section(p, pos2(x, rect.top() + 16.0), "Activity", t);
        let h = &d.history;
        type Row<'a> = (
            &'a str,
            String,
            Vec<(&'a crate::state::Ring, Color32)>,
            Option<f32>,
        );
        let rows: [Row; 3] = [
            (
                "Network",
                format!(
                    "↓ {}  ↑ {}",
                    format::rate(d.snap.network.read_per_sec),
                    format::rate(d.snap.network.write_per_sec)
                ),
                vec![(&h.net_rx, t.green), (&h.net_tx, t.blue)],
                None,
            ),
            (
                "Disk I/O",
                format!(
                    "R {}  W {}",
                    format::rate(d.snap.disk_io.read_per_sec),
                    format::rate(d.snap.disk_io.write_per_sec)
                ),
                vec![(&h.disk_read, t.cyan), (&h.disk_write, t.amber)],
                None,
            ),
            (
                "GPU",
                d.snap
                    .gpu
                    .map(format::percent)
                    .unwrap_or_else(|| "n/a".into()),
                vec![(&h.gpu, t.orange)],
                Some(1.0),
            ),
        ];
        let mut y = rect.top() + 40.0;
        let row_h = (rect.bottom() - 14.0 - y) / 3.0;
        for (label, value, series, fixed_max) in rows {
            p.text(
                pos2(x, y),
                Align2::LEFT_TOP,
                label,
                theme::font(12.0, theme::MEDIUM),
                t.text2,
            );
            p.text(
                pos2(x + w, y),
                Align2::RIGHT_TOP,
                value,
                theme::font(12.0, theme::MEDIUM),
                t.text,
            );
            let chart = Rect::from_min_size(pos2(x, y + 19.0), vec2(w, row_h - 26.0));
            let max = fixed_max
                .unwrap_or_else(|| series.iter().map(|(r, _)| r.max()).fold(1.0, f32::max) * 1.15);
            for (ring, color) in series {
                let values: Vec<f32> = ring.iter().collect();
                paint::area_chart(p, chart, &values, ring.capacity(), max, color, 0.28, 1.4);
            }
            y += row_h;
        }
    }

    fn apps_card(&mut self, ui: &mut Ui, p: &egui::Painter, rect: Rect, d: &Data, t: &Theme) {
        paint::card(p, rect, t);
        let ctx = ui.ctx().clone();
        let x = rect.left() + 18.0;
        paint::section(p, pos2(x, rect.top() + 16.0), "Top apps", t);

        // CPU / Memory toggle.
        let labels = ["CPU", "Memory"];
        let mut tx = rect.right() - 14.0;
        for (i, label) in labels.iter().enumerate().rev() {
            let galley = p.layout_no_wrap(
                label.to_string(),
                theme::font(11.0, theme::SEMIBOLD),
                t.text,
            );
            let r = Rect::from_min_size(
                pos2(tx - galley.size().x - 16.0, rect.top() + 11.0),
                vec2(galley.size().x + 16.0, 20.0),
            );
            let active = (i == 1) == self.apps_by_memory;
            let resp = ui.interact(r, ui.scope_id().with(("apps-sort", i)), Sense::click());
            if resp.clicked() {
                self.apps_by_memory = i == 1;
            }
            if active {
                p.rect_filled(r, CornerRadius::same(10), t.track);
            } else if resp.hovered() {
                p.rect_filled(r, CornerRadius::same(10), t.grid);
            }
            p.galley(
                pos2(r.left() + 8.0, r.center().y - galley.size().y / 2.0),
                galley,
                if active { t.text } else { t.text3 },
            );
            tx = r.left() - 2.0;
        }

        let mut apps = d.apps.clone();
        if self.apps_by_memory {
            apps.sort_by_key(|a| std::cmp::Reverse(a.memory));
        }
        let max_cpu = apps.iter().map(|a| a.cpu).fold(1.0f32, f32::max);
        let total_mem = d.snap.memory.total.max(1) as f32;
        let mut y = rect.top() + 42.0;
        let row_h = 24.0;
        for app in apps.iter().take(6) {
            if y + row_h > rect.bottom() - 8.0 {
                break;
            }
            let icon_rect = Rect::from_min_size(pos2(x, y + 2.0), vec2(19.0, 19.0));
            match self.icon(&ctx, app) {
                Some(tex) => {
                    p.image(
                        tex,
                        icon_rect,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                None => {
                    p.rect_filled(icon_rect, CornerRadius::same(5), t.track);
                }
            }
            let value = if self.apps_by_memory {
                format::bytes_short(app.memory)
            } else {
                format!("{:.0}%", app.cpu)
            };
            let frac = if self.apps_by_memory {
                app.memory as f32 / total_mem
            } else {
                app.cpu / max_cpu
            };
            let value_x = rect.right() - 18.0;
            let name_w = rect.width() - 18.0 * 2.0 - 27.0 - 64.0;
            let name = paint::elided(
                p,
                &app.name,
                theme::font(12.5, theme::MEDIUM),
                t.text,
                name_w,
            );
            p.galley(pos2(x + 27.0, y + 1.0), name, t.text);
            let bar = Rect::from_min_size(pos2(x + 27.0, y + 18.0), vec2(name_w, 3.0));
            paint::bar(
                p,
                bar,
                &[(
                    frac,
                    if self.apps_by_memory {
                        t.violet
                    } else {
                        t.blue
                    },
                )],
                t,
            );
            p.text(
                pos2(value_x, y + 2.0),
                Align2::RIGHT_TOP,
                value,
                theme::font(12.0, theme::SEMIBOLD),
                t.text,
            );
            if app.processes > 1 {
                p.text(
                    pos2(value_x, y + 16.0),
                    Align2::RIGHT_TOP,
                    format!("{} procs", app.processes),
                    theme::font(9.5, theme::REGULAR),
                    t.text3,
                );
            }
            y += row_h + 3.0;
        }
        if apps.is_empty() {
            p.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Sampling…",
                theme::font(12.0, theme::REGULAR),
                t.text3,
            );
        }
    }
}

fn big_number(
    p: &egui::Painter,
    pos: egui::Pos2,
    value: &str,
    unit: &str,
    color: Color32,
    t: &Theme,
) {
    let r = p.text(
        pos,
        Align2::LEFT_TOP,
        value,
        theme::font(40.0, theme::ROUNDED),
        color,
    );
    p.text(
        pos2(r.right() + 2.0, r.bottom() - 9.0),
        Align2::LEFT_BOTTOM,
        unit,
        theme::font(20.0, theme::ROUNDED),
        t.text2,
    );
}

/// Heatmap colour for a load value: faint at idle, saturated when busy and
/// warming towards orange near 100%.
fn heat(v: f32, base: Color32, t: &Theme) -> Color32 {
    let v = v.clamp(0.0, 1.0);
    let floor = if t.dark { 0.10 } else { 0.12 };
    let c = paint::lerp_color(with_alpha(base, floor), base, v.powf(0.75));
    if v > 0.85 {
        paint::lerp_color(c, t.orange, (v - 0.85) / 0.15)
    } else {
        c
    }
}

fn background(p: &egui::Painter, rect: Rect, t: &Theme) {
    p.rect_filled(rect, 0.0, t.bg);
    paint::glow(
        p,
        pos2(rect.left() + rect.width() * 0.18, rect.top() - 40.0),
        rect.width() * 0.55,
        t.bg_glow_a,
    );
    paint::glow(
        p,
        pos2(rect.right() - rect.width() * 0.1, rect.top() + 80.0),
        rect.width() * 0.45,
        t.bg_glow_b,
    );
}

/// Split `rect` horizontally by weights.
fn split(rect: Rect, weights: &[f32], gap: f32) -> Vec<Rect> {
    let total: f32 = weights.iter().sum();
    let avail = rect.width() - gap * (weights.len() as f32 - 1.0);
    let mut x = rect.left();
    weights
        .iter()
        .map(|w| {
            let width = avail * w / total;
            let r = Rect::from_min_size(pos2(x, rect.top()), vec2(width, rect.height()));
            x += width + gap;
            r
        })
        .collect()
}
