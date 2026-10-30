//! Background image decoding, a small decoded-image cache and folder listing.
//!
//! Decoding happens on worker threads so the UI never blocks. The first image
//! is requested from `main` before the window and GL context are created, so
//! decoding and window setup overlap.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use image::codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder};
use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbaImage};

/// File extensions decoded by the `image` crate.
const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "apng", "gif", "bmp", "dib", "ico", "tif", "tiff", "webp",
    "tga", "qoi", "pbm", "pgm", "ppm", "pnm", "pam", "hdr", "exr", "dds", "ff",
];

/// Every extension we can open, for folder browsing and file dialogs.
pub fn extensions() -> &'static [&'static str] {
    static ALL: OnceLock<Vec<&'static str>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut v = IMAGE_EXTENSIONS.to_vec();
        v.extend_from_slice(crate::raw::EXTENSIONS);
        v.extend_from_slice(crate::platform::EXTENSIONS);
        v
    })
}

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions().iter().any(|x| x.eq_ignore_ascii_case(e)))
}

pub struct Frame {
    pub image: Arc<RgbaImage>,
    pub delay: Duration,
}

pub struct Decoded {
    pub path: PathBuf,
    /// The image, or the first frame of an animation.
    pub image: Arc<RgbaImage>,
    pub has_alpha: bool,
    pub file_size: u64,
    pub format: String,
    pub decode_ms: f64,
    /// All frames of an animation (more than one), filled in by the decode
    /// thread after the first frame has already been shown.
    pub frames: OnceLock<Vec<Frame>>,
    /// A better version of `image` produced later (full RAW development).
    pub refined: OnceLock<Arc<RgbaImage>>,
}

/// Memory cap for decoded animation frames.
const ANIMATION_BUDGET: usize = 1024 * 1024 * 1024;

/// Work left after the first image is on screen.
enum Rest<'a> {
    /// Remaining animation frames (and the first frame's delay).
    Frames(Duration, image::Frames<'a>),
    /// The RAW preview was small; develop the sensor data.
    RawDevelop(Arc<Vec<u8>>),
}

/// Reads and decodes a file to RGBA8 (first frame only for animations, full
/// resolution for RAW), applying EXIF orientation.
pub fn decode(path: &Path) -> Result<Decoded, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let (mut d, rest) = decode_first(path, &data, Instant::now())?;
    if let Some(Rest::RawDevelop(raw)) = rest {
        d.image = Arc::new(crate::raw::develop(&raw)?.into_rgba8());
        d.format = d.format.replace("embedded preview", "developed");
    }
    Ok(d)
}

/// The first frame (or RAW embedded preview) from bytes already read.
pub fn decode_first_frame(path: &Path, data: &[u8]) -> Result<RgbaImage, String> {
    let (d, _) = decode_first(path, data, Instant::now())?;
    Ok(Arc::try_unwrap(d.image).unwrap_or_else(|a| (*a).clone()))
}

