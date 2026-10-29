//! Procedurally drawn app icon, shared by the runtime window icon and the
//! `gen_icon` example that writes the `.ico` / `.png` assets for packaging.

/// Returns `size * size` RGBA8 pixels: a rounded blue tile with a picture glyph
/// (sun + mountains) and a lightning bolt.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut out = vec![0u8; (size * size * 4) as usize];
    let radius = s * 0.2;
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let (u, v) = (px / s, py / s);
            let i = ((y * size + x) * 4) as usize;

            // Rounded-square coverage with a 1px soft edge.
            let dx = (px - s / 2.0).abs() - (s / 2.0 - radius);
            let dy = (py - s / 2.0).abs() - (s / 2.0 - radius);
            let dist = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() - radius;
            let cov = (0.5 - dist).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }

            // Vertical gradient background.
            let mut c = [
                lerp(40.0, 20.0, v),
                lerp(120.0, 70.0, v),
                lerp(230.0, 170.0, v),
            ];

            // Sun.
            let sun = ((u - 0.68).powi(2) + (v - 0.32).powi(2)).sqrt();
            if sun < 0.1 {
                c = [255.0, 210.0, 80.0];
            }
            // Mountains.
            let m1 = 0.78 - (0.42 - (u - 0.35).abs()) * 1.1;
            let m2 = 0.80 - (0.30 - (u - 0.72).abs()) * 1.0;
            if v > m1.min(m2) && v < 0.86 && u > 0.12 && u < 0.88 {
                c = if v > m1 { [235.0, 245.0, 255.0] } else { [190.0, 215.0, 245.0] };
            }
            // Lightning bolt (speed!) in the lower-left corner.
            if in_bolt(u, v) {
                c = [255.0, 200.0, 40.0];
            }

            out[i] = c[0] as u8;
            out[i + 1] = c[1] as u8;
            out[i + 2] = c[2] as u8;
            out[i + 3] = (cov * 255.0) as u8;
        }
    }
    out
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn in_bolt(u: f32, v: f32) -> bool {
    // Two triangles forming a stylised bolt.
    let tri = |p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        let s = |p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)| {
            (p1.0 - p3.0) * (p2.1 - p3.1) - (p2.0 - p3.0) * (p1.1 - p3.1)
        };
        let d1 = s(p, a, b);
        let d2 = s(p, b, c);
        let d3 = s(p, c, a);
        let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        !(neg && pos)
    };
    let p = (u, v);
    tri(p, (0.30, 0.50), (0.12, 0.74), (0.26, 0.72))
        || tri(p, (0.26, 0.66), (0.33, 0.66), (0.14, 0.92))
}
