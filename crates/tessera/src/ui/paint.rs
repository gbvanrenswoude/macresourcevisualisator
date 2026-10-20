//! Drawing helpers shared by the dashboard: cards, charts, gauges, text.

use std::sync::Arc;

use egui::epaint::{Mesh, PathStroke, RectShape, Shadow, TextureId};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align2, Color32, CornerRadius, FontId, Galley, Painter, Pos2, Rect, Shape, Stroke, StrokeKind,
    pos2, vec2,
};

use super::theme::{self, Theme};

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

pub fn with_alpha(c: Color32, alpha: f32) -> Color32 {
    c.gamma_multiply(alpha.clamp(0.0, 1.0))
}

/// Lighten (amount > 0) or darken (amount < 0) an opaque colour.
pub fn shade(c: Color32, amount: f32) -> Color32 {
    if amount >= 0.0 {
        lerp_color(c, Color32::WHITE, amount)
    } else {
        lerp_color(c, Color32::BLACK, -amount)
    }
}

/// A soft radial glow (triangle fan fading to transparent).
pub fn glow(p: &Painter, center: Pos2, radius: f32, color: Color32) {
    let mut mesh = Mesh::default();
    let segments = 64;
    mesh.colored_vertex(center, color);
    for i in 0..=segments {
        let a = i as f32 / segments as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(
            center + vec2(a.cos(), a.sin()) * radius,
            Color32::TRANSPARENT,
        );
    }
    for i in 0..segments {
        mesh.add_triangle(0, i + 1, i + 2);
    }
    p.add(Shape::mesh(mesh));
}

pub fn card(p: &Painter, rect: Rect, t: &Theme) {
    let r = CornerRadius::same(16);
    let shadow = Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: t.shadow,
    };
    p.add(shadow.as_shape(rect, r));
    p.rect_filled(rect, r, t.card);
    p.rect_stroke(rect, r, Stroke::new(1.0, t.border), StrokeKind::Inside);
    // Faint top highlight gives the glass a bevel.
    let top = Rect::from_min_size(rect.min + vec2(16.0, 0.5), vec2(rect.width() - 32.0, 1.0));
    p.rect_filled(top, 0.0, t.highlight);
}

/// Small uppercase, letter-spaced section title.
pub fn section(p: &Painter, pos: Pos2, text: &str, t: &Theme) -> Rect {
    let mut job = LayoutJob::default();
    job.append(
        &text.to_uppercase(),
        0.0,
        TextFormat {
            font_id: theme::font(10.5, theme::SEMIBOLD),
            color: t.text3,
            extra_letter_spacing: 0.9,
            ..Default::default()
        },
    );
    let galley = p.layout_job(job);
    let rect = Rect::from_min_size(pos, galley.size());
    p.galley(pos, galley, t.text3);
    rect
}

/// Lay out a single line, truncating with "…" to fit `max_width`.
pub fn elided(
    p: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(
        text.to_string(),
        TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
    job.wrap = TextWrapping {
        max_width: max_width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    p.layout_job(job)
}

/// A filled area chart with a gradient that fades towards the baseline.
#[allow(clippy::too_many_arguments)]
pub fn area_chart(
    p: &Painter,
    rect: Rect,
    values: &[f32],
    capacity: usize,
    max: f32,
    color: Color32,
    fill_alpha: f32,
    line_width: f32,
) {
    if values.len() < 2 || rect.width() <= 0.0 {
        return;
    }
    let max = max.max(1e-6);
    let step = rect.width() / (capacity.max(2) - 1) as f32;
    let x0 = rect.right() - step * (values.len() - 1) as f32;
    let pts: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            pos2(
                x0 + step * i as f32,
                rect.bottom() - (v / max).clamp(0.0, 1.0) * rect.height(),
            )
        })
        .collect();

    let mut mesh = Mesh::default();
    let top = with_alpha(color, fill_alpha);
    for pt in &pts {
        let k = 1.0 - (pt.y - rect.top()) / rect.height().max(1.0);
        mesh.colored_vertex(
            *pt,
            lerp_color(with_alpha(color, fill_alpha * 0.25), top, k),
        );
        mesh.colored_vertex(pos2(pt.x, rect.bottom()), Color32::TRANSPARENT);
    }
    for i in 0..(pts.len() as u32 - 1) {
        let a = i * 2;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 1, a + 3, a + 2);
    }
    p.add(Shape::mesh(mesh));
    p.add(Shape::line(pts, PathStroke::new(line_width, color)));
}

pub fn gridlines(p: &Painter, rect: Rect, rows: usize, t: &Theme) {
    for i in 1..rows {
        let y = rect.top() + rect.height() * i as f32 / rows as f32;
        p.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(1.0, t.grid),
        );
    }
}

