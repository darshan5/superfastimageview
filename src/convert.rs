//! Saving to the supported output formats, and multi-threaded batch conversion.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use image::codecs::{
    bmp::BmpEncoder, gif::GifEncoder, ico::IcoEncoder, jpeg::JpegEncoder, png, pnm::PnmEncoder,
    qoi::QoiEncoder, tga::TgaEncoder, tiff::TiffEncoder, webp::WebPEncoder,
};
use image::{ImageEncoder, RgbImage, RgbaImage};
use rayon::prelude::*;

use crate::edit::{self, ResizeFilter};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutFormat {
    Jpeg,
    Png,
    Webp,
    Bmp,
    Gif,
    Tiff,
    Tga,
    Qoi,
    Ico,
    Pnm,
}

impl OutFormat {
    pub const ALL: [OutFormat; 10] = [
        OutFormat::Jpeg,
        OutFormat::Png,
        OutFormat::Webp,
        OutFormat::Bmp,
        OutFormat::Gif,
        OutFormat::Tiff,
        OutFormat::Tga,
        OutFormat::Qoi,
        OutFormat::Ico,
        OutFormat::Pnm,
    ];

    pub fn ext(self) -> &'static str {
        match self {
            OutFormat::Jpeg => "jpg",
            OutFormat::Png => "png",
            OutFormat::Webp => "webp",
            OutFormat::Bmp => "bmp",
            OutFormat::Gif => "gif",
            OutFormat::Tiff => "tif",
            OutFormat::Tga => "tga",
            OutFormat::Qoi => "qoi",
            OutFormat::Ico => "ico",
            OutFormat::Pnm => "ppm",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OutFormat::Jpeg => "JPEG",
            OutFormat::Png => "PNG",
            OutFormat::Webp => "WebP (lossless)",
            OutFormat::Bmp => "BMP",
            OutFormat::Gif => "GIF",
            OutFormat::Tiff => "TIFF",
            OutFormat::Tga => "TGA",
            OutFormat::Qoi => "QOI",
            OutFormat::Ico => "ICO (max 256px)",
            OutFormat::Pnm => "PPM",
        }
    }

    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            OutFormat::Jpeg => &["jpg", "jpeg", "jpe", "jfif"],
            OutFormat::Png => &["png"],
            OutFormat::Webp => &["webp"],
            OutFormat::Bmp => &["bmp"],
            OutFormat::Gif => &["gif"],
            OutFormat::Tiff => &["tif", "tiff"],
            OutFormat::Tga => &["tga"],
            OutFormat::Qoi => &["qoi"],
            OutFormat::Ico => &["ico"],
            OutFormat::Pnm => &["ppm", "pnm"],
        }
    }

    pub fn from_path(path: &Path) -> Option<OutFormat> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Self::ALL.into_iter().find(|f| f.extensions().contains(&ext.as_str()))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PngLevel {
    Fast,
    Default,
    Best,
}

#[derive(Clone, Copy, Debug)]
pub struct SaveOptions {
    pub jpeg_quality: u8,
    pub png_level: PngLevel,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self { jpeg_quality: 90, png_level: PngLevel::Fast }
    }
}

