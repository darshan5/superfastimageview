//! Thumbnails for the filmstrip and the grid view.
//!
//! Each frame the UI asks for the thumbnails it is about to draw; anything
//! asked for but not yet started is forgotten at the start of the next frame,
//! so after fast scrolling the workers only decode what is on screen. JPEGs
//! use libjpeg-turbo's DCT-domain scaling (1/8 size decodes ~20x faster), RAW
//! files their embedded previews.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};

use egui::{ColorImage, TextureHandle, TextureOptions};
use image::metadata::Orientation;
use image::{DynamicImage, RgbaImage};

use crate::edit::{self, ResizeFilter};

/// Long side of a thumbnail, in pixels.
pub const SIZE: u32 = 256;
/// ~256 MB of textures at most.
const MAX_READY: usize = 1000;

enum Entry {
    Requested,
    Ready { tex: TextureHandle, used: u64 },
    Failed,
}

struct Queue {
    items: Mutex<VecDeque<PathBuf>>,
    cv: Condvar,
}

pub struct Thumbs {
    ctx: egui::Context,
    entries: HashMap<PathBuf, Entry>,
    queue: Arc<Queue>,
    tx: Sender<(PathBuf, Result<RgbaImage, String>)>,
    rx: Receiver<(PathBuf, Result<RgbaImage, String>)>,
    started: bool,
    frame: u64,
}