/// Arc from `start` to `end` (radians, 0 = 12 o'clock, clockwise).
pub fn arc(
    p: &Painter,
    center: Pos2,
    radius: f32,
    start: f32,
    end: f32,
    width: f32,
    color: Color32,
) {
    if end - start <= 0.001 {
        return;
    }
    let n = ((end - start) / 0.04).ceil().max(2.0) as usize;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = start + (end - start) * i as f32 / n as f32 - std::f32::consts::FRAC_PI_2;
            center + vec2(a.cos(), a.sin()) * radius
        })
        .collect();
    p.add(Shape::line(pts, PathStroke::new(width, color)));
    // Round caps.
    for a in [start, end] {
        let a = a - std::f32::consts::FRAC_PI_2;
        p.circle_filled(center + vec2(a.cos(), a.sin()) * radius, width / 2.0, color);
    }
}

/// A ring gauge made of consecutive segments (fractions of the full circle).
pub fn ring(
    p: &Painter,
    center: Pos2,
    radius: f32,
    width: f32,
    segments: &[(f32, Color32)],
    t: &Theme,
) {
    p.circle_stroke(center, radius, Stroke::new(width, t.track));
    let tau = std::f32::consts::TAU;
    let gap = 0.035;
    let mut a = 0.0;
    let visible: Vec<&(f32, Color32)> = segments.iter().filter(|(f, _)| *f > 0.004).collect();
    for (frac, color) in &visible {
        let span = frac.clamp(0.0, 1.0) * tau;
        let (s, e) = if visible.len() > 1 {
            (a + gap / 2.0, a + span - gap / 2.0)
        } else {
            (a, a + span)
        };
        if e > s {
            arc(p, center, radius, s, e.min(tau - 0.001), width, *color);
        }
        a += span;
    }
}

/// A horizontal bar of consecutive segments over a rounded track.
pub fn bar(p: &Painter, rect: Rect, segments: &[(f32, Color32)], t: &Theme) {
    let r = CornerRadius::from(rect.height() / 2.0);
    p.rect_filled(rect, r, t.track);
    let mut x = rect.left();
    let n = segments.len();
    for (i, (frac, color)) in segments.iter().enumerate() {
        let w = frac.clamp(0.0, 1.0) * rect.width();
        if w < 0.5 {
            continue;
        }
        let seg = Rect::from_min_size(
            pos2(x, rect.top()),
            vec2(w.min(rect.right() - x), rect.height()),
        );
        let mut cr = CornerRadius::ZERO;
        if i == 0 || x <= rect.left() + 0.5 {
            cr.nw = r.nw;
            cr.sw = r.sw;
        }
        if i + 1 == n || seg.right() >= rect.right() - 0.5 {
            cr.ne = r.ne;
            cr.se = r.se;
        }
        p.rect_filled(seg, cr, *color);
        x += w;
    }
}

/// Pill-shaped badge with a coloured dot. Returns its rect.
pub fn pill(
    p: &Painter,
    anchor: Pos2,
    align: Align2,
    label: &str,
    color: Color32,
    t: &Theme,
) -> Rect {
    let galley = p.layout_no_wrap(label.to_string(), theme::font(11.5, theme::MEDIUM), t.text);
    let size = vec2(galley.size().x + 30.0, 24.0);
    let rect = align.anchor_size(anchor, size);
    p.rect_filled(rect, CornerRadius::same(12), with_alpha(color, 0.16));
    p.rect_stroke(
        rect,
        CornerRadius::same(12),
        Stroke::new(1.0, with_alpha(color, 0.35)),
        StrokeKind::Inside,
    );
    p.circle_filled(pos2(rect.left() + 13.0, rect.center().y), 4.0, color);
    p.galley(
        pos2(rect.left() + 22.0, rect.center().y - galley.size().y / 2.0),
        galley,
        t.text,
    );
    rect
}

/// Rect filled with a vertical gradient, via a tinted gradient texture so
/// corners stay rounded and anti-aliased.
pub fn gradient_rect(
    p: &Painter,
    rect: Rect,
    radius: CornerRadius,
    color: Color32,
    texture: TextureId,
) {
    let shape = RectShape::filled(rect, radius, color)
        .with_texture(texture, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
    p.add(shape);
}

/// Text with a soft dark shadow, for labels on top of coloured tiles.
pub fn shadowed(p: &Painter, galley: Arc<Galley>, pos: Pos2, color: Color32) {
    p.galley_with_override_text_color(
        pos + vec2(0.0, 1.0),
        galley.clone(),
        Color32::from_black_alpha(70),
    );
    p.galley_with_override_text_color(pos, galley, color);
}