/// Composites onto white for formats without alpha.
fn flatten(img: &RgbaImage) -> RgbImage {
    let mut out = RgbImage::new(img.width(), img.height());
    out.as_mut()
        .par_chunks_mut(3)
        .zip(img.as_raw().par_chunks(4))
        .for_each(|(o, p)| {
            let a = p[3] as u32;
            for c in 0..3 {
                o[c] = ((p[c] as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
            }
        });
    out
}

/// Encodes `img` to `path`. Writes to a temporary file first and renames it,
/// so a failed save never destroys the original.
pub fn save(img: &RgbaImage, path: &Path, fmt: OutFormat, opts: &SaveOptions) -> Result<(), String> {
    let tmp = path.with_extension(format!("{}.sfiv-tmp", fmt.ext()));
    let result = (|| -> Result<(), String> {
        let file = File::create(&tmp).map_err(|e| e.to_string())?;
        let mut w = BufWriter::with_capacity(1 << 20, file);
        encode(img, &mut w, fmt, opts).map_err(|e| e.to_string())?;
        w.flush().map_err(|e| e.to_string())?;
        Ok(())
    })();
    match result {
        Ok(()) => std::fs::rename(&tmp, path).map_err(|e| e.to_string()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn encode<W: Write + std::io::Seek>(
    img: &RgbaImage,
    w: &mut W,
    fmt: OutFormat,
    opts: &SaveOptions,
) -> image::ImageResult<()> {
    use image::ExtendedColorType::{Rgb8, Rgba8};
    let (iw, ih) = img.dimensions();
    match fmt {
        OutFormat::Jpeg => {
            let rgb = flatten(img);
            JpegEncoder::new_with_quality(w, opts.jpeg_quality.clamp(1, 100))
                .write_image(rgb.as_raw(), iw, ih, Rgb8)
        }
        OutFormat::Png => {
            let level = match opts.png_level {
                PngLevel::Fast => png::CompressionType::Fast,
                PngLevel::Default => png::CompressionType::Default,
                PngLevel::Best => png::CompressionType::Best,
            };
            png::PngEncoder::new_with_quality(w, level, png::FilterType::Adaptive)
                .write_image(img.as_raw(), iw, ih, Rgba8)
        }
        OutFormat::Webp => WebPEncoder::new_lossless(w).write_image(img.as_raw(), iw, ih, Rgba8),
        OutFormat::Bmp => BmpEncoder::new(w).write_image(img.as_raw(), iw, ih, Rgba8),
        OutFormat::Gif => {
            let mut enc = GifEncoder::new_with_speed(w, 10);
            enc.encode_frame(image::Frame::new(img.clone()))
        }
        OutFormat::Tiff => TiffEncoder::new(w).write_image(img.as_raw(), iw, ih, Rgba8),
        OutFormat::Tga => TgaEncoder::new(w).write_image(img.as_raw(), iw, ih, Rgba8),
        OutFormat::Qoi => QoiEncoder::new(w).write_image(img.as_raw(), iw, ih, Rgba8),
        OutFormat::Ico => {
            let small;
            let src = if iw > 256 || ih > 256 {
                let s = 256.0 / iw.max(ih) as f32;
                small = edit::resize(
                    img,
                    (iw as f32 * s).round() as u32,
                    (ih as f32 * s).round() as u32,
                    ResizeFilter::Lanczos3,
                );
                &small
            } else {
                img
            };
            IcoEncoder::new(w).write_image(src.as_raw(), src.width(), src.height(), Rgba8)
        }
        OutFormat::Pnm => {
            let rgb = flatten(img);
            PnmEncoder::new(w).write_image(rgb.as_raw(), iw, ih, Rgb8)
        }
    }
}

// ---------------------------------------------------------------------------
// Batch conversion

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BatchResize {
    None,
    Percent(f32),
    /// Fit inside the box, keeping aspect ratio. Never enlarges.
    Fit(u32, u32),
    Exact(u32, u32),
}

#[derive(Clone, Debug)]
pub struct BatchSettings {
    pub inputs: Vec<PathBuf>,
    pub out_dir: PathBuf,
    pub format: OutFormat,
    pub save: SaveOptions,
    pub resize: BatchResize,
    pub filter: ResizeFilter,
    pub grayscale: bool,
    pub flip_h: bool,
    pub rotate_cw: bool,
    /// `{name}` = original file stem, `{n}` = 1-based index (zero padded).
    pub pattern: String,
    pub overwrite: bool,
}

#[derive(Default)]
pub struct BatchProgress {
    pub total: usize,
    pub done: AtomicUsize,
    pub failed: AtomicUsize,
    pub skipped: AtomicUsize,
    pub cancel: AtomicBool,
    pub finished: AtomicBool,
    pub log: Mutex<Vec<String>>,
}

pub fn output_name(pattern: &str, input: &Path, index: usize, total: usize) -> String {
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    let width = total.to_string().len().max(3);
    let pattern = if pattern.trim().is_empty() { "{name}" } else { pattern };
    pattern
        .replace("{name}", &stem)
        .replace("{n}", &format!("{:0width$}", index + 1))
}

/// Starts a batch on a background thread. Progress is reported through the
/// returned handle, and the UI is woken as files complete.
pub fn start_batch(settings: BatchSettings, ctx: egui::Context) -> Arc<BatchProgress> {
    let progress = Arc::new(BatchProgress { total: settings.inputs.len(), ..Default::default() });
    let p = progress.clone();
    std::thread::spawn(move || {
        if let Err(e) = std::fs::create_dir_all(&settings.out_dir) {
            p.log.lock().unwrap().push(format!("Cannot create output folder: {e}"));
        } else {
            settings.inputs.par_iter().enumerate().for_each(|(i, input)| {
                if p.cancel.load(Ordering::Relaxed) {
                    return;
                }
                match convert_one(&settings, input, i) {
                    Ok(true) => {}
                    Ok(false) => {
                        p.skipped.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        p.failed.fetch_add(1, Ordering::Relaxed);
                        p.log.lock().unwrap().push(format!("{}: {e}", input.display()));
                    }
                }
                p.done.fetch_add(1, Ordering::Relaxed);
                ctx.request_repaint();
            });
        }
        p.finished.store(true, Ordering::Relaxed);
        ctx.request_repaint();
    });
    progress
}

/// Returns `Ok(false)` when skipped because the output exists.
fn convert_one(s: &BatchSettings, input: &Path, index: usize) -> Result<bool, String> {
    let name = output_name(&s.pattern, input, index, s.inputs.len());
    let out = s.out_dir.join(format!("{name}.{}", s.format.ext()));
    if !s.overwrite && out.exists() {
        return Ok(false);
    }
    let decoded = crate::loader::decode(input)?;
    let mut img = Arc::try_unwrap(decoded.image).unwrap_or_else(|a| (*a).clone());
    let (w, h) = img.dimensions();
    let target = match s.resize {
        BatchResize::None => None,
        BatchResize::Percent(p) => Some((
            (w as f32 * p / 100.0).round() as u32,
            (h as f32 * p / 100.0).round() as u32,
        )),
        BatchResize::Fit(bw, bh) => {
            let scale = (bw as f32 / w as f32).min(bh as f32 / h as f32);
            (scale < 1.0).then(|| ((w as f32 * scale).round() as u32, (h as f32 * scale).round() as u32))
        }
        BatchResize::Exact(bw, bh) => Some((bw, bh)),
    };
    if let Some((tw, th)) = target
        && (tw, th) != (w, h) {
            img = edit::resize(&img, tw, th, s.filter);
        }
    if s.rotate_cw {
        img = edit::rotate_cw(&img);
    }
    if s.flip_h {
        edit::flip_h(&mut img);
    }
    if s.grayscale {
        edit::grayscale(&mut img);
    }
    save(&img, &out, s.format, &s.save)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let p = Path::new("C:/x/photo.jpeg");
        assert_eq!(output_name("{name}", p, 0, 10), "photo");
        assert_eq!(output_name("img_{n}", p, 4, 10), "img_005");
        assert_eq!(output_name("", p, 4, 10), "photo");
    }

    #[test]
    fn batch_converts_resizes_and_skips_existing() {
        let dir = std::env::temp_dir().join("sfiv-test-batch");
        let _ = std::fs::remove_dir_all(&dir);
        let out_dir = dir.join("out");
        std::fs::create_dir_all(&dir).unwrap();
        let mut inputs = Vec::new();
        for i in 0..3 {
            let p = dir.join(format!("in{i}.png"));
            RgbaImage::from_pixel(200, 100, image::Rgba([i * 50, 0, 0, 255])).save(&p).unwrap();
            inputs.push(p);
        }
        inputs.push(dir.join("missing.png"));
        let settings = BatchSettings {
            inputs,
            out_dir: out_dir.clone(),
            format: OutFormat::Jpeg,
            save: SaveOptions::default(),
            resize: BatchResize::Fit(50, 50),
            filter: ResizeFilter::Lanczos3,
            grayscale: true,
            flip_h: false,
            rotate_cw: false,
            pattern: "img_{n}".into(),
            overwrite: false,
        };
        let run = |s: BatchSettings| {
            let p = start_batch(s, egui::Context::default());
            while !p.finished.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            p
        };
        let p = run(settings.clone());
        assert_eq!(p.done.load(Ordering::Relaxed), 4);
        assert_eq!(p.failed.load(Ordering::Relaxed), 1);
        let d = crate::loader::decode(&out_dir.join("img_001.jpg")).unwrap();
        assert_eq!(d.image.dimensions(), (50, 25));
        // Second run skips the three existing outputs.
        let p = run(settings);
        assert_eq!(p.skipped.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn roundtrip_all_formats() {
        let dir = std::env::temp_dir().join("sfiv-test-formats");
        std::fs::create_dir_all(&dir).unwrap();
        let img = RgbaImage::from_fn(40, 30, |x, y| image::Rgba([x as u8 * 6, y as u8 * 8, 100, 255]));
        for f in OutFormat::ALL {
            let path = dir.join(format!("t.{}", f.ext()));
            save(&img, &path, f, &SaveOptions::default()).unwrap_or_else(|e| panic!("{f:?}: {e}"));
            let d = crate::loader::decode(&path).unwrap_or_else(|e| panic!("{f:?}: {e}"));
            assert_eq!(d.image.dimensions(), (40, 30), "{f:?}");
        }
    }
}
