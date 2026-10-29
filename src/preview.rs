//! Windows-only instant preview: paints the decoded image into the window
//! with plain GDI while the OpenGL context is still being created, so the
//! first image is on screen without waiting for the GPU driver.

use image::RgbaImage;
use rayon::prelude::*;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

/// Approximate heights of the UI chrome (menu + toolbar, status bar) in points,
/// so the preview lands where the real UI will draw the image.
pub const TOP_CHROME: f32 = 49.0;
pub const BOTTOM_CHROME: f32 = 25.0;
const BG: [u8; 3] = [24, 24, 24];
/// egui's dark panel color, so the chrome bands match what comes next.
const PANEL: [u8; 3] = [27, 27, 27];

/// Paints the background and, if given, the image fitted (shrink-only) into
/// the area between the chrome bars.
pub fn paint(hwnd: HWND, img: Option<&RgbaImage>, scale: f32) {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let (cw, ch) = (rc.right - rc.left, rc.bottom - rc.top);
        if cw <= 0 || ch <= 0 {
            return;
        }
        let hdc = GetDC(hwnd);
        let top = (TOP_CHROME * scale).round() as i32;
        let bottom = (BOTTOM_CHROME * scale).round() as i32;
        let fill = |r: RECT, c: [u8; 3]| {
            let brush = CreateSolidBrush(rgb(c));
            FillRect(hdc, &r, brush);
            DeleteObject(brush as _);
        };
        fill(RECT { bottom: top, ..rc }, PANEL);
        fill(RECT { top, bottom: ch - bottom, ..rc }, BG);
        fill(RECT { top: ch - bottom, ..rc }, PANEL);

        if let Some(img) = img {
            let (aw, ah) = (cw, (ch - top - bottom).max(1));
            let (iw, ih) = img.dimensions();
            let s = (aw as f32 / iw as f32).min(ah as f32 / ih as f32).min(1.0);
            let dw = ((iw as f32 * s).round() as i32).max(1);
            let dh = ((ih as f32 * s).round() as i32).max(1);
            let bgra = downscale_bgra(img, dw as u32, dh as u32);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: dw,
                    biHeight: -dh, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..std::mem::zeroed()
                },
                ..std::mem::zeroed()
            };
            let x = (aw - dw) / 2;
            let y = top + (ah - dh) / 2;
            StretchDIBits(hdc, x, y, dw, dh, 0, 0, dw, dh, bgra.as_ptr().cast(), &bmi, DIB_RGB_COLORS, SRCCOPY);
        }
        ReleaseDC(hwnd, hdc);
    }
}

fn rgb(c: [u8; 3]) -> u32 {
    c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16
}

/// Fast 2x2-tap downscale to BGRA, composited over the background.
fn downscale_bgra(img: &RgbaImage, dw: u32, dh: u32) -> Vec<u8> {
    let (iw, ih) = img.dimensions();
    let raw = img.as_raw();
    let sx = iw as f32 / dw as f32;
    let sy = ih as f32 / dh as f32;
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    out.par_chunks_mut((dw * 4) as usize).enumerate().for_each(|(y, row)| {
        let ys = [((y as f32 + 0.25) * sy) as u32, ((y as f32 + 0.75) * sy) as u32].map(|v| v.min(ih - 1));
        for x in 0..dw {
            let xs = [((x as f32 + 0.25) * sx) as u32, ((x as f32 + 0.75) * sx) as u32].map(|v| v.min(iw - 1));
            let mut acc = [0u32; 4];
            for yy in ys {
                for xx in xs {
                    let i = ((yy * iw + xx) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += raw[i + c] as u32;
                    }
                }
            }
            let a = acc[3] / 4;
            let o = (x * 4) as usize;
            for c in 0..3 {
                let v = acc[c] / 4;
                row[o + 2 - c] = ((v * a + BG[c] as u32 * (255 - a)) / 255) as u8;
            }
            row[o + 3] = 255;
        }
    });
    out
}
