//! GPU textures for the current image. Large images are split into tiles so
//! they fit the GPU's maximum texture size, and edits re-upload only the
//! changed region.

use egui::{Color32, ColorImage, Painter, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use image::RgbaImage;
use rayon::prelude::*;

const MAX_TILE: usize = 8192;

struct Tile {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    tex: TextureHandle,
}

#[derive(Default)]
pub struct Canvas {
    tiles: Vec<Tile>,
    pub pixelated: bool,
}

impl Canvas {
    pub fn options(&self) -> TextureOptions {
        TextureOptions {
            magnification: if self.pixelated {
                egui::TextureFilter::Nearest
            } else {
                egui::TextureFilter::Linear
            },
            minification: egui::TextureFilter::Linear,
            wrap_mode: egui::TextureWrapMode::ClampToEdge,
            mipmap_mode: Some(egui::TextureFilter::Linear),
        }
    }

    pub fn clear(&mut self) {
        self.tiles.clear();
    }

    pub fn set_image(&mut self, ctx: &egui::Context, img: &RgbaImage, max_side: usize) {
        self.tiles.clear();
        let tile = max_side.clamp(256, MAX_TILE);
        let (w, h) = (img.width() as usize, img.height() as usize);
        let opts = self.options();
        for ty in (0..h).step_by(tile) {
            for tx in (0..w).step_by(tile) {
                let tw = tile.min(w - tx);
                let th = tile.min(h - ty);
                let ci = region(img, tx, ty, tw, th);
                let tex = ctx.load_texture(format!("img-{tx}-{ty}"), ci, opts);
                self.tiles.push(Tile { x: tx, y: ty, w: tw, h: th, tex });
            }
        }
    }

    /// Re-uploads pixels in `[x0, x1) x [y0, y1)`.
    pub fn update_region(&mut self, img: &RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) {
        let opts = self.options();
        let (x0, y0, x1, y1) = (x0 as usize, y0 as usize, x1 as usize, y1 as usize);
        for t in &mut self.tiles {
            let ix0 = x0.max(t.x);
            let iy0 = y0.max(t.y);
            let ix1 = x1.min(t.x + t.w);
            let iy1 = y1.min(t.y + t.h);
            if ix0 >= ix1 || iy0 >= iy1 {
                continue;
            }
            let ci = region(img, ix0, iy0, ix1 - ix0, iy1 - iy0);
            t.tex.set_partial([ix0 - t.x, iy0 - t.y], ci, opts);
        }
    }

    /// Draws the image so that its full extent maps onto `dest` (screen space).
    pub fn paint(&self, painter: &Painter, dest: Rect, img_w: u32, img_h: u32) {
        let sx = dest.width() / img_w as f32;
        let sy = dest.height() / img_h as f32;
        let clip = painter.clip_rect();
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        for t in &self.tiles {
            let r = Rect::from_min_size(
                dest.min + Vec2::new(t.x as f32 * sx, t.y as f32 * sy),
                Vec2::new(t.w as f32 * sx, t.h as f32 * sy),
            );
            if r.intersects(clip) {
                painter.image(t.tex.id(), r, uv, Color32::WHITE);
            }
        }
    }
}

/// Copies a sub-rectangle into a premultiplied `ColorImage`, in parallel.
fn region(img: &RgbaImage, x: usize, y: usize, w: usize, h: usize) -> ColorImage {
    let stride = img.width() as usize * 4;
    let raw = img.as_raw();
    let mut pixels = vec![Color32::TRANSPARENT; w * h];
    pixels.par_chunks_mut(w).enumerate().for_each(|(row, out)| {
        let start = (y + row) * stride + x * 4;
        let src = &raw[start..start + w * 4];
        for (o, p) in out.iter_mut().zip(src.chunks_exact(4)) {
            *o = if p[3] == 255 {
                Color32::from_rgb(p[0], p[1], p[2])
            } else {
                Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3])
            };
        }
    });
    ColorImage::new([w, h], pixels)
}

/// A tiny repeating checkerboard texture drawn under transparent images.
pub fn checker_texture(ctx: &egui::Context) -> TextureHandle {
    let a = Color32::from_gray(60);
    let b = Color32::from_gray(80);
    let img = ColorImage::new([2, 2], vec![a, b, b, a]);
    ctx.load_texture(
        "checker",
        img,
        TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Nearest,
            wrap_mode: egui::TextureWrapMode::Repeat,
            mipmap_mode: None,
        },
    )
}
