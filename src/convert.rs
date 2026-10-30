//! Saving to the supported output formats, and multi-threaded batch conversion.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use image::codecs::{
    bmp::BmpEncoder, gif::GifEncoder, ico::IcoEncoder, jpeg::JpegEncoder, png, pnm::PnmEncoder,
    qoi::QoiEncoder, tga::TgaEncoder, tiff::TiffEncoder,
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
            OutFormat::Webp => "WebP",
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
    pub webp_quality: u8,
    pub webp_lossless: bool,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self { jpeg_quality: 90, png_level: PngLevel::Fast, webp_quality: 85, webp_lossless: false }
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
        OutFormat::Webp => {
            // libwebp: lossy with quality, or (better-compressing) lossless.
            let enc = webp::Encoder::from_rgba(img.as_raw(), iw, ih);
            let mem = if opts.webp_lossless { enc.encode_lossless() } else { enc.encode(opts.webp_quality.clamp(1, 100) as f32) };
            if mem.is_empty() {
                return Err(image::ImageError::IoError(std::io::Error::other("WebP encoding failed")));
            }
            w.write_all(&mem).map_err(image::ImageError::IoError)
        }
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BatchMode {
    Convert,
    /// Lossless metadata removal; files keep their format.
    StripMetadata,
}

#[derive(Clone, Debug)]
pub struct BatchSettings {
    pub mode: BatchMode,
    /// StripMetadata only: rewrite the originals instead of writing to `out_dir`.
    pub in_place: bool,
    pub inputs: Vec<PathBuf>,
    pub out_dir: PathBuf,
    pub format: OutFormat,
    pub save: SaveOptions,
    pub resize: BatchResize,
    pub filter: ResizeFilter,
    pub grayscale: bool,
    pub flip_h: bool,
    pub rotate_cw: bool,
    /// See [`output_name`].
    pub pattern: String,
    /// The first value of `{n}`.
    pub start: usize,
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

/// Name tokens for the batch file name pattern, with descriptions.
pub const NAME_TOKENS: &[(&str, &str)] = &[
    ("{name}", "original file name"),
    ("{n}", "counter (zero padded)"),
    ("{parent}", "folder name"),
    ("{date}", "file date, YYYY-MM-DD"),
    ("{ext}", "original extension"),
    ("{w}", "output width"),
    ("{h}", "output height"),
];

pub const NAME_PRESETS: &[&str] = &["{name}", "{name}_converted", "{n}", "{parent}_{n}", "{date}_{name}", "{name}_{w}x{h}"];

/// The output file name (without extension) for input number `index` of
/// `total`. `dims` are the output size if known; without them `{w}`/`{h}`
/// show as "w"/"h" (for previews).
pub fn output_name(pattern: &str, input: &Path, index: usize, total: usize, start: usize, dims: Option<(u32, u32)>) -> String {
    let pattern = if pattern.trim().is_empty() { "{name}" } else { pattern };
    let mut out = pattern.replace("{name}", &input.file_stem().unwrap_or_default().to_string_lossy());
    if out.contains("{n}") {
        let width = (start + total.saturating_sub(1)).to_string().len().max(3);
        out = out.replace("{n}", &format!("{:0width$}", start + index));
    }
    if out.contains("{parent}") {
        let parent = input.parent().and_then(|p| p.file_name()).unwrap_or_default().to_string_lossy();
        out = out.replace("{parent}", &parent);
    }
    if out.contains("{date}") {
        out = out.replace("{date}", &file_date(input));
    }
    out = out.replace("{ext}", &input.extension().unwrap_or_default().to_string_lossy());
    let (w, h) = dims.map_or(("w".to_string(), "h".to_string()), |(w, h)| (w.to_string(), h.to_string()));
    out = out.replace("{w}", &w).replace("{h}", &h);
    // Keep the result a plain file name.
    out.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }).collect()
}

pub fn needs_dims(pattern: &str) -> bool {
    pattern.contains("{w}") || pattern.contains("{h}")
}

/// Modification date (UTC) as YYYY-MM-DD.
fn file_date(path: &Path) -> String {
    let secs = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 to (year, month, day); Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Starts a batch on a background thread. Progress is reported through the
/// returned handle, and the UI is woken as files complete.
pub fn start_batch(settings: BatchSettings, ctx: egui::Context) -> Arc<BatchProgress> {
    let progress = Arc::new(BatchProgress { total: settings.inputs.len(), ..Default::default() });
    let p = progress.clone();
    std::thread::spawn(move || {
        let needs_dir = !(settings.mode == BatchMode::StripMetadata && settings.in_place);
        if let Err(e) = if needs_dir { std::fs::create_dir_all(&settings.out_dir) } else { Ok(()) } {
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
    let total = s.inputs.len();
    if s.mode == BatchMode::StripMetadata {
        if !crate::metadata::is_supported(input) {
            return Err("metadata removal supports JPEG, PNG and WebP".into());
        }
        let out = if s.in_place {
            input.to_path_buf()
        } else {
            let name = output_name(&s.pattern, input, index, total, s.start, None);
            let ext = input.extension().unwrap_or_default().to_string_lossy();
            let out = s.out_dir.join(format!("{name}.{ext}"));
            if !s.overwrite && out.exists() {
                return Ok(false);
            }
            out
        };
        crate::metadata::strip_file(input, &out)?;
        return Ok(true);
    }
    // With {w}/{h} in the pattern the name is only known after resizing.
    let early = (!needs_dims(&s.pattern)).then(|| output_name(&s.pattern, input, index, total, s.start, None));
    if let Some(name) = &early
        && !s.overwrite
        && s.out_dir.join(format!("{name}.{}", s.format.ext())).exists()
    {
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
    let name = early.unwrap_or_else(|| output_name(&s.pattern, input, index, total, s.start, Some(img.dimensions())));
    let out = s.out_dir.join(format!("{name}.{}", s.format.ext()));
    if !s.overwrite && out.exists() {
        return Ok(false);
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
        assert_eq!(output_name("{name}", p, 0, 10, 1, None), "photo");
        assert_eq!(output_name("img_{n}", p, 4, 10, 1, None), "img_005");
        assert_eq!(output_name("", p, 4, 10, 1, None), "photo");
        assert_eq!(output_name("{parent}-{n}.{ext}", p, 0, 5000, 10, None), "x-0010.jpeg");
        assert_eq!(output_name("{name}_{w}x{h}", p, 0, 1, 1, Some((640, 480))), "photo_640x480");
        assert_eq!(output_name("a/b:{name}", p, 0, 1, 1, None), "a_b_photo");
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
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
            mode: BatchMode::Convert,
            in_place: false,
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
            start: 1,
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
        let p = run(settings.clone());
        assert_eq!(p.skipped.load(Ordering::Relaxed), 3);
        // Metadata removal in place: PNGs are fine, the missing file fails.
        let strip = BatchSettings { mode: BatchMode::StripMetadata, in_place: true, ..settings };
        let p = run(strip);
        assert_eq!(p.failed.load(Ordering::Relaxed), 1);
        assert_eq!(p.done.load(Ordering::Relaxed), 4);
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
