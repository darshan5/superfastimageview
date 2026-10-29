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
use std::time::Instant;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbaImage};

/// File extensions we list when browsing a folder.
pub const EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "apng", "gif", "bmp", "dib", "ico", "tif", "tiff", "webp",
    "tga", "qoi", "pbm", "pgm", "ppm", "pnm", "pam", "hdr", "exr", "dds", "ff",
];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

pub struct Decoded {
    pub path: PathBuf,
    pub image: Arc<RgbaImage>,
    pub has_alpha: bool,
    pub file_size: u64,
    pub format: Option<ImageFormat>,
    pub decode_ms: f64,
}

/// Reads and decodes a file to RGBA8, applying EXIF orientation.
pub fn decode(path: &Path) -> Result<Decoded, String> {
    let t = Instant::now();
    // One read syscall into memory is faster than a buffered streaming reader.
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let file_size = data.len() as u64;
    let mut reader = ImageReader::new(Cursor::new(&data))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    if reader.format().is_none()
        && let Ok(f) = ImageFormat::from_path(path) {
            reader.set_format(f);
        }
    let format = reader.format();
    reader.no_limits();
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    let has_alpha = img.color().has_alpha();
    let image = Arc::new(img.into_rgba8());
    Ok(Decoded {
        path: path.to_path_buf(),
        image,
        has_alpha,
        file_size,
        format,
        decode_ms: t.elapsed().as_secs_f64() * 1000.0,
    })
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
                let result = decode(&path).map(Arc::new);
                let _ = tx.send(LoadMsg { path, result });
                if let Some(wake) = waker.get() {
                    wake();
                }
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
