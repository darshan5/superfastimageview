//! Raster drawing primitives operating directly on RGBA8 pixels, in image
//! coordinates (pixel `(i, j)` covers `[i, i+1) x [j, j+1)`).
//!
//! Every function returns the dirty region so the canvas can re-upload only
//! what changed.

use egui::Pos2;
use image::RgbaImage;

/// Half-open pixel rectangle `[x0, x1) x [y0, y1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dirty {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Dirty {
    pub fn union(self, o: Dirty) -> Dirty {
        Dirty {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
}

/// Clamps a float bounding box to the image, returning integer bounds.
fn clamp_box(img: &RgbaImage, x0: f32, y0: f32, x1: f32, y1: f32) -> Option<Dirty> {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let d = Dirty {
        x0: x0.floor().clamp(0.0, w) as u32,
        y0: y0.floor().clamp(0.0, h) as u32,
        x1: x1.ceil().clamp(0.0, w) as u32,
        y1: y1.ceil().clamp(0.0, h) as u32,
    };
    (d.x0 < d.x1 && d.y0 < d.y1).then_some(d)
}

#[inline]
fn put(img: &mut RgbaImage, x: u32, y: u32, c: [u8; 4]) {
    let w = img.width();
    let i = ((y * w + x) * 4) as usize;
    img.as_mut()[i..i + 4].copy_from_slice(&c);
}

/// A round-capped stroke from `a` to `b` with the given diameter.
pub fn segment(img: &mut RgbaImage, a: Pos2, b: Pos2, diameter: f32, c: [u8; 4]) -> Option<Dirty> {
    if diameter <= 1.0 {
        return bresenham(img, a, b, c);
    }
    let r = diameter / 2.0;
    let d = clamp_box(img, a.x.min(b.x) - r, a.y.min(b.y) - r, a.x.max(b.x) + r, a.y.max(b.y) + r)?;
    let ab = b - a;
    let len2 = ab.length_sq();
    let r2 = r * r;
    for y in d.y0..d.y1 {
        for x in d.x0..d.x1 {
            let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
            let t = if len2 > 0.0 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
            if (p - (a + ab * t)).length_sq() <= r2 {
                put(img, x, y, c);
            }
        }
    }
    Some(d)
}

fn bresenham(img: &mut RgbaImage, a: Pos2, b: Pos2, c: [u8; 4]) -> Option<Dirty> {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let (mut x0, mut y0) = (a.x.floor() as i64, a.y.floor() as i64);
    let (x1, y1) = (b.x.floor() as i64, b.y.floor() as i64);
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut dirty: Option<Dirty> = None;
    loop {
        if x0 >= 0 && y0 >= 0 && x0 < w && y0 < h {
            put(img, x0 as u32, y0 as u32, c);
            let p = Dirty { x0: x0 as u32, y0: y0 as u32, x1: x0 as u32 + 1, y1: y0 as u32 + 1 };
            dirty = Some(dirty.map_or(p, |d| d.union(p)));
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
    dirty
}

/// Pixel-snapped inclusive bounds of a drag from `a` to `b`.
fn bounds(a: Pos2, b: Pos2) -> (f32, f32, f32, f32) {
    (
        a.x.min(b.x).floor(),
        a.y.min(b.y).floor(),
        a.x.max(b.x).floor() + 1.0,
        a.y.max(b.y).floor() + 1.0,
    )
}

/// Axis-aligned rectangle; the outline is drawn inside the bounds.
pub fn rect(
    img: &mut RgbaImage,
    a: Pos2,
    b: Pos2,
    thickness: f32,
    outline: Option<[u8; 4]>,
    fill: Option<[u8; 4]>,
) -> Option<Dirty> {
    let (x0, y0, x1, y1) = bounds(a, b);
    let d = clamp_box(img, x0, y0, x1, y1)?;
    let t = thickness.max(1.0).round();
    for y in d.y0..d.y1 {
        for x in d.x0..d.x1 {
            let (px, py) = (x as f32, y as f32);
            let inner = px >= x0 + t && px < x1 - t && py >= y0 + t && py < y1 - t;
            if !inner
                && let Some(c) = outline {
                    put(img, x, y, c);
                    continue;
                }
            if let Some(c) = fill {
                put(img, x, y, c);
            }
        }
    }
    Some(d)
}

/// Ellipse inscribed in the drag bounds; the outline is drawn inside.
pub fn ellipse(
    img: &mut RgbaImage,
    a: Pos2,
    b: Pos2,
    thickness: f32,
    outline: Option<[u8; 4]>,
    fill: Option<[u8; 4]>,
) -> Option<Dirty> {
    let (x0, y0, x1, y1) = bounds(a, b);
    let d = clamp_box(img, x0, y0, x1, y1)?;
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (rx, ry) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
    let t = thickness.max(1.0);
    let (irx, iry) = (rx - t, ry - t);
    for y in d.y0..d.y1 {
        for x in d.x0..d.x1 {
            let (px, py) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            if (px / rx).powi(2) + (py / ry).powi(2) > 1.0 {
                continue;
            }
            let inner = irx > 0.0 && iry > 0.0 && (px / irx).powi(2) + (py / iry).powi(2) <= 1.0;
            if !inner
                && let Some(c) = outline {
                    put(img, x, y, c);
                    continue;
                }
            if let Some(c) = fill {
                put(img, x, y, c);
            }
        }
    }
    Some(d)
}

/// Scanline flood fill of the region connected to `(x, y)` whose colors are
/// within `tolerance` (max per-channel difference) of the seed color.
pub fn flood_fill(img: &mut RgbaImage, x: u32, y: u32, c: [u8; 4], tolerance: u8) -> Option<Dirty> {
    let (w, h) = (img.width(), img.height());
    if x >= w || y >= h {
        return None;
    }
    let seed = img.get_pixel(x, y).0;
    if tolerance == 0 && seed == c {
        return None;
    }
    let matches = |p: &[u8]| {
        p.iter().zip(seed.iter()).all(|(a, b)| a.abs_diff(*b) <= tolerance)
    };
    let mut visited = vec![false; (w * h) as usize];
    let mut stack = vec![(x, y)];
    let mut dirty = Dirty { x0: x, y0: y, x1: x + 1, y1: y + 1 };
    let raw_idx = |x: u32, y: u32| ((y * w + x) * 4) as usize;
    while let Some((sx, sy)) = stack.pop() {
        let row = (sy * w) as usize;
        if visited[row + sx as usize] {
            continue;
        }
        // Expand left and right along the scanline.
        let mut lx = sx;
        while lx > 0 && !visited[row + lx as usize - 1] && matches(&img.as_raw()[raw_idx(lx - 1, sy)..][..4]) {
            lx -= 1;
        }
        let mut rx = sx;
        while rx + 1 < w && !visited[row + rx as usize + 1] && matches(&img.as_raw()[raw_idx(rx + 1, sy)..][..4]) {
            rx += 1;
        }
        for px in lx..=rx {
            visited[row + px as usize] = true;
        }
        // Queue matching runs in the rows above and below (before painting, so
        // comparisons still see the original colors).
        for ny in [sy.wrapping_sub(1), sy + 1] {
            if ny >= h {
                continue;
            }
            let nrow = (ny * w) as usize;
            let mut in_run = false;
            for px in lx..=rx {
                let ok = !visited[nrow + px as usize] && matches(&img.as_raw()[raw_idx(px, ny)..][..4]);
                if ok && !in_run {
                    stack.push((px, ny));
                }
                in_run = ok;
            }
        }
        for px in lx..=rx {
            put(img, px, sy, c);
        }
        dirty = dirty.union(Dirty { x0: lx, y0: sy, x1: rx + 1, y1: sy + 1 });
    }
    Some(dirty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_stops_at_border() {
        let mut img = RgbaImage::from_pixel(10, 10, image::Rgba([255, 255, 255, 255]));
        rect(&mut img, Pos2::new(2.0, 2.0), Pos2::new(7.0, 7.0), 1.0, Some([0, 0, 0, 255]), None);
        let d = flood_fill(&mut img, 4, 4, [255, 0, 0, 255], 0).unwrap();
        assert_eq!(d, Dirty { x0: 3, y0: 3, x1: 7, y1: 7 });
        assert_eq!(img.get_pixel(0, 0).0, [255, 255, 255, 255]);
        assert_eq!(img.get_pixel(5, 5).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(2, 2).0, [0, 0, 0, 255]);
    }

    #[test]
    fn fill_with_tolerance_terminates() {
        let mut img = RgbaImage::from_pixel(50, 50, image::Rgba([100, 100, 100, 255]));
        let d = flood_fill(&mut img, 0, 0, [105, 100, 100, 255], 20).unwrap();
        assert_eq!(d, Dirty { x0: 0, y0: 0, x1: 50, y1: 50 });
    }

    #[test]
    fn thick_segment_covers_endpoints() {
        let mut img = RgbaImage::new(20, 20);
        segment(&mut img, Pos2::new(3.5, 3.5), Pos2::new(15.5, 3.5), 3.0, [1, 2, 3, 255]);
        assert_eq!(img.get_pixel(3, 3).0, [1, 2, 3, 255]);
        assert_eq!(img.get_pixel(15, 4).0, [1, 2, 3, 255]);
        assert_eq!(img.get_pixel(9, 8).0, [0, 0, 0, 0]);
    }
}
