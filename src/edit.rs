//! Whole-image operations: rotate, flip, invert, grayscale, crop, resize.

use fast_image_resize::{FilterType, ResizeAlg, ResizeOptions, Resizer};
use image::{RgbaImage, imageops};
use rayon::prelude::*;

pub fn rotate_cw(img: &RgbaImage) -> RgbaImage {
    imageops::rotate90(img)
}

pub fn rotate_ccw(img: &RgbaImage) -> RgbaImage {
    imageops::rotate270(img)
}

pub fn rotate_180(img: &mut RgbaImage) {
    imageops::rotate180_in_place(img);
}

pub fn flip_h(img: &mut RgbaImage) {
    imageops::flip_horizontal_in_place(img);
}

pub fn flip_v(img: &mut RgbaImage) {
    imageops::flip_vertical_in_place(img);
}

pub fn invert(img: &mut RgbaImage) {
    img.par_chunks_mut(4).for_each(|p| {
        p[0] = 255 - p[0];
        p[1] = 255 - p[1];
        p[2] = 255 - p[2];
    });
}

pub fn grayscale(img: &mut RgbaImage) {
    img.par_chunks_mut(4).for_each(|p| {
        let l = (p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19) >> 8;
        p[0] = l as u8;
        p[1] = l as u8;
        p[2] = l as u8;
    });
}

pub fn crop(img: &RgbaImage, x: u32, y: u32, w: u32, h: u32) -> RgbaImage {
    imageops::crop_imm(img, x, y, w, h).to_image()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResizeFilter {
    Nearest,
    Bilinear,
    Bicubic,
    Lanczos3,
}

impl ResizeFilter {
    pub const ALL: [ResizeFilter; 4] = [
        ResizeFilter::Nearest,
        ResizeFilter::Bilinear,
        ResizeFilter::Bicubic,
        ResizeFilter::Lanczos3,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ResizeFilter::Nearest => "Nearest (pixel art)",
            ResizeFilter::Bilinear => "Bilinear",
            ResizeFilter::Bicubic => "Bicubic",
            ResizeFilter::Lanczos3 => "Lanczos3 (sharpest)",
        }
    }
}

/// SIMD-accelerated resize (alpha-correct: premultiplies internally).
pub fn resize(img: &RgbaImage, w: u32, h: u32, filter: ResizeFilter) -> RgbaImage {
    let w = w.max(1);
    let h = h.max(1);
    let mut dst = RgbaImage::new(w, h);
    let alg = match filter {
        ResizeFilter::Nearest => ResizeAlg::Nearest,
        ResizeFilter::Bilinear => ResizeAlg::Convolution(FilterType::Bilinear),
        ResizeFilter::Bicubic => ResizeAlg::Convolution(FilterType::CatmullRom),
        ResizeFilter::Lanczos3 => ResizeAlg::Convolution(FilterType::Lanczos3),
    };
    let opts = ResizeOptions::new().resize_alg(alg);
    if Resizer::new().resize(img, &mut dst, &opts).is_err() {
        // Fall back to the image crate if fast_image_resize rejects the input.
        let f = match filter {
            ResizeFilter::Nearest => imageops::FilterType::Nearest,
            ResizeFilter::Bilinear => imageops::FilterType::Triangle,
            ResizeFilter::Bicubic => imageops::FilterType::CatmullRom,
            ResizeFilter::Lanczos3 => imageops::FilterType::Lanczos3,
        };
        dst = imageops::resize(img, w, h, f);
    }
    dst
}