/// Decodes the first image; for animations also returns the frame iterator so
/// the caller can show the first frame before decoding the rest.
fn decode_first<'a>(path: &Path, data: &'a [u8], t: Instant) -> Result<(Decoded, Option<Rest<'a>>), String> {
    let file_size = data.len() as u64;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_uppercase();
    let done = |img: DynamicImage, format: String, rest: Option<Rest<'a>>| {
        let has_alpha = img.color().has_alpha();
        let decoded = Decoded {
            path: path.to_path_buf(),
            image: Arc::new(img.into_rgba8()),
            has_alpha,
            file_size,
            format,
            decode_ms: t.elapsed().as_secs_f64() * 1000.0,
            frames: OnceLock::new(),
            refined: OnceLock::new(),
        };
        Ok((decoded, rest))
    };

    // RAW files are often TIFF containers, so route them before `image`
    // claims them (it would only find a tiny thumbnail).
    if crate::raw::is_raw(path) {
        let raw = Arc::new(data.to_vec());
        let p = crate::raw::decode(&raw)?;
        let rest = p.needs_develop.then_some(Rest::RawDevelop(raw));
        return done(p.image, format!("{ext} RAW ({})", p.what), rest);
    }
    if crate::platform::is_platform_format(path) || crate::platform::sniff(data) {
        let img = crate::platform::decode(path)?;
        return done(DynamicImage::ImageRgba8(img), format!("{ext} (system codec)"), None);
    }

    let mut reader = ImageReader::new(Cursor::new(data)).with_guessed_format().map_err(|e| e.to_string())?;
    if reader.format().is_none()
        && let Ok(f) = ImageFormat::from_path(path)
    {
        reader.set_format(f);
    }
    let format = reader.format();
    let name = format.map(|f| format!("{f:?}").to_uppercase()).unwrap_or_else(|| ext.clone());

    // Animations: hand back the frame iterator after decoding frame one.
    let frames: Option<image::Frames<'a>> = match format {
        Some(ImageFormat::Gif) => Some(GifDecoder::new(Cursor::new(data)).map_err(|e| e.to_string())?.into_frames()),
        Some(ImageFormat::WebP) => {
            let d = WebPDecoder::new(Cursor::new(data)).map_err(|e| e.to_string())?;
            if d.has_animation() { Some(d.into_frames()) } else { None }
        }
        Some(ImageFormat::Png) => {
            let d = PngDecoder::new(Cursor::new(data)).map_err(|e| e.to_string())?;
            if d.is_apng().unwrap_or(false) {
                Some(d.apng().map_err(|e| e.to_string())?.into_frames())
            } else {
                None
            }
        }
        _ => None,
    };
    if let Some(mut frames) = frames {
        let first = frames.next().ok_or("animation has no frames")?.map_err(|e| e.to_string())?;
        let delay = frame_delay(&first);
        let img = DynamicImage::ImageRgba8(first.into_buffer());
        return done(img, name, Some(Rest::Frames(delay, frames)));
    }

    reader.no_limits();
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    done(img, name, None)
}

fn frame_delay(f: &image::Frame) -> Duration {
    let (n, d) = f.delay().numer_denom_ms();
    let ms = n.checked_div(d).unwrap_or(0);
    // Browsers treat very short delays as 100 ms; so do we.
    Duration::from_millis(if ms < 20 { 100 } else { ms as u64 })
}

/// Decodes in a worker: `first` gets the image as soon as it exists. Then
/// animation frames (`Decoded::frames`) or a full RAW development
/// (`Decoded::refined`) follow, and `more` is called.
fn decode_progressive(path: &Path, first: impl FnOnce(Result<Arc<Decoded>, String>), more: impl FnOnce()) {
    let t = Instant::now();
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => return first(Err(e.to_string())),
    };
    let (decoded, rest) = match decode_first(path, &data, t) {
        Ok(r) => r,
        Err(e) => return first(Err(e)),
    };
    let decoded = Arc::new(decoded);
    first(Ok(decoded.clone()));
    let (delay, rest) = match rest {
        None => return,
        Some(Rest::RawDevelop(raw)) => {
            if let Ok(img) = crate::raw::develop(&raw) {
                let _ = decoded.refined.set(Arc::new(img.into_rgba8()));
                more();
            }
            return;
        }
        Some(Rest::Frames(delay, rest)) => (delay, rest),
    };
    let mut frames = vec![Frame { image: decoded.image.clone(), delay }];
    let mut bytes = decoded.image.as_raw().len();
    for f in rest {
        let Ok(f) = f else { break };
        let delay = frame_delay(&f);
        let image = f.into_buffer();
        bytes += image.as_raw().len();
        frames.push(Frame { image: Arc::new(image), delay });
        if bytes > ANIMATION_BUDGET {
            break;
        }
    }
    if frames.len() > 1 {
        let _ = decoded.frames.set(frames);
        more();
    }
}

/// Lists supported images in `dir`, in natural (human) sort order.
pub fn list_images(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(String, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| is_supported(p))
        .map(|p| (p.file_name().unwrap_or_default().to_string_lossy().to_lowercase(), p))
        .collect();
    files.sort_by(|a, b| natord::compare(&a.0, &b.0));
    files.into_iter().map(|(_, p)| p).collect()
}

pub type Waker = Arc<dyn Fn() + Send + Sync>;

pub struct LoadMsg {
    pub path: PathBuf,
    pub result: Result<Arc<Decoded>, String>,
}

