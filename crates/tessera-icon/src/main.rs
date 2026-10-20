//! Renders Tessera's artwork from code: the app icon (as an `.iconset`
//! folder for `iconutil`), the menu bar template glyph and a README logo.
//!
//! `cargo run -p tessera-icon -- <assets dir>`
//!
//! The mark is a small treemap — a mosaic of tiles, like the disk map — with
//! a pulse line running through the largest tile for live activity.

use std::path::{Path, PathBuf};

use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, Path as SkPath,
    PathBuilder, Pixmap, Point, Shader, SpreadMode, Stroke, Transform,
};

/// Tile layout on a unit square: (x, y, w, h, colour top, colour bottom).
const TILES: [(f32, f32, f32, f32, u32, u32); 4] = [
    (0.0, 0.0, 0.56, 1.0, 0x8B6CFF, 0x4F7BFF), // big: violet → blue
    (0.56, 0.0, 0.44, 0.52, 0x34E0C4, 0x10A8A0), // top right: teal
    (0.56, 0.52, 0.24, 0.48, 0xFF7AB8, 0xE94B8E), // bottom: pink
    (0.80, 0.52, 0.20, 0.48, 0xFFD15C, 0xF59E0B), // bottom: amber
];

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "assets".into()));
    std::fs::create_dir_all(&out).unwrap();

    let iconset = out.join("AppIcon.iconset");
    std::fs::create_dir_all(&iconset).unwrap();
    for (name, px) in [
        ("icon_16x16.png", 16),
        ("icon_16x16@2x.png", 32),
        ("icon_32x32.png", 32),
        ("icon_32x32@2x.png", 64),
        ("icon_128x128.png", 128),
        ("icon_128x128@2x.png", 256),
        ("icon_256x256.png", 256),
        ("icon_256x256@2x.png", 512),
        ("icon_512x512.png", 512),
        ("icon_512x512@2x.png", 1024),
    ] {
        app_icon(px).save_png(iconset.join(name)).unwrap();
    }
    app_icon(512).save_png(out.join("logo.png")).unwrap();
    menubar_glyph(18).save_png(out.join("menubar.png")).unwrap();
    menubar_glyph(36)
        .save_png(out.join("menubar@2x.png"))
        .unwrap();
    println!("wrote artwork to {}", out.display());
    let _ = Path::new(&out);
}

fn rgb(hex: u32, alpha: f32) -> Color {
    Color::from_rgba(
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
        alpha,
    )
    .unwrap()
}

