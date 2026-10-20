//! Squarified treemap layout (Bruls, Huizing & van Wijk, 2000): tiles that
//! stay close to square, so labels fit and sizes are easy to compare.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn area(&self) -> f32 {
        self.w.max(0.0) * self.h.max(0.0)
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }

    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(
            self.x + d,
            self.y + d,
            (self.w - 2.0 * d).max(0.0),
            (self.h - 2.0 * d).max(0.0),
        )
    }
}

/// Lay out `weights` inside `bounds`. Returns one rectangle per weight, in
/// input order; zero or negative weights get an empty rectangle.
pub fn squarify(weights: &[f64], bounds: Rect) -> Vec<Rect> {
    let mut out = vec![Rect::default(); weights.len()];
    if bounds.w <= 0.0 || bounds.h <= 0.0 {
        return out;
    }
    let mut order: Vec<usize> = (0..weights.len())
        .filter(|&i| weights[i] > 0.0 && weights[i].is_finite())
        .collect();
    if order.is_empty() {
        return out;
    }
    order.sort_by(|&a, &b| weights[b].total_cmp(&weights[a]));
    let total: f64 = order.iter().map(|&i| weights[i]).sum();
    let scale = (bounds.w as f64 * bounds.h as f64) / total;
    let areas: Vec<f64> = order.iter().map(|&i| weights[i] * scale).collect();

    let (mut x, mut y, mut w, mut h) = (
        bounds.x as f64,
        bounds.y as f64,
        bounds.w as f64,
        bounds.h as f64,
    );
    let worst = |sum: f64, min: f64, max: f64, side: f64| {
        let s2 = sum * sum;
        let w2 = side * side;
        (w2 * max / s2).max(s2 / (w2 * min))
    };

    let n = areas.len();
    let mut start = 0;
    while start < n {
        let side = w.min(h).max(f64::EPSILON);
        let (mut sum, mut min, mut max) = (areas[start], areas[start], areas[start]);
        let mut current = worst(sum, min, max, side);
        let mut end = start + 1;
        while end < n {
            let a = areas[end];
            let next = worst(sum + a, min.min(a), max.max(a), side);
            if next > current {
                break;
            }
            sum += a;
            min = min.min(a);
            max = max.max(a);
            current = next;
            end += 1;
        }
        let last_row = end == n;
        if w >= h {
            // Column on the left.
            let col = if last_row { w } else { (sum / h).min(w) };
            let mut cy = y;
            for k in start..end {
                let th = if k + 1 == end {
                    (y + h) - cy
                } else {
                    areas[k] / col
                };
                out[order[k]] = Rect::new(x as f32, cy as f32, col as f32, th.max(0.0) as f32);
                cy += th;
            }
            x += col;
            w -= col;
        } else {
            // Row along the top.
            let row = if last_row { h } else { (sum / w).min(h) };
            let mut cx = x;
            for k in start..end {
                let tw = if k + 1 == end {
                    (x + w) - cx
                } else {
                    areas[k] / row
                };
                out[order[k]] = Rect::new(cx as f32, y as f32, tw.max(0.0) as f32, row as f32);
                cx += tw;
            }
            y += row;
            h -= row;
        }
        start = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn areas_are_proportional_and_tile_the_bounds() {
        let weights = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0, 0.0];
        let bounds = Rect::new(10.0, 20.0, 600.0, 400.0);
        let rects = squarify(&weights, bounds);
        let total: f64 = weights.iter().sum();
        let mut covered = 0.0;
        for (wgt, r) in weights.iter().zip(&rects) {
            let expected = wgt / total * (600.0 * 400.0);
            assert!(
                (r.area() as f64 - expected).abs() < 1.0,
                "{r:?} vs {expected}"
            );
            if *wgt > 0.0 {
                assert!(r.x >= 10.0 - 0.01 && r.y >= 20.0 - 0.01);
                assert!(r.x + r.w <= 610.01 && r.y + r.h <= 420.01);
            }
            covered += r.area() as f64;
        }
        assert!((covered - 240_000.0).abs() < 2.0);
        assert_eq!(rects[7], Rect::default());
        // Squarified tiles should be reasonably square.
        let worst = rects[..7]
            .iter()
            .map(|r| (r.w / r.h).max(r.h / r.w))
            .fold(0.0f32, f32::max);
        assert!(worst < 3.5, "aspect {worst}");
    }

    #[test]
    fn degenerate_inputs() {
        assert!(squarify(&[], Rect::new(0.0, 0.0, 10.0, 10.0)).is_empty());
        assert_eq!(
            squarify(&[1.0], Rect::new(0.0, 0.0, 0.0, 10.0)),
            vec![Rect::default()]
        );
        let one = squarify(&[5.0], Rect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(one[0], Rect::new(0.0, 0.0, 10.0, 10.0));
    }
}