impl Thumbs {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        Self {
            ctx: ctx.clone(),
            entries: HashMap::new(),
            queue: Arc::new(Queue { items: Mutex::new(VecDeque::new()), cv: Condvar::new() }),
            tx,
            rx,
            started: false,
            frame: 0,
        }
    }

    /// Call once per frame before any [`Thumbs::get`].
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        // Requests nobody picked up yet: drop them, they're re-made if still visible.
        for p in self.queue.items.lock().unwrap().drain(..) {
            self.entries.remove(&p);
        }
        while let Ok((path, result)) = self.rx.try_recv() {
            // Ignore results for entries invalidated while in flight.
            if !matches!(self.entries.get(&path), Some(Entry::Requested)) {
                continue;
            }
            let entry = match result {
                Ok(img) => {
                    let ci = ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                    let tex = self.ctx.load_texture(path.to_string_lossy(), ci, TextureOptions::LINEAR);
                    Entry::Ready { tex, used: self.frame }
                }
                Err(_) => Entry::Failed,
            };
            self.entries.insert(path, entry);
        }
        self.evict();
    }

    /// The thumbnail if ready; otherwise queues it.
    pub fn get(&mut self, path: &Path) -> Option<&TextureHandle> {
        if !self.entries.contains_key(path) {
            self.entries.insert(path.to_path_buf(), Entry::Requested);
            self.queue.items.lock().unwrap().push_back(path.to_path_buf());
            self.queue.cv.notify_one();
            self.start_workers();
            return None;
        }
        match self.entries.get_mut(path) {
            Some(Entry::Ready { tex, used }) => {
                *used = self.frame;
                Some(tex)
            }
            _ => None,
        }
    }

    pub fn failed(&self, path: &Path) -> bool {
        matches!(self.entries.get(path), Some(Entry::Failed))
    }

    /// The file changed on disk.
    pub fn invalidate(&mut self, path: &Path) {
        self.entries.remove(path);
    }

    fn evict(&mut self) {
        let ready = self.entries.values().filter(|e| matches!(e, Entry::Ready { .. })).count();
        if ready <= MAX_READY {
            return;
        }
        let mut by_age: Vec<(u64, PathBuf)> = self
            .entries
            .iter()
            .filter_map(|(p, e)| match e {
                Entry::Ready { used, .. } => Some((*used, p.clone())),
                _ => None,
            })
            .collect();
        by_age.sort_unstable_by_key(|e| e.0);
        for (_, p) in by_age.into_iter().take(ready - MAX_READY * 3 / 4) {
            self.entries.remove(&p);
        }
    }

    fn start_workers(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let n = std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(2, 4);
        for _ in 0..n {
            let queue = self.queue.clone();
            let tx = self.tx.clone();
            let ctx = self.ctx.clone();
            std::thread::spawn(move || {
                loop {
                    let path = {
                        let mut items = queue.items.lock().unwrap();
                        loop {
                            if let Some(p) = items.pop_front() {
                                break p;
                            }
                            items = queue.cv.wait(items).unwrap();
                        }
                    };
                    let result = make(&path);
                    if tx.send((path, result)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
        }
    }
}

/// Decodes a small version of the image.
pub fn make(path: &Path) -> Result<RgbaImage, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let img = if data.starts_with(&[0xFF, 0xD8]) {
        match jpeg_scaled(&data) {
            Ok(img) => img,
            // e.g. CMYK, which libjpeg-turbo can't convert to RGB.
            Err(_) => crate::loader::decode_first_frame(path, &data)?,
        }
    } else if crate::raw::is_raw(path) {
        crate::raw::thumbnail(&Arc::new(data), SIZE)?.into_rgba8()
    } else {
        crate::loader::decode_first_frame(path, &data)?
    };
    Ok(shrink(img))
}

fn shrink(img: RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let long = w.max(h);
    if long <= SIZE {
        return img;
    }
    let s = SIZE as f32 / long as f32;
    edit::resize(&img, (w as f32 * s).round() as u32, (h as f32 * s).round() as u32, ResizeFilter::Bilinear)
}

/// Decodes at the smallest DCT scale that still gives at least [`SIZE`] px.
fn jpeg_scaled(data: &[u8]) -> Result<RgbaImage, String> {
    use turbojpeg::{Decompressor, Image, PixelFormat, ScalingFactor};
    let mut dec = Decompressor::new().map_err(|e| e.to_string())?;
    let header = dec.read_header(data).map_err(|e| e.to_string())?;
    let long = header.width.max(header.height);
    let factor = [ScalingFactor::ONE_EIGHTH, ScalingFactor::ONE_QUARTER, ScalingFactor::ONE_HALF]
        .into_iter()
        .find(|f| f.scale(long) >= SIZE as usize)
        .unwrap_or(ScalingFactor::ONE);
    dec.set_scaling_factor(factor).map_err(|e| e.to_string())?;
    let h = header.scaled(factor);
    let mut pixels = vec![0u8; 4 * h.width * h.height];
    let out = Image { pixels: &mut pixels[..], width: h.width, pitch: 4 * h.width, height: h.height, format: PixelFormat::RGBA };
    dec.decompress(data, out).map_err(|e| e.to_string())?;
    let img = RgbaImage::from_raw(h.width as u32, h.height as u32, pixels).ok_or("bad JPEG size")?;
    let mut img = DynamicImage::ImageRgba8(img);
    if let Some(o) = Orientation::from_exif(crate::jpegtran::orientation(data) as u8) {
        img.apply_orientation(o);
    }
    Ok(img.into_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_thumbnail_is_scaled_and_upright() {
        let dir = std::env::temp_dir().join("sfiv-test-thumbs");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("big.jpg");
        image::RgbImage::from_fn(3000, 1000, |x, _| image::Rgb([(x / 12) as u8, 0, 0])).save(&p).unwrap();
        let t = make(&p).unwrap();
        assert_eq!(t.dimensions(), (256, 85));
        let p = dir.join("small.png");
        image::RgbaImage::new(40, 20).save(&p).unwrap();
        assert_eq!(make(&p).unwrap().dimensions(), (40, 20));
    }

    /// `SFIV_SAMPLES=<dir> cargo test thumb_samples -- --nocapture`
    #[test]
    fn thumb_samples() {
        let Some(dir) = std::env::var_os("SFIV_SAMPLES") else { return };
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let t = std::time::Instant::now();
            let r = make(&e.path());
            println!("{:?}: {:?} in {:.0} ms", e.file_name(), r.map(|i| i.dimensions()), t.elapsed().as_secs_f64() * 1000.0);
        }
    }
}
