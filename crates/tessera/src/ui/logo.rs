//! The Tessera mark drawn as vectors (same geometry as the app icon).

use egui::epaint::{PathStroke, TextureId};
use egui::{Color32, CornerRadius, Painter, Rect, Shape, pos2, vec2};

use super::paint;

const TILES: [(f32, f32, f32, f32, u32); 4] = [
    (0.0, 0.0, 0.56, 1.0, 0x8B6CFF),
    (0.56, 0.0, 0.44, 0.52, 0x34E0C4),
    (0.56, 0.52, 0.24, 0.48, 0xFF7AB8),
    (0.80, 0.52, 0.20, 0.48, 0xFFD15C),
];

fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub fn paint(p: &Painter, rect: Rect, gradient: TextureId) {
    let s = rect.width();
    let radius = CornerRadius::from(s * 0.25);
    p.rect_filled(rect, radius, rgb(0x161B2E));
    paint::gradient_rect(p, rect, radius, rgb(0x232A47), gradient);
    let inner = rect.shrink(s * 0.16);
    let gap = s * 0.035;
    for (i, (x, y, w, h, color)) in TILES.iter().enumerate() {
        let mut r = Rect::from_min_size(
            pos2(
                inner.left() + x * inner.width(),
                inner.top() + y * inner.height(),
            ),
            vec2(w * inner.width(), h * inner.height()),
        );
        if *x > 0.0 {
            r.min.x += gap / 2.0;
        }
        if x + w < 0.999 {
            r.max.x -= gap / 2.0;
        }
        if *y > 0.0 {
            r.min.y += gap / 2.0;
        }
        if y + h < 0.999 {
            r.max.y -= gap / 2.0;
        }
        let cr = CornerRadius::from(if i == 0 { s * 0.07 } else { s * 0.055 });
        paint::gradient_rect(p, r, cr, rgb(*color), gradient);
        if i == 0 {
            let pts = [
                (0.10, 0.60),
                (0.32, 0.60),
                (0.42, 0.42),
                (0.54, 0.80),
                (0.66, 0.30),
                (0.76, 0.60),
                (0.92, 0.60),
            ];
            let line: Vec<_> = pts
                .iter()
                .map(|(px, py)| pos2(r.left() + px * r.width(), r.top() + py * r.height()))
                .collect();
            p.add(Shape::line(
                line,
                PathStroke::new((s * 0.035).max(1.2), Color32::WHITE),
            ));
        }
    }
}