/// Decodes on background threads and keeps a few recently decoded images.
pub struct Loader {
    tx: Sender<LoadMsg>,
    rx: Receiver<LoadMsg>,
    waker: Arc<OnceLock<Waker>>,
    cache: HashMap<PathBuf, Arc<Decoded>>,
    lru: VecDeque<PathBuf>,
    pending: HashSet<PathBuf>,
    budget_bytes: usize,
}

impl Loader {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            waker: Arc::new(OnceLock::new()),
            cache: HashMap::new(),
            lru: VecDeque::new(),
            pending: HashSet::new(),
            budget_bytes: 1536 * 1024 * 1024,
        }
    }

    /// Lets worker threads wake the UI when a decode finishes. (egui's own
    /// repaint request is not enough: before the first GPU frame it may not
    /// wake the event loop, and we want to show a preview then.)
    pub fn set_waker(&self, waker: Waker) {
        let _ = self.waker.set(waker);
    }

    pub fn waker(&self) -> Option<Waker> {
        self.waker.get().cloned()
    }

    /// Starts decoding `path` unless it is cached or already in flight.
    pub fn request(&mut self, path: &Path) {
        if self.cache.contains_key(path) || self.pending.contains(path) {
            return;
        }
        self.pending.insert(path.to_path_buf());
        let tx = self.tx.clone();
        let waker = self.waker.clone();
        let path = path.to_path_buf();
        std::thread::Builder::new()
            .name("decode".into())
            .spawn(move || {
                let wake = || {
                    if let Some(w) = waker.get() {
                        w();
                    }
                };
                decode_progressive(
                    &path.clone(),
                    |result| {
                        let _ = tx.send(LoadMsg { path, result });
                        wake();
                    },
                    wake,
                );
            })
            .expect("spawn decode thread");
    }

    /// Collects finished decodes, caching successes.
    pub fn poll(&mut self) -> Vec<LoadMsg> {
        let mut out = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            self.pending.remove(&msg.path);
            if let Ok(d) = &msg.result {
                self.insert(d.clone());
            }
            out.push(msg);
        }
        out
    }

    pub fn get(&mut self, path: &Path) -> Option<Arc<Decoded>> {
        let d = self.cache.get(path)?.clone();
        self.touch(path);
        Some(d)
    }

    pub fn invalidate(&mut self, path: &Path) {
        self.cache.remove(path);
        self.lru.retain(|p| p != path);
    }

    fn touch(&mut self, path: &Path) {
        self.lru.retain(|p| p != path);
        self.lru.push_back(path.to_path_buf());
    }

    fn insert(&mut self, d: Arc<Decoded>) {
        self.cache.insert(d.path.clone(), d.clone());
        self.touch(&d.path);
        // Evict least recently used until within budget, always keeping the newest two.
        let mut total: usize = self.cache.values().map(|d| d.image.as_raw().len()).sum();
        while total > self.budget_bytes && self.lru.len() > 2 {
            if let Some(old) = self.lru.pop_front()
                && let Some(d) = self.cache.remove(&old) {
                    total -= d.image.as_raw().len();
                }
        }
        while self.lru.len() > 8 {
            if let Some(old) = self.lru.pop_front() {
                self.cache.remove(&old);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes every file in `$SFIV_SAMPLES` (real camera RAWs, HEIC, ...)
    /// and prints what came out. Skipped when the variable isn't set.
    #[test]
    fn decode_samples() {
        let Some(dir) = std::env::var_os("SFIV_SAMPLES") else { return };
        let mut failed = Vec::new();
        for p in list_images(Path::new(&dir)) {
            let t = Instant::now();
            match decode(&p) {
                Ok(d) => println!(
                    "{:<32} {:>5}x{:<5} {:<34} {:>6.0} ms",
                    p.file_name().unwrap().to_string_lossy(),
                    d.image.width(),
                    d.image.height(),
                    d.format,
                    t.elapsed().as_secs_f64() * 1000.0
                ),
                Err(e) => {
                    println!("{:<32} ERROR {e}", p.file_name().unwrap().to_string_lossy());
                    failed.push(p);
                }
            }
        }
        assert!(failed.is_empty(), "failed: {failed:?}");
    }
}