fn vertical_gradient(y0: f32, y1: f32, top: Color, bottom: Color) -> Shader<'static> {
    LinearGradient::new(
        Point::from_xy(0.0, y0),
        Point::from_xy(0.0, y1),
        vec![GradientStop::new(0.0, top), GradientStop::new(1.0, bottom)],
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap()
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> SkPath {
    let r = r.min(w / 2.0).min(h / 2.0);
    let k = 0.552_284_8 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish().unwrap()
}

/// Apple-style continuous-corner "squircle" (a superellipse).
fn squircle(cx: f32, cy: f32, half: f32) -> SkPath {
    let n = 5.0f32;
    let mut pb = PathBuilder::new();
    let steps = 360;
    for i in 0..=steps {
        let t = i as f32 / steps as f32 * std::f32::consts::TAU;
        let (s, c) = t.sin_cos();
        let x = cx + half * c.signum() * c.abs().powf(2.0 / n);
        let y = cy + half * s.signum() * s.abs().powf(2.0 / n);
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.close();
    pb.finish().unwrap()
}

fn fill(pixmap: &mut Pixmap, path: &SkPath, shader: Shader<'static>) {
    let paint = Paint {
        shader,
        anti_alias: true,
        ..Paint::default()
    };
    pixmap.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
}

/// Separable box blur, three passes ≈ gaussian. Used for the drop shadow.
fn blur(pixmap: &mut Pixmap, radius: usize) {
    let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
    let data = pixmap.data_mut();
    let mut tmp = vec![0f32; w * h * 4];
    for _ in 0..3 {
        for horizontal in [true, false] {
            let (outer, inner) = if horizontal { (h, w) } else { (w, h) };
            for o in 0..outer {
                let idx = |i: usize| {
                    if horizontal {
                        (o * w + i) * 4
                    } else {
                        (i * w + o) * 4
                    }
                };
                let mut acc = [0f32; 4];
                let win = (2 * radius + 1) as f32;
                for i in 0..=radius.min(inner - 1) {
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += data[idx(i) + c] as f32;
                    }
                }
                for i in 0..inner {
                    for c in 0..4 {
                        tmp[idx(i) + c] = acc[c] / win;
                    }
                    let add = i + radius + 1;
                    if add < inner {
                        for (c, a) in acc.iter_mut().enumerate() {
                            *a += data[idx(add) + c] as f32;
                        }
                    }
                    if i >= radius {
                        for (c, a) in acc.iter_mut().enumerate() {
                            *a -= data[idx(i - radius) + c] as f32;
                        }
                    }
                }
            }
            for (d, t) in data.iter_mut().zip(&tmp) {
                *d = t.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn app_icon(px: u32) -> Pixmap {
    // Draw at 1024 and downsample for crisp small sizes.
    let size = 1024.0f32;
    let mut pm = Pixmap::new(1024, 1024).unwrap();
    let body = 824.0;
    let (cx, cy) = (size / 2.0, size / 2.0 - 6.0);

    // Drop shadow.
    let mut shadow = Pixmap::new(1024, 1024).unwrap();
    fill(
        &mut shadow,
        &squircle(cx, cy + 14.0, body / 2.0),
        Shader::SolidColor(rgb(0x000000, 0.42)),
    );
    blur(&mut shadow, 14);
    pm.draw_pixmap(
        0,
        0,
        shadow.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        Transform::identity(),
        None,
    );

    // Body: deep midnight gradient.
    let squircle_path = squircle(cx, cy, body / 2.0);
    fill(
        &mut pm,
        &squircle_path,
        vertical_gradient(
            cy - body / 2.0,
            cy + body / 2.0,
            rgb(0x1E2440, 1.0),
            rgb(0x0A0D18, 1.0),
        ),
    );
    // Soft sheen on the upper half.
    fill(
        &mut pm,
        &squircle_path,
        vertical_gradient(cy - body / 2.0, cy, rgb(0xFFFFFF, 0.10), rgb(0xFFFFFF, 0.0)),
    );

    // Mosaic tiles.
    let inset = 150.0;
    let area = size - 2.0 * inset;
    let gap = 26.0;
    let ox = inset;
    let oy = inset - 6.0;
    for (i, (x, y, w, h, top, bottom)) in TILES.iter().enumerate() {
        let rx = ox + x * area + if *x > 0.0 { gap / 2.0 } else { 0.0 };
        let ry = oy + y * area + if *y > 0.0 { gap / 2.0 } else { 0.0 };
        let rw = w * area
            - if *x > 0.0 { gap / 2.0 } else { 0.0 }
            - if x + w < 0.999 { gap / 2.0 } else { 0.0 };
        let rh = h * area
            - if *y > 0.0 { gap / 2.0 } else { 0.0 }
            - if y + h < 0.999 { gap / 2.0 } else { 0.0 };
        let radius = if i == 0 { 54.0 } else { 40.0 };
        let path = rounded_rect(rx, ry, rw, rh, radius);
        fill(
            &mut pm,
            &path,
            vertical_gradient(ry, ry + rh, rgb(*top, 1.0), rgb(*bottom, 1.0)),
        );
        // Glassy top highlight.
        fill(
            &mut pm,
            &path,
            vertical_gradient(ry, ry + rh * 0.55, rgb(0xFFFFFF, 0.28), rgb(0xFFFFFF, 0.0)),
        );

        if i == 0 {
            // Pulse line across the big tile.
            let pts = [
                (0.08, 0.60),
                (0.30, 0.60),
                (0.40, 0.42),
                (0.52, 0.80),
                (0.64, 0.30),
                (0.74, 0.60),
                (0.92, 0.60),
            ];
            let mut pb = PathBuilder::new();
            for (k, (px_, py_)) in pts.iter().enumerate() {
                let (x, y) = (rx + px_ * rw, ry + py_ * rh);
                if k == 0 {
                    pb.move_to(x, y)
                } else {
                    pb.line_to(x, y)
                }
            }
            let line = pb.finish().unwrap();
            let stroke = Stroke {
                width: 30.0,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Stroke::default()
            };
            let paint = Paint {
                shader: Shader::SolidColor(rgb(0xFFFFFF, 0.95)),
                anti_alias: true,
                ..Paint::default()
            };
            pm.stroke_path(&line, &paint, &stroke, Transform::identity(), None);
        }
    }

    // Hairline rim for definition on light backgrounds.
    let rim = Stroke {
        width: 3.0,
        ..Stroke::default()
    };
    let paint = Paint {
        shader: Shader::SolidColor(rgb(0xFFFFFF, 0.10)),
        anti_alias: true,
        ..Paint::default()
    };
    pm.stroke_path(&squircle_path, &paint, &rim, Transform::identity(), None);

    if px == 1024 { pm } else { downsample(&pm, px) }
}

/// Monochrome glyph for the menu bar (used as a template image, so only the
/// alpha channel matters and macOS tints it for light and dark menu bars).
fn menubar_glyph(px: u32) -> Pixmap {
    let s = px as f32 / 18.0; // design on an 18 pt grid
    let mut pm = Pixmap::new(px, px).unwrap();
    let black = Shader::SolidColor(rgb(0x000000, 1.0));
    let (ox, oy, area) = (2.0 * s, 2.0 * s, 14.0 * s);
    let gap = 1.6 * s;
    let tiles = [
        (0.0, 0.0, 0.56, 1.0),
        (0.56, 0.0, 0.44, 0.5),
        (0.56, 0.5, 0.22, 0.5),
        (0.78, 0.5, 0.22, 0.5),
    ];
    for (i, (x, y, w, h)) in tiles.iter().enumerate() {
        let rx = ox + x * area + if *x > 0.0 { gap / 2.0 } else { 0.0 };
        let ry = oy + y * area + if *y > 0.0 { gap / 2.0 } else { 0.0 };
        let rw = w * area
            - if *x > 0.0 { gap / 2.0 } else { 0.0 }
            - if x + w < 0.999 { gap / 2.0 } else { 0.0 };
        let rh = h * area
            - if *y > 0.0 { gap / 2.0 } else { 0.0 }
            - if y + h < 0.999 { gap / 2.0 } else { 0.0 };
        let path = rounded_rect(rx, ry, rw, rh, if i == 0 { 2.2 * s } else { 1.6 * s });
        fill(&mut pm, &path, black.clone());
        if i == 0 {
            // Pulse line cut out of the big tile.
            let pts = [
                (0.12, 0.58),
                (0.34, 0.58),
                (0.46, 0.38),
                (0.60, 0.78),
                (0.74, 0.58),
                (0.88, 0.58),
            ];
            let mut pb = PathBuilder::new();
            for (k, (px_, py_)) in pts.iter().enumerate() {
                let (x, y) = (rx + px_ * rw, ry + py_ * rh);
                if k == 0 {
                    pb.move_to(x, y)
                } else {
                    pb.line_to(x, y)
                }
            }
            let line = pb.finish().unwrap();
            let stroke = Stroke {
                width: 1.25 * s,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Stroke::default()
            };
            let paint = Paint {
                shader: Shader::SolidColor(rgb(0x000000, 1.0)),
                anti_alias: true,
                blend_mode: tiny_skia::BlendMode::Clear,
                ..Paint::default()
            };
            pm.stroke_path(&line, &paint, &stroke, Transform::identity(), None);
        }
    }
    pm
}

/// Area-average downsampling (premultiplied, so edges stay clean).
fn downsample(src: &Pixmap, px: u32) -> Pixmap {
    let mut out = Pixmap::new(px, px).unwrap();
    let factor = src.width() as f32 / px as f32;
    let sd = src.data();
    let sw = src.width() as usize;
    let od = out.data_mut();
    for oy in 0..px as usize {
        for ox in 0..px as usize {
            let x0 = (ox as f32 * factor) as usize;
            let y0 = (oy as f32 * factor) as usize;
            let x1 = (((ox + 1) as f32 * factor) as usize).max(x0 + 1);
            let y1 = (((oy + 1) as f32 * factor) as usize).max(y0 + 1);
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * sw + x) * 4;
                    for c in 0..4 {
                        acc[c] += sd[i + c] as u32;
                    }
                    n += 1;
                }
            }
            let o = (oy * px as usize + ox) * 4;
            for c in 0..4 {
                od[o + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    out
}
