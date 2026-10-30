//! The viewer/editor UI.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use egui::{
    Color32, Key, Modifiers, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle,
    Ui, Vec2, ViewportCommand,
};
use image::RgbaImage;

use crate::canvas::{Canvas, checker_texture};
use crate::convert::{
    self, BatchMode, BatchProgress, BatchResize, BatchSettings, NAME_PRESETS, NAME_TOKENS, OutFormat, PngLevel, SaveOptions,
};
use crate::edit::{self, ResizeFilter};
use crate::jpegtran;
use crate::loader::{self, Decoded, Loader};
use crate::paint::{self, Dirty};
use crate::thumbs::Thumbs;
use crate::{metadata, text};

const APP_NAME: &str = "SuperFastImageView";
const BG: Color32 = Color32::from_gray(24);
const UNDO_BUDGET: usize = 1024 * 1024 * 1024;

static NEXT_VERSION: AtomicU64 = AtomicU64::new(1);

fn next_version() -> u64 {
    NEXT_VERSION.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Document

pub struct Document {
    pub path: Option<PathBuf>,
    pub image: Arc<RgbaImage>,
    pub has_alpha: bool,
    pub file_size: u64,
    pub format: String,
    pub decode_ms: f64,
    pub dirty: bool,
    pub version: u64,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The decode this came from; animation frames arrive on it later.
    source: Option<Arc<Decoded>>,
    anim: Option<Anim>,
}

/// One undoable edit: the pixels on the other side of it, and its name.
struct Step {
    image: Arc<RgbaImage>,
    label: &'static str,
}

/// Playback state of an animated GIF / WebP / APNG.
struct Anim {
    frame: usize,
    next_at: Instant,
    playing: bool,
}

impl Document {
    fn from_decoded(d: &Arc<Decoded>) -> Self {
        Self {
            path: Some(d.path.clone()),
            image: d.refined.get().unwrap_or(&d.image).clone(),
            has_alpha: d.has_alpha,
            file_size: d.file_size,
            format: d.format.clone(),
            decode_ms: d.decode_ms,
            dirty: false,
            version: next_version(),
            undo: Vec::new(),
            redo: Vec::new(),
            source: Some(d.clone()),
            anim: None,
        }
    }

    fn untitled(img: RgbaImage) -> Self {
        Self {
            path: None,
            image: Arc::new(img),
            has_alpha: true,
            file_size: 0,
            format: String::new(),
            decode_ms: 0.0,
            dirty: true,
            version: next_version(),
            undo: Vec::new(),
            redo: Vec::new(),
            source: None,
            anim: None,
        }
    }

    fn name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into())
    }

    /// Saves the current pixels for undo. Cheap: the pixels are shared until
    /// the next edit copies them (`Arc::make_mut`).
    fn checkpoint(&mut self, label: &'static str) {
        // Editing works on the current frame; the animation stops for good.
        self.anim = None;
        self.source = None;
        self.undo.push(Step { image: self.image.clone(), label });
        self.redo.clear();
        let mut total: usize = self.undo.iter().map(|s| s.image.as_raw().len()).sum();
        while self.undo.len() > 1 && (total > UNDO_BUDGET || self.undo.len() > 50) {
            total -= self.undo.remove(0).image.as_raw().len();
        }
    }

    fn touched(&mut self) {
        self.version = next_version();
        self.dirty = true;
    }

    fn replace(&mut self, img: RgbaImage, label: &'static str) {
        self.checkpoint(label);
        self.image = Arc::new(img);
        self.touched();
    }

    fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop() else { return false };
        let current = std::mem::replace(&mut self.image, step.image);
        self.redo.push(Step { image: current, label: step.label });
        self.touched();
        true
    }

    fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else { return false };
        let current = std::mem::replace(&mut self.image, step.image);
        self.undo.push(Step { image: current, label: step.label });
        self.touched();
        true
    }
}

// ---------------------------------------------------------------------------
// Tools & dialogs

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tool {
    Hand,
    Select,
    Picker,
    Brush,
    Eraser,
    Line,
    Rect,
    Ellipse,
    Fill,
    Text,
}

impl Tool {
    const ALL: [Tool; 10] = [
        Tool::Hand,
        Tool::Select,
        Tool::Picker,
        Tool::Brush,
        Tool::Eraser,
        Tool::Line,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Fill,
        Tool::Text,
    ];

    fn label(self) -> &'static str {
        match self {
            Tool::Hand => "Pan / view",
            Tool::Select => "Select / crop",
            Tool::Picker => "Color picker",
            Tool::Brush => "Brush",
            Tool::Eraser => "Eraser",
            Tool::Line => "Line",
            Tool::Rect => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Fill => "Fill",
            Tool::Text => "Text",
        }
    }

    fn uses_size(self) -> bool {
        matches!(self, Tool::Brush | Tool::Eraser | Tool::Line | Tool::Rect | Tool::Ellipse)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShapeStyle {
    Outline,
    Filled,
    OutlineAndFill,
}

struct StrokeState {
    tool: Tool,
    button: PointerButton,
    start: Pos2,
    last: Pos2,
}

/// Text being typed with the Text tool, not yet drawn into the image.
struct TextDraft {
    /// Top-left of the first line, in image pixels.
    pos: Pos2,
    content: String,
    preview: Option<TextPreview>,
}

struct TextPreview {
    key: (String, usize, u32, [u8; 4]),
    img: RgbaImage,
    ox: i32,
    oy: i32,
    tex: TextureHandle,
}

struct Slideshow {
    /// The image whose timer is running.
    shown: Option<PathBuf>,
    next_at: Instant,
    paused: bool,
    was_fullscreen: bool,
}

#[derive(Clone, Copy)]
struct View {
    /// Physical screen pixels per image pixel.
    zoom: f32,
    /// Offset of the image center from the viewport center, in points.
    pan: Vec2,
    fit: bool,
}

struct ResizeDialog {
    w: u32,
    h: u32,
    orig: (u32, u32),
    keep_aspect: bool,
    filter: ResizeFilter,
}

struct NewDialog {
    w: u32,
    h: u32,
    transparent: bool,
}

struct BatchDialog {
    open: bool,
    mode: BatchMode,
    in_place: bool,
    inputs: Vec<PathBuf>,
    out_dir: String,
    format: OutFormat,
    save: SaveOptions,
    resize_mode: usize,
    percent: f32,
    w: u32,
    h: u32,
    filter: ResizeFilter,
    grayscale: bool,
    flip_h: bool,
    rotate_cw: bool,
    pattern: String,
    start: usize,
    overwrite: bool,
    recursive: bool,
    progress: Option<Arc<BatchProgress>>,
    /// (pattern, inputs, start, mode) -> how many output names collide.
    duplicates: Option<((String, usize, usize, BatchMode), usize)>,
}

impl Default for BatchDialog {
    fn default() -> Self {
        Self {
            open: false,
            mode: BatchMode::Convert,
            in_place: false,
            inputs: Vec::new(),
            out_dir: String::new(),
            format: OutFormat::Jpeg,
            save: SaveOptions::default(),
            resize_mode: 0,
            percent: 50.0,
            w: 1920,
            h: 1080,
            filter: ResizeFilter::Lanczos3,
            grayscale: false,
            flip_h: false,
            rotate_cw: false,
            pattern: "{name}".into(),
            start: 1,
            overwrite: false,
            recursive: false,
            progress: None,
            duplicates: None,
        }
    }
}

/// An action deferred until the user answers "save changes?".
enum Pending {
    Open(PathBuf),
    Replace(Box<Document>),
    Quit,
}

// ---------------------------------------------------------------------------
// App

pub struct App {
    ctx: egui::Context,
    loader: Loader,
    timing: bool,

    folder_dir: Option<PathBuf>,
    folder: Vec<PathBuf>,
    folder_rx: Option<Receiver<(PathBuf, Vec<PathBuf>)>>,
    /// The file we want on screen; may still be decoding.
    target: Option<PathBuf>,
    load_error: Option<String>,
    nav_dir: isize,

    doc: Option<Document>,
    canvas: Canvas,
    uploaded_version: u64,
    checker: Option<TextureHandle>,
    view: View,
    fit_upscale: bool,
    title: String,

    tool: Tool,
    primary: Color32,
    secondary: Color32,
    size: f32,
    shape_style: ShapeStyle,
    tolerance: u8,
    erase_transparent: bool,
    stroke: Option<StrokeState>,
    /// x, y, w, h in image pixels.
    selection: Option<[u32; 4]>,
    hover: Option<(u32, u32)>,
    show_tools: bool,
    text_font: usize,
    text_size: f32,
    text_draft: Option<TextDraft>,
    text_focus: bool,

    thumbs: Thumbs,
    show_strip: bool,
    /// The image the strip last scrolled to.
    strip_scrolled: Option<PathBuf>,
    grid: bool,
    grid_cursor: usize,
    grid_cell: f32,
    grid_cols: usize,
    grid_scroll: bool,
    slide: Option<Slideshow>,
    slide_secs: f32,
    slide_loop: bool,
    slide_fullscreen: bool,
    /// (file, metadata found) for the info panel.
    meta_info: Option<(PathBuf, String)>,

    save_opts: SaveOptions,
    resize_dlg: Option<ResizeDialog>,
    new_dlg: Option<NewDialog>,
    batch: BatchDialog,
    show_help: bool,
    show_info: bool,
    confirm_delete: bool,
    pending: Option<Pending>,
    allow_close: bool,
    status: String,
}

impl App {
    pub fn new(ctx: &egui::Context, loader: Loader, initial: Option<PathBuf>) -> Self {
        let mut app = Self {
            ctx: ctx.clone(),
            loader,
            timing: std::env::var_os("SFIV_TIMING").is_some(),
            folder_dir: None,
            folder: Vec::new(),
            folder_rx: None,
            target: None,
            load_error: None,
            nav_dir: 1,
            doc: None,
            canvas: Canvas::default(),
            uploaded_version: 0,
            checker: None,
            view: View { zoom: 1.0, pan: Vec2::ZERO, fit: true },
            fit_upscale: false,
            title: String::new(),
            tool: Tool::Hand,
            primary: Color32::BLACK,
            secondary: Color32::WHITE,
            size: 4.0,
            shape_style: ShapeStyle::Outline,
            tolerance: 32,
            erase_transparent: false,
            stroke: None,
            selection: None,
            hover: None,
            show_tools: false,
            text_font: 0,
            text_size: 48.0,
            text_draft: None,
            text_focus: false,
            thumbs: Thumbs::new(ctx),
            show_strip: false,
            strip_scrolled: None,
            grid: false,
            grid_cursor: 0,
            grid_cell: 180.0,
            grid_cols: 1,
            grid_scroll: false,
            slide: None,
            slide_secs: 4.0,
            slide_loop: true,
            slide_fullscreen: true,
            meta_info: None,
            save_opts: SaveOptions::default(),
            resize_dlg: None,
            new_dlg: None,
            batch: BatchDialog::default(),
            show_help: false,
            show_info: false,
            confirm_delete: false,
            pending: None,
            allow_close: false,
            status: String::new(),
        };
        ctx.set_visuals(egui::Visuals::dark());
        if let Some(p) = initial {
            if p.is_dir() {
                app.scan_folder(&p);
            } else {
                // Decoding was already started in `main`.
                app.target = Some(p.clone());
                if let Some(dir) = p.parent() {
                    app.scan_folder(dir);
                }
            }
        }
        app
    }

    // -- navigation ---------------------------------------------------------

    fn scan_folder(&mut self, dir: &Path) {
        let (tx, rx) = channel();
        let dir = dir.to_path_buf();
        let ctx = self.ctx.clone();
        let waker = self.loader.waker();
        std::thread::spawn(move || {
            let list = loader::list_images(&dir);
            let _ = tx.send((dir, list));
            match waker {
                Some(w) => w(),
                None => ctx.request_repaint(),
            }
        });
        self.folder_rx = Some(rx);
    }

    fn index(&self) -> Option<usize> {
        let t = self.target.as_ref()?;
        self.folder.iter().position(|p| p == t)
    }

    /// Opens `path`, asking to save unsaved edits first.
    fn request_open(&mut self, path: PathBuf) {
        if self.doc.as_ref().is_some_and(|d| d.dirty) {
            self.pending = Some(Pending::Open(path));
        } else {
            self.open(path);
        }
    }

    fn open(&mut self, path: PathBuf) {
        self.load_error = None;
        self.target = Some(path.clone());
        if let Some(d) = self.loader.get(&path) {
            self.show(&d);
        } else {
            self.loader.request(&path);
        }
        if let Some(dir) = path.parent()
            && self.folder_dir.as_deref() != Some(dir) && self.folder_rx.is_none() {
                self.scan_folder(dir);
            }
    }

    fn show(&mut self, d: &Arc<Decoded>) {
        self.doc = Some(Document::from_decoded(d));
        self.view.fit = true;
        self.selection = None;
        self.stroke = None;
        self.text_draft = None;
        self.prefetch();
    }

    fn navigate(&mut self, delta: isize) {
        if self.folder.is_empty() {
            return;
        }
        let n = self.folder.len() as isize;
        let i = match self.index() {
            Some(i) => i as isize,
            None => {
                if delta > 0 {
                    -1
                } else {
                    n
                }
            }
        };
        let next = (i + delta).rem_euclid(n) as usize;
        self.nav_dir = delta.signum();
        self.request_open(self.folder[next].clone());
    }

    fn go_to(&mut self, index: usize) {
        if let Some(p) = self.folder.get(index).cloned() {
            self.request_open(p);
        }
    }

    /// Decodes the neighbours in the background so arrow keys feel instant.
    fn prefetch(&mut self) {
        let Some(i) = self.index() else { return };
        let n = self.folder.len() as isize;
        if n < 2 {
            return;
        }
        let d = self.nav_dir;
        for off in [d, -d, 2 * d] {
            let j = (i as isize + off).rem_euclid(n) as usize;
            let p = self.folder[j].clone();
            self.loader.request(&p);
        }
    }

    fn poll_background(&mut self, ctx: &egui::Context) {
        for msg in self.loader.poll() {
            if self.target.as_ref() != Some(&msg.path) {
                continue;
            }
            if self.doc.as_ref().is_some_and(|d| d.path.as_ref() == Some(&msg.path)) {
                continue;
            }
            match msg.result {
                Ok(d) => {
                    if self.timing {
                        eprintln!("[timing] decoded {} in {:.1} ms", d.path.display(), d.decode_ms);
                    }
                    self.show(&d);
                }
                Err(e) => {
                    self.doc = None;
                    self.canvas.clear();
                    self.load_error = Some(format!("{}: {e}", msg.path.display()));
                }
            }
        }
        if let Some(rx) = &self.folder_rx
            && let Ok((dir, list)) = rx.try_recv() {
                self.folder_rx = None;
                self.folder_dir = Some(dir);
                self.folder = list;
                if self.target.is_none() {
                    if let Some(first) = self.folder.first().cloned() {
                        self.open(first);
                    }
                } else {
                    self.prefetch();
                }
                ctx.request_repaint();
            }
    }

    // -- file operations ----------------------------------------------------

    fn open_dialog(&mut self) {
        let mut dlg = rfd::FileDialog::new()
            .add_filter("Images", loader::extensions())
            .add_filter("All files", &["*"]);
        if let Some(dir) = &self.folder_dir {
            dlg = dlg.set_directory(dir);
        }
        if let Some(p) = dlg.pick_file() {
            self.request_open(p);
        }
    }

    fn save(&mut self) -> bool {
        let Some(doc) = &self.doc else { return false };
        match doc.path.clone().and_then(|p| OutFormat::from_path(&p).map(|f| (p, f))) {
            Some((p, f)) => self.save_to(p, f),
            None => self.save_as(),
        }
    }

    fn save_as(&mut self) -> bool {
        let Some(doc) = &self.doc else { return false };
        let current = doc.path.as_deref().and_then(OutFormat::from_path).unwrap_or(OutFormat::Png);
        let stem = doc
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".into());
        let mut dlg = rfd::FileDialog::new().set_file_name(format!("{stem}.{}", current.ext()));
        // The first filter is the dialog's default, so list the current format first.
        let mut formats = vec![current];
        formats.extend(OutFormat::ALL.into_iter().filter(|f| *f != current));
        for f in formats {
            dlg = dlg.add_filter(f.label(), f.extensions());
        }
        if let Some(dir) = doc.path.as_ref().and_then(|p| p.parent()).or(self.folder_dir.as_deref()) {
            dlg = dlg.set_directory(dir);
        }
        let Some(mut path) = dlg.save_file() else { return false };
        let fmt = match OutFormat::from_path(&path) {
            Some(f) => f,
            None => {
                path.set_extension(current.ext());
                current
            }
        };
        self.save_to(path, fmt)
    }

    fn save_to(&mut self, path: PathBuf, fmt: OutFormat) -> bool {
        let Some(doc) = &mut self.doc else { return false };
        let t = Instant::now();
        match convert::save(&doc.image, &path, fmt, &self.save_opts) {
            Ok(()) => {
                self.loader.invalidate(&path);
                self.thumbs.invalidate(&path);
                self.meta_info = None;
                doc.path = Some(path.clone());
                doc.dirty = false;
                doc.file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                self.status = format!("Saved {} ({:.0} ms)", path.display(), t.elapsed().as_secs_f64() * 1000.0);
                self.target = Some(path.clone());
                if !self.folder.contains(&path)
                    && let Some(dir) = path.parent() {
                        self.scan_folder(dir);
                    }
                true
            }
            Err(e) => {
                self.status = format!("Save failed: {e}");
                false
            }
        }
    }

    fn delete_current(&mut self) {
        let Some(path) = self.doc.as_ref().and_then(|d| d.path.clone()) else { return };
        if let Err(e) = trash::delete(&path) {
            self.status = format!("Delete failed: {e}");
            return;
        }
        self.loader.invalidate(&path);
        let i = self.index();
        self.folder.retain(|p| p != &path);
        self.doc = None;
        self.canvas.clear();
        self.target = None;
        self.status = format!("Moved {} to the trash", path.display());
        if let Some(i) = i
            && !self.folder.is_empty() {
                let p = self.folder[i.min(self.folder.len() - 1)].clone();
                self.open(p);
            }
    }

    fn copy(&mut self) {
        let Some(doc) = &self.doc else { return };
        let img = match self.selection {
            Some([x, y, w, h]) => edit::crop(&doc.image, x, y, w, h),
            None => (*doc.image).clone(),
        };
        let data = arboard::ImageData {
            width: img.width() as usize,
            height: img.height() as usize,
            bytes: img.into_raw().into(),
        };
        self.status = match arboard::Clipboard::new().and_then(|mut c| c.set_image(data)) {
            Ok(()) => "Copied to clipboard".into(),
            Err(e) => format!("Copy failed: {e}"),
        };
    }

    fn paste(&mut self) {
        let mut cb = match arboard::Clipboard::new() {
            Ok(c) => c,
            Err(e) => {
                self.status = format!("Clipboard unavailable: {e}");
                return;
            }
        };
        if let Ok(img) = cb.get_image()
            && let Some(img) = RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned()) {
                self.replace_doc(Document::untitled(img));
                return;
            }
        // A copied file path also works.
        if let Ok(text) = cb.get_text() {
            let p = PathBuf::from(text.trim().trim_matches('"'));
            if p.is_file() {
                self.request_open(p);
                return;
            }
        }
        self.status = "Clipboard has no image".into();
    }

    fn replace_doc(&mut self, doc: Document) {
        if self.doc.as_ref().is_some_and(|d| d.dirty) {
            self.pending = Some(Pending::Replace(Box::new(doc)));
        } else {
            self.target = None;
            self.doc = Some(doc);
            self.view.fit = true;
            self.selection = None;
        }
    }

    fn run_pending(&mut self, ctx: &egui::Context, p: Pending) {
        if let Some(d) = &mut self.doc {
            d.dirty = false;
        }
        match p {
            Pending::Open(path) => self.open(path),
            Pending::Replace(doc) => self.replace_doc(*doc),
            Pending::Quit => {
                self.allow_close = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }

    // -- edits --------------------------------------------------------------

    fn transform(&mut self, label: &'static str, f: impl FnOnce(&RgbaImage) -> RgbaImage) {
        if let Some(doc) = &mut self.doc {
            let img = f(&doc.image);
            doc.replace(img, label);
            self.selection = None;
            self.view.fit = true;
        }
    }

    fn modify(&mut self, label: &'static str, f: impl FnOnce(&mut RgbaImage)) {
        if let Some(doc) = &mut self.doc {
            doc.checkpoint(label);
            f(Arc::make_mut(&mut doc.image));
            doc.touched();
        }
    }

    fn crop_to_selection(&mut self) {
        if let Some([x, y, w, h]) = self.selection {
            self.transform("Crop", |img| edit::crop(img, x, y, w, h));
        } else {
            self.status = "Select an area first (Select tool)".into();
        }
    }

    fn undo(&mut self) {
        self.text_draft = None;
        if let Some(d) = &mut self.doc
            && d.undo() {
                self.selection = None;
            }
    }

    fn redo(&mut self) {
        if let Some(d) = &mut self.doc
            && d.redo() {
                self.selection = None;
            }
    }

    fn open_resize(&mut self) {
        if let Some(d) = &self.doc {
            let (w, h) = d.image.dimensions();
            self.resize_dlg = Some(ResizeDialog { w, h, orig: (w, h), keep_aspect: true, filter: ResizeFilter::Lanczos3 });
        }
    }

    /// Undo point for the current paint tool.
    fn checkpoint(&mut self) {
        if let Some(d) = &mut self.doc {
            d.checkpoint(self.tool.label());
        }
    }

    /// Undoes or redoes until `target` steps are on the undo stack.
    fn history_jump(&mut self, target: usize) {
        let Some(d) = &mut self.doc else { return };
        while d.undo.len() > target && d.undo() {}
        while d.undo.len() < target && d.redo() {}
        self.selection = None;
    }

    /// Applies a paint operation and re-uploads only the touched pixels.
    fn paint_op(&mut self, f: impl FnOnce(&mut RgbaImage) -> Option<Dirty>) {
        let Some(doc) = &mut self.doc else { return };
        if let Some(d) = f(Arc::make_mut(&mut doc.image)) {
            self.canvas.update_region(&doc.image, d.x0, d.y0, d.x1, d.y1);
            doc.touched();
            self.uploaded_version = doc.version;
        }
    }

    // -- view ---------------------------------------------------------------

    fn fit_zoom(&self, area: Rect, w: u32, h: u32, ppp: f32) -> f32 {
        let z = (area.width() * ppp / w as f32).min(area.height() * ppp / h as f32);
        if self.fit_upscale { z } else { z.min(1.0) }
    }

    fn image_rect(&self, area: Rect, w: u32, h: u32, ppp: f32) -> Rect {
        let scale = self.view.zoom / ppp;
        let r = Rect::from_center_size(area.center() + self.view.pan, Vec2::new(w as f32, h as f32) * scale);
        // Snap to physical pixels so 100% zoom is crisp.
        let min = Pos2::new((r.min.x * ppp).round() / ppp, (r.min.y * ppp).round() / ppp);
        Rect::from_min_size(min, r.size())
    }

    fn zoom_about(&mut self, area: Rect, pivot: Pos2, factor: f32, ppp: f32) {
        let Some(doc) = &self.doc else { return };
        let (w, h) = doc.image.dimensions();
        let r = self.image_rect(area, w, h, ppp);
        let old = self.view.zoom / ppp;
        let new_zoom = (self.view.zoom * factor).clamp(0.01, 64.0);
        let new = new_zoom / ppp;
        let ip = (pivot - r.min) / old;
        let min = pivot - ip * new;
        let center = min + Vec2::new(w as f32, h as f32) * new / 2.0;
        self.view.zoom = new_zoom;
        self.view.pan = center - area.center();
        self.view.fit = false;
    }

    fn set_zoom(&mut self, zoom: f32) {
        self.view.zoom = zoom.clamp(0.01, 64.0);
        self.view.pan = Vec2::ZERO;
        self.view.fit = false;
    }

    // -- input --------------------------------------------------------------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // Only a focused text field should swallow our single-key shortcuts.
        if ctx.text_edit_focused() {
            return;
        }
        let cmd = Modifiers::COMMAND;
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let k = |m: Modifiers, key: Key| ctx.input_mut(|i| i.consume_key(m, key));

        // Most specific modifiers first: Ctrl+Shift+S must not trigger Ctrl+S.
        if k(cmd_shift, Key::S) {
            self.save_as();
        } else if k(cmd_shift, Key::Z) {
            self.redo();
        } else if k(cmd_shift, Key::R) {
            self.lossless(jpegtran::Op::RotateCw);
        } else if k(cmd_shift, Key::L) {
            self.lossless(jpegtran::Op::RotateCcw);
        } else if k(cmd_shift, Key::I) {
            self.modify("Invert colors", edit::invert);
        } else if k(cmd, Key::E) {
            self.strip_current();
        } else if k(cmd, Key::O) {
            self.open_dialog();
        } else if k(cmd, Key::S) {
            self.save();
        } else if k(cmd, Key::Z) {
            self.undo();
        } else if k(cmd, Key::Y) {
            self.crop_to_selection();
        } else if k(cmd, Key::R) {
            self.open_resize();
        } else if k(cmd, Key::G) {
            self.modify("Grayscale", edit::grayscale);
        } else if k(cmd, Key::C) {
            self.copy();
        } else if k(cmd, Key::V) {
            self.paste();
        } else if k(cmd, Key::N) {
            self.new_dlg = Some(NewDialog { w: 800, h: 600, transparent: false });
        } else if k(cmd, Key::A) {
            if let Some(d) = &self.doc {
                self.selection = Some([0, 0, d.image.width(), d.image.height()]);
            }
        } else if k(cmd, Key::Q) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }

        let none = Modifiers::NONE;
        let pressed = |key: Key| ctx.input_mut(|i| i.consume_key(none, key));
        if self.grid {
            self.grid_keys(ctx);
            return;
        }
        if pressed(Key::F5) {
            if self.slide.is_some() {
                self.stop_slideshow(ctx);
            } else {
                self.start_slideshow(ctx);
            }
        }
        if let Some(s) = &mut self.slide {
            if pressed(Key::Space) {
                s.paused = !s.paused;
                s.next_at = Instant::now() + Duration::from_secs_f32(self.slide_secs);
            }
            if pressed(Key::Escape) {
                self.stop_slideshow(ctx);
            }
        }
        if pressed(Key::ArrowRight) || pressed(Key::Space) || pressed(Key::PageDown) {
            self.navigate(1);
        }
        if pressed(Key::ArrowLeft) || pressed(Key::Backspace) || pressed(Key::PageUp) {
            self.navigate(-1);
        }
        if pressed(Key::Home) {
            self.go_to(0);
        }
        if pressed(Key::End) {
            self.go_to(self.folder.len().saturating_sub(1));
        }
        if pressed(Key::R) {
            self.transform("Rotate right", edit::rotate_cw);
        }
        if pressed(Key::L) {
            self.transform("Rotate left", edit::rotate_ccw);
        }
        if pressed(Key::H) {
            self.modify("Flip horizontal", edit::flip_h);
        }
        if pressed(Key::V) {
            self.modify("Flip vertical", edit::flip_v);
        }
        if pressed(Key::Num0) || pressed(Key::F) {
            self.view.fit = true;
        }
        if pressed(Key::Num1) {
            self.set_zoom(1.0);
        }
        let zoom_in = ctx.input_mut(|i| i.consume_key(none, Key::Plus) || i.consume_key(none, Key::Equals));
        let zoom_out = pressed(Key::Minus);
        if zoom_in || zoom_out {
            let area = ctx.content_rect();
            let ppp = ctx.pixels_per_point();
            self.zoom_about(area, area.center(), if zoom_in { 1.25 } else { 0.8 }, ppp);
        }
        if pressed(Key::Delete) && self.doc.as_ref().is_some_and(|d| d.path.is_some()) {
            self.confirm_delete = true;
        }
        if pressed(Key::Enter) || pressed(Key::F11) {
            toggle_fullscreen(ctx);
        }
        if pressed(Key::B) {
            self.open_batch();
        }
        if pressed(Key::I) {
            self.show_info = !self.show_info;
        }
        if pressed(Key::E) {
            self.show_tools = !self.show_tools;
            if !self.show_tools {
                self.tool = Tool::Hand;
            }
        }
        if pressed(Key::T) {
            self.show_strip = !self.show_strip;
        }
        if pressed(Key::G) {
            self.toggle_grid();
        }
        if pressed(Key::P) {
            self.toggle_playback();
        }
        if pressed(Key::F1) {
            self.show_help = !self.show_help;
        }
        if pressed(Key::Escape) {
            if self.stroke.is_some() {
                self.stroke = None;
            } else if self.text_draft.is_some() {
                self.text_draft = None;
            } else if self.close_top_dialog() {
            } else if self.selection.is_some() {
                self.selection = None;
            } else if is_fullscreen(ctx) {
                ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false));
            } else {
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }

    /// Closes the most recently relevant open dialog; false if none was open.
    fn close_top_dialog(&mut self) -> bool {
        if self.resize_dlg.take().is_some() || self.new_dlg.take().is_some() {
            return true;
        }
        for open in [&mut self.show_help, &mut self.show_info] {
            if *open {
                *open = false;
                return true;
            }
        }
        if self.batch.open {
            if let Some(p) = &self.batch.progress {
                p.cancel.store(true, Ordering::Relaxed);
            }
            self.batch.open = false;
            self.batch.progress = None;
            return true;
        }
        false
    }

    fn grid_keys(&mut self, ctx: &egui::Context) {
        let pressed = |key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        let n = self.folder.len() as isize;
        let cols = self.grid_cols.max(1) as isize;
        let mut c = self.grid_cursor as isize;
        let before = c;
        if pressed(Key::ArrowRight) {
            c += 1;
        }
        if pressed(Key::ArrowLeft) {
            c -= 1;
        }
        if pressed(Key::ArrowDown) {
            c += cols;
        }
        if pressed(Key::ArrowUp) {
            c -= cols;
        }
        if pressed(Key::PageDown) {
            c += cols * 4;
        }
        if pressed(Key::PageUp) {
            c -= cols * 4;
        }
        if pressed(Key::Home) {
            c = 0;
        }
        if pressed(Key::End) {
            c = n - 1;
        }
        if c != before && n > 0 {
            self.grid_cursor = c.clamp(0, n - 1) as usize;
            self.grid_scroll = true;
        }
        let bigger = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Plus) || i.consume_key(Modifiers::NONE, Key::Equals));
        if bigger {
            self.grid_cell = (self.grid_cell * 1.2).min(420.0);
        }
        if pressed(Key::Minus) {
            self.grid_cell = (self.grid_cell / 1.2).max(90.0);
        }
        if pressed(Key::Enter) && n > 0 {
            self.grid = false;
            self.go_to(self.grid_cursor);
        }
        if pressed(Key::G) || pressed(Key::Escape) {
            self.grid = false;
        }
        if pressed(Key::T) {
            self.show_strip = !self.show_strip;
        }
        if pressed(Key::B) {
            self.open_batch();
        }
        if pressed(Key::F5) {
            self.start_slideshow(ctx);
        }
        if pressed(Key::F1) {
            self.show_help = !self.show_help;
        }
    }

    fn toggle_grid(&mut self) {
        self.grid = !self.grid;
        if self.grid {
            self.grid_cursor = self.index().unwrap_or(0);
            self.grid_scroll = true;
        }
    }

    fn start_slideshow(&mut self, ctx: &egui::Context) {
        if self.folder.is_empty() {
            self.status = "Open a folder of images first".into();
            return;
        }
        let from_grid = std::mem::take(&mut self.grid);
        let fs = is_fullscreen(ctx);
        if self.slide_fullscreen && !fs {
            ctx.send_viewport_cmd(ViewportCommand::Fullscreen(true));
        }
        self.slide = Some(Slideshow { shown: None, next_at: Instant::now(), paused: false, was_fullscreen: fs });
        if from_grid {
            self.go_to(self.grid_cursor);
        } else if self.index().is_none() {
            self.go_to(0);
        }
    }

    fn stop_slideshow(&mut self, ctx: &egui::Context) {
        if let Some(s) = self.slide.take()
            && self.slide_fullscreen
            && !s.was_fullscreen
        {
            ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false));
        }
    }

    /// Advances the slideshow. The timer starts once an image is on screen,
    /// so slow decodes don't eat into the display time.
    fn slideshow_tick(&mut self, ctx: &egui::Context) {
        let secs = Duration::from_secs_f32(self.slide_secs);
        let Some(s) = &mut self.slide else { return };
        let current = self.doc.as_ref().and_then(|d| d.path.clone());
        if current.is_none() || current != self.target {
            return; // still decoding; the loader wakes us
        }
        let now = Instant::now();
        if s.shown != current {
            s.shown = current;
            s.next_at = now + secs;
        }
        if s.paused {
            return;
        }
        if now < s.next_at {
            ctx.request_repaint_after(s.next_at - now);
            return;
        }
        s.next_at = now + secs;
        let at_end = self.index().is_some_and(|i| i + 1 == self.folder.len());
        if (at_end && !self.slide_loop) || self.doc.as_ref().is_some_and(|d| d.dirty) {
            self.stop_slideshow(ctx);
            self.status = "Slideshow finished".into();
        } else {
            self.navigate(1);
        }
    }

    /// Re-reads the current file after it was rewritten on disk.
    fn reload_from_disk(&mut self, path: &Path) {
        self.loader.invalidate(path);
        self.thumbs.invalidate(path);
        self.meta_info = None;
        match loader::decode(path) {
            Ok(d) => {
                self.doc = Some(Document::from_decoded(&Arc::new(d)));
                self.selection = None;
                self.view.fit = true;
            }
            Err(e) => self.load_error = Some(e),
        }
    }

    /// Removes EXIF/XMP/IPTC from the current file, overwriting it.
    fn strip_current(&mut self) {
        let Some(doc) = &self.doc else { return };
        let Some(path) = doc.path.clone() else {
            self.status = "Save the image first".into();
            return;
        };
        if !metadata::is_supported(&path) {
            self.status = "Metadata removal works on JPEG, PNG and WebP files".into();
            return;
        }
        if doc.dirty {
            // Our encoders write no metadata, so saving over the file strips it too.
            if let Some(f) = OutFormat::from_path(&path)
                && self.save_to(path.clone(), f)
            {
                self.status = format!("Saved {} with edits and without metadata", path.display());
            }
            return;
        }
        let t = Instant::now();
        match metadata::strip_file(&path, &path) {
            Ok(None) => self.status = "No metadata to remove".into(),
            Ok(Some(s)) => {
                // Pixels are unchanged; only the file and its listings are stale.
                self.loader.invalidate(&path);
                self.thumbs.invalidate(&path);
                self.meta_info = None;
                if let Some(d) = &mut self.doc {
                    d.file_size = s.data.len() as u64;
                }
                self.status = format!(
                    "Removed metadata, {} smaller ({:.0} ms){}",
                    human_size(s.removed as u64),
                    t.elapsed().as_secs_f64() * 1000.0,
                    if s.kept_orientation { ". Kept the rotation tag so it stays upright." } else { "" }
                );
            }
            Err(e) => self.status = format!("Metadata removal failed: {e}"),
        }
    }

    /// Renders the text preview when the text or its style changed.
    fn refresh_text_preview(&mut self, ctx: &egui::Context) {
        let color = rgba(self.primary);
        let (font_i, size) = (self.text_font, self.text_size);
        let Some(d) = &mut self.text_draft else { return };
        let key = (d.content.clone(), font_i, size.to_bits(), color);
        if d.preview.as_ref().is_some_and(|p| p.key == key) {
            return;
        }
        d.preview = text::render(&text::font(font_i), size, &d.content, color).map(|(img, ox, oy)| {
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            let tex = ctx.load_texture("text-preview", ci, egui::TextureOptions::LINEAR);
            TextPreview { key, img, ox, oy, tex }
        });
    }

    /// Draws the pending text into the image.
    fn commit_text(&mut self) {
        let ctx = self.ctx.clone();
        self.refresh_text_preview(&ctx);
        let Some(d) = self.text_draft.take() else { return };
        let Some(p) = d.preview else { return };
        let (x, y) = (d.pos.x.round() as i32 + p.ox, d.pos.y.round() as i32 + p.oy);
        if let Some(doc) = &mut self.doc {
            doc.checkpoint("Text");
        }
        self.paint_op(|img| text::blend(img, &p.img, x, y));
    }

    fn open_batch(&mut self) {
        self.batch.open = true;
        if self.batch.inputs.is_empty() {
            self.batch.inputs = self.folder.clone();
        }
        if self.batch.out_dir.is_empty()
            && let Some(d) = &self.folder_dir {
                self.batch.out_dir = d.join("converted").display().to_string();
            }
    }

    // -- UI -----------------------------------------------------------------

    fn menu_bar(&mut self, ui: &mut Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            let has_doc = self.doc.is_some();
            ui.menu_button("File", |ui| {
                if ui.add(egui::Button::new("Open…").shortcut_text(sc("Ctrl+O"))).clicked() {
                    self.open_dialog();
                }
                if ui.add(egui::Button::new("New image…").shortcut_text(sc("Ctrl+N"))).clicked() {
                    self.new_dlg = Some(NewDialog { w: 800, h: 600, transparent: false });
                }
                ui.separator();
                if ui.add_enabled(has_doc, egui::Button::new("Save").shortcut_text(sc("Ctrl+S"))).clicked() {
                    self.save();
                }
                if ui.add_enabled(has_doc, egui::Button::new("Save as…").shortcut_text(sc("Ctrl+Shift+S"))).clicked() {
                    self.save_as();
                }
                ui.menu_button("Save options", |ui| {
                    ui.add(egui::Slider::new(&mut self.save_opts.jpeg_quality, 1..=100).text("JPEG quality"));
                    ui.horizontal(|ui| {
                        ui.label("PNG compression:");
                        ui.selectable_value(&mut self.save_opts.png_level, PngLevel::Fast, "Fast");
                        ui.selectable_value(&mut self.save_opts.png_level, PngLevel::Default, "Default");
                        ui.selectable_value(&mut self.save_opts.png_level, PngLevel::Best, "Best");
                    });
                    ui.checkbox(&mut self.save_opts.webp_lossless, "WebP lossless");
                    ui.add_enabled(
                        !self.save_opts.webp_lossless,
                        egui::Slider::new(&mut self.save_opts.webp_quality, 1..=100).text("WebP quality"),
                    );
                });
                ui.separator();
                if ui.add(egui::Button::new("Batch conversion…").shortcut_text("B")).clicked() {
                    self.open_batch();
                }
                if ui.add(egui::Button::new("Batch remove metadata…")).clicked() {
                    self.open_batch();
                    self.batch.mode = BatchMode::StripMetadata;
                }
                let can_strip = self.doc.as_ref().and_then(|d| d.path.as_deref()).is_some_and(metadata::is_supported);
                if ui
                    .add_enabled(can_strip, egui::Button::new("Remove metadata (overwrite)").shortcut_text(sc("Ctrl+E")))
                    .on_hover_text("Removes EXIF, GPS, XMP and IPTC from the file without recompressing it")
                    .clicked()
                {
                    self.strip_current();
                }
                ui.separator();
                if ui.add_enabled(has_doc, egui::Button::new("Move to trash…").shortcut_text("Del")).clicked() {
                    self.confirm_delete = true;
                }
                if ui.add(egui::Button::new("Exit").shortcut_text("Esc")).clicked() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                }
            });
            ui.menu_button("Edit", |ui| {
                let (can_undo, can_redo) =
                    self.doc.as_ref().map_or((false, false), |d| (!d.undo.is_empty(), !d.redo.is_empty()));
                if ui.add_enabled(can_undo, egui::Button::new("Undo").shortcut_text(sc("Ctrl+Z"))).clicked() {
                    self.undo();
                }
                if ui.add_enabled(can_redo, egui::Button::new("Redo").shortcut_text(sc("Ctrl+Shift+Z"))).clicked() {
                    self.redo();
                }
                if ui.button("History…").on_hover_text("Shown in the edit panel").clicked() {
                    self.show_tools = true;
                }
                ui.separator();
                if ui.add_enabled(has_doc, egui::Button::new("Copy").shortcut_text(sc("Ctrl+C"))).clicked() {
                    self.copy();
                }
                if ui.add(egui::Button::new("Paste as new image").shortcut_text(sc("Ctrl+V"))).clicked() {
                    self.paste();
                }
                ui.separator();
                if ui.add_enabled(has_doc, egui::Button::new("Select all").shortcut_text(sc("Ctrl+A"))).clicked()
                    && let Some(d) = &self.doc {
                        self.selection = Some([0, 0, d.image.width(), d.image.height()]);
                    }
                if ui.add_enabled(self.selection.is_some(), egui::Button::new("Deselect").shortcut_text("Esc")).clicked() {
                    self.selection = None;
                }
            });
            ui.menu_button("Image", |ui| {
                ui.add_enabled_ui(has_doc, |ui| {
                    if ui.add(egui::Button::new("Rotate right").shortcut_text("R")).clicked() {
                        self.transform("Rotate right", edit::rotate_cw);
                    }
                    if ui.add(egui::Button::new("Rotate left").shortcut_text("L")).clicked() {
                        self.transform("Rotate left", edit::rotate_ccw);
                    }
                    if ui.button("Rotate 180°").clicked() {
                        self.modify("Rotate 180°", edit::rotate_180);
                    }
                    let can_lossless =
                        self.doc.as_ref().is_some_and(|d| !d.dirty && d.path.as_deref().is_some_and(jpegtran::is_jpeg));
                    ui.add_enabled_ui(can_lossless, |ui| {
                        ui.menu_button("Lossless JPEG transform", |ui| {
                            for op in jpegtran::Op::ALL {
                                let shortcut = match op {
                                    jpegtran::Op::RotateCw => sc("Ctrl+Shift+R"),
                                    jpegtran::Op::RotateCcw => sc("Ctrl+Shift+L"),
                                    _ => String::new(),
                                };
                                if ui.add(egui::Button::new(op.label()).shortcut_text(shortcut)).clicked() {
                                    self.lossless(op);
                                }
                            }
                            ui.weak("Rewrites the file without recompressing.");
                        });
                    });
                    if ui.add(egui::Button::new("Flip horizontal").shortcut_text("H")).clicked() {
                        self.modify("Flip horizontal", edit::flip_h);
                    }
                    if ui.add(egui::Button::new("Flip vertical").shortcut_text("V")).clicked() {
                        self.modify("Flip vertical", edit::flip_v);
                    }
                    ui.separator();
                    if ui.add(egui::Button::new("Crop to selection").shortcut_text(sc("Ctrl+Y"))).clicked() {
                        self.crop_to_selection();
                    }
                    if ui.add(egui::Button::new("Resize…").shortcut_text(sc("Ctrl+R"))).clicked() {
                        self.open_resize();
                    }
                    ui.separator();
                    if ui.add(egui::Button::new("Invert colors").shortcut_text(sc("Ctrl+Shift+I"))).clicked() {
                        self.modify("Invert colors", edit::invert);
                    }
                    if ui.add(egui::Button::new("Grayscale").shortcut_text(sc("Ctrl+G"))).clicked() {
                        self.modify("Grayscale", edit::grayscale);
                    }
                });
            });
            ui.menu_button("View", |ui| {
                if ui.add(egui::Button::new("Fit to window").shortcut_text("0 / F")).clicked() {
                    self.view.fit = true;
                }
                if ui.add(egui::Button::new("Actual size (100%)").shortcut_text("1")).clicked() {
                    self.set_zoom(1.0);
                }
                ui.checkbox(&mut self.fit_upscale, "Fit enlarges small images");
                let mut px = self.canvas.pixelated;
                if ui.checkbox(&mut px, "Pixelated zoom (nearest neighbour)").changed() {
                    self.canvas.pixelated = px;
                    self.uploaded_version = 0;
                }
                ui.separator();
                if ui.add(egui::Button::new("Full screen").shortcut_text("Enter / F11")).clicked() {
                    toggle_fullscreen(ui.ctx());
                }
                ui.checkbox(&mut self.show_strip, "Thumbnail strip (T)");
                let mut grid = self.grid;
                if ui.checkbox(&mut grid, "Grid view (G)").changed() {
                    self.toggle_grid();
                }
                ui.menu_button("Slideshow", |ui| {
                    let ctx = ui.ctx().clone();
                    if self.slide.is_some() {
                        if ui.add(egui::Button::new("Stop slideshow").shortcut_text("F5 / Esc")).clicked() {
                            self.stop_slideshow(&ctx);
                        }
                    } else if ui.add(egui::Button::new("Start slideshow").shortcut_text("F5")).clicked() {
                        self.start_slideshow(&ctx);
                    }
                    ui.add(egui::Slider::new(&mut self.slide_secs, 0.5..=60.0).logarithmic(true).suffix(" s").text("Delay"));
                    ui.checkbox(&mut self.slide_loop, "Loop");
                    ui.checkbox(&mut self.slide_fullscreen, "Full screen");
                    ui.weak("Space pauses, arrows step.");
                });
                if ui.add(egui::Button::new("Image info").shortcut_text("I")).clicked() {
                    self.show_info = !self.show_info;
                }
                if ui.checkbox(&mut self.show_tools, "Paint tools panel (E)").changed() && !self.show_tools {
                    self.tool = Tool::Hand;
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.add(egui::Button::new("Keyboard shortcuts").shortcut_text("F1")).clicked() {
                    self.show_help = true;
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if ui.button("Open").on_hover_text("Open (Ctrl+O)").clicked() {
                self.open_dialog();
            }
            if ui.add_enabled(self.doc.is_some(), egui::Button::new("Save")).on_hover_text("Save (Ctrl+S)").clicked() {
                self.save();
            }
            ui.separator();
            if ui.button("< Prev").on_hover_text("Previous (←)").clicked() {
                self.navigate(-1);
            }
            if ui.button("Next >").on_hover_text("Next (→)").clicked() {
                self.navigate(1);
            }
            ui.separator();
            let ctx = ui.ctx().clone();
            let area = ctx.content_rect();
            let ppp = ctx.pixels_per_point();
            if ui.button(" + ").on_hover_text("Zoom in (+)").clicked() {
                self.zoom_about(area, area.center(), 1.25, ppp);
            }
            if ui.button(" − ").on_hover_text("Zoom out (−)").clicked() {
                self.zoom_about(area, area.center(), 0.8, ppp);
            }
            if ui.selectable_label(self.view.fit, "Fit").on_hover_text("Fit to window (0)").clicked() {
                self.view.fit = true;
            }
            if ui.button("1:1").on_hover_text("Actual size (1)").clicked() {
                self.set_zoom(1.0);
            }
            ui.separator();
            ui.add_enabled_ui(self.doc.is_some(), |ui| {
                if ui.button("Rotate L").on_hover_text("Rotate left (L)").clicked() {
                    self.transform("Rotate left", edit::rotate_ccw);
                }
                if ui.button("Rotate R").on_hover_text("Rotate right (R)").clicked() {
                    self.transform("Rotate right", edit::rotate_cw);
                }
                if ui.button("Flip H").on_hover_text("Flip horizontal (H)").clicked() {
                    self.modify("Flip horizontal", edit::flip_h);
                }
                if ui.button("Flip V").on_hover_text("Flip vertical (V)").clicked() {
                    self.modify("Flip vertical", edit::flip_v);
                }
                if ui.button("Crop").on_hover_text("Crop to selection (Ctrl+Y)").clicked() {
                    self.crop_to_selection();
                }
                if ui.button("Resize").on_hover_text("Resize (Ctrl+R)").clicked() {
                    self.open_resize();
                }
                ui.separator();
                if ui.button("Undo").on_hover_text("Undo (Ctrl+Z)").clicked() {
                    self.undo();
                }
                if ui.button("Redo").on_hover_text("Redo (Ctrl+Shift+Z)").clicked() {
                    self.redo();
                }
            });
            ui.separator();
            if ui.selectable_label(self.show_tools, "Edit").on_hover_text("Paint tools (E)").clicked() {
                self.show_tools = !self.show_tools;
                if !self.show_tools {
                    self.tool = Tool::Hand;
                }
            }
            if ui.button("Batch").on_hover_text("Batch conversion (B)").clicked() {
                self.open_batch();
            }
            ui.separator();
            if ui.selectable_label(self.show_strip, "Thumbs").on_hover_text("Thumbnail strip (T)").clicked() {
                self.show_strip = !self.show_strip;
            }
            if ui.selectable_label(self.grid, "Grid").on_hover_text("Grid view (G)").clicked() {
                self.toggle_grid();
            }
            if ui.button("Slideshow").on_hover_text("Slideshow (F5)").clicked() {
                let ctx = ui.ctx().clone();
                self.start_slideshow(&ctx);
            }
        });
    }

    fn tools_panel(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        for t in Tool::ALL {
            if ui.selectable_label(self.tool == t, t.label()).clicked() {
                self.tool = t;
            }
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.color_edit_button_srgba(&mut self.primary).on_hover_text("Primary (left click)");
            ui.color_edit_button_srgba(&mut self.secondary).on_hover_text("Secondary (right click)");
            if ui.small_button("Swap").on_hover_text("Swap colors").clicked() {
                std::mem::swap(&mut self.primary, &mut self.secondary);
            }
        });
        ui.horizontal_wrapped(|ui| {
            for c in [
                Color32::BLACK,
                Color32::WHITE,
                Color32::from_gray(128),
                Color32::RED,
                Color32::from_rgb(255, 140, 0),
                Color32::YELLOW,
                Color32::from_rgb(0, 200, 0),
                Color32::from_rgb(0, 200, 255),
                Color32::BLUE,
                Color32::from_rgb(160, 0, 220),
                Color32::from_rgb(255, 0, 200),
                Color32::from_rgb(120, 70, 20),
            ] {
                let (r, resp) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
                ui.painter().rect_filled(r, 2.0, c);
                ui.painter().rect_stroke(r, 2.0, Stroke::new(1.0, Color32::from_gray(90)), StrokeKind::Inside);
                if resp.clicked() {
                    self.primary = c;
                }
                if resp.secondary_clicked() {
                    self.secondary = c;
                }
            }
        });
        ui.separator();
        if self.tool.uses_size() {
            ui.add(egui::Slider::new(&mut self.size, 1.0..=200.0).logarithmic(true).text("Size"));
        }
        if matches!(self.tool, Tool::Rect | Tool::Ellipse) {
            ui.radio_value(&mut self.shape_style, ShapeStyle::Outline, "Outline");
            ui.radio_value(&mut self.shape_style, ShapeStyle::Filled, "Filled");
            ui.radio_value(&mut self.shape_style, ShapeStyle::OutlineAndFill, "Outline + fill");
        }
        if self.tool == Tool::Fill {
            ui.add(egui::Slider::new(&mut self.tolerance, 0..=255).text("Tolerance"));
        }
        if self.tool == Tool::Eraser {
            ui.checkbox(&mut self.erase_transparent, "Erase to transparent");
        }
        if self.tool == Tool::Text {
            self.text_options(ui);
        }
        ui.add_space(8.0);
        ui.weak(match self.tool {
            Tool::Hand => "Drag to pan, wheel to zoom.",
            Tool::Select => "Drag to select. Ctrl+Y crops, Ctrl+C copies.",
            Tool::Picker => "Left: pick primary. Right: pick secondary.",
            Tool::Line | Tool::Rect | Tool::Ellipse => "Shift constrains. Right button uses secondary color.",
            Tool::Text => "Click to place, click again to move. Uses the primary color.",
            _ => "Left: primary color. Right: secondary.",
        });
        self.history_ui(ui);
    }

    fn text_options(&mut self, ui: &mut Ui) {
        let fonts = text::choices();
        ui.label("Font");
        egui::ComboBox::from_id_salt("font")
            .width(150.0)
            .selected_text(fonts.get(self.text_font).map_or("", |f| f.name))
            .show_ui(ui, |ui| {
                for (i, f) in fonts.iter().enumerate() {
                    ui.selectable_value(&mut self.text_font, i, f.name);
                }
            });
        ui.spacing_mut().slider_width = 90.0;
        ui.add(egui::Slider::new(&mut self.text_size, 6.0..=1000.0).logarithmic(true).text("px"));
        let (mut apply, mut cancel) = (false, false);
        if let Some(d) = &mut self.text_draft {
            let id = ui.make_persistent_id("text-tool-input");
            // Ctrl+Enter applies; take it before the text field turns it into a newline.
            let focused = ui.memory(|m| m.has_focus(id));
            apply = focused && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
            let resp = ui.add(
                egui::TextEdit::multiline(&mut d.content)
                    .id(id)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("Type here"),
            );
            if std::mem::take(&mut self.text_focus) {
                resp.request_focus();
            }
            ui.horizontal(|ui| {
                apply |= ui.button("Apply").on_hover_text(sc("Ctrl+Enter")).clicked();
                cancel = ui.button("Cancel").clicked();
            });
        } else {
            ui.weak("Click on the image to place text.");
        }
        if apply {
            self.commit_text();
        } else if cancel {
            self.text_draft = None;
        }
    }

    /// Undo history: click a step to go back (or forward) to it.
    fn history_ui(&mut self, ui: &mut Ui) {
        let Some(doc) = &self.doc else { return };
        ui.separator();
        ui.strong("History");
        let n = doc.undo.len();
        let mut jump = None;
        egui::ScrollArea::vertical().id_salt("history").max_height(240.0).auto_shrink([false, true]).show(ui, |ui| {
            if ui.selectable_label(n == 0, "Start").clicked() {
                jump = Some(0);
            }
            for (i, s) in doc.undo.iter().enumerate() {
                if ui.selectable_label(i + 1 == n, s.label).clicked() {
                    jump = Some(i + 1);
                }
            }
            // Undone steps, greyed out, in the order they'd be redone.
            for (k, s) in doc.redo.iter().rev().enumerate() {
                if ui.selectable_label(false, egui::RichText::new(s.label).weak()).clicked() {
                    jump = Some(n + k + 1);
                }
            }
        });
        if let Some(j) = jump {
            self.history_jump(j);
        }
    }

    fn strip_ui(&mut self, ui: &mut Ui) {
        const CELL: f32 = 92.0;
        let n = self.folder.len();
        let current = self.index();
        let scroll_to = current.filter(|_| self.strip_scrolled.as_ref() != self.target.as_ref());
        if scroll_to.is_some() {
            self.strip_scrolled = self.target.clone();
        }
        let mut clicked = None;
        // The mouse wheel scrolls the strip sideways.
        ui.style_mut().always_scroll_the_only_direction = true;
        egui::ScrollArea::horizontal().id_salt("strip").auto_shrink([false, false]).show_viewport(ui, |ui, vp| {
            let height = ui.available_height();
            let (rect, _) = ui.allocate_exact_size(Vec2::new(n as f32 * CELL, height), Sense::hover());
            let first = ((vp.min.x / CELL).floor() as isize - 2).max(0) as usize;
            let last = ((vp.max.x / CELL).ceil() as usize + 2).min(n);
            let cell_rect = |i: usize| Rect::from_min_size(rect.min + Vec2::new(i as f32 * CELL, 0.0), Vec2::new(CELL, height));
            for i in first..last {
                let r = cell_rect(i);
                let resp = ui.interact(r, ui.scope_id().with(("strip", i)), Sense::click());
                draw_thumb(ui, &mut self.thumbs, &self.folder[i], r.shrink(5.0), current == Some(i), resp.hovered());
                if resp.on_hover_text(file_name(&self.folder[i])).clicked() {
                    clicked = Some(i);
                }
            }
            if let Some(i) = scroll_to {
                ui.scroll_to_rect(cell_rect(i), Some(egui::Align::Center));
            }
        });
        if let Some(i) = clicked {
            self.go_to(i);
        }
    }

    fn grid_ui(&mut self, ui: &mut Ui) {
        let area = ui.max_rect();
        ui.painter().rect_filled(area, 0.0, BG);
        let n = self.folder.len();
        if n == 0 {
            let msg = if self.folder_rx.is_some() { "Reading folder…" } else { "No images in this folder" };
            ui.painter().text(area.center(), egui::Align2::CENTER_CENTER, msg, egui::FontId::proportional(16.0), Color32::from_gray(160));
            return;
        }
        let cell = self.grid_cell;
        let row_h = cell + 18.0;
        let cols = ((area.width() - 16.0) / cell).floor().max(1.0) as usize;
        self.grid_cols = cols;
        let rows = n.div_ceil(cols);
        let pad = ((area.width() - cols as f32 * cell) / 2.0).max(0.0);
        let scroll = std::mem::take(&mut self.grid_scroll);
        let (mut clicked, mut opened) = (None, None);
        egui::ScrollArea::vertical().id_salt("grid").auto_shrink([false, false]).show_viewport(ui, |ui, vp| {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), rows as f32 * row_h + 16.0), Sense::hover());
            let cell_rect = |i: usize| {
                let (row, col) = (i / cols, i % cols);
                Rect::from_min_size(rect.min + Vec2::new(pad + col as f32 * cell, 8.0 + row as f32 * row_h), Vec2::new(cell, row_h))
            };
            let first = ((vp.min.y / row_h).floor() as isize - 1).max(0) as usize;
            let last = ((vp.max.y / row_h).ceil() as usize + 1).min(rows);
            for i in first * cols..(last * cols).min(n) {
                let r = cell_rect(i);
                let resp = ui.interact(r, ui.scope_id().with(("grid", i)), Sense::click());
                let thumb = Rect::from_min_size(r.min, Vec2::splat(cell)).shrink(10.0);
                draw_thumb(ui, &mut self.thumbs, &self.folder[i], thumb, i == self.grid_cursor, resp.hovered());
                let font = egui::FontId::proportional(12.0);
                let name = elide(ui.painter(), &file_name(&self.folder[i]), &font, cell - 12.0);
                ui.painter().text(
                    Pos2::new(r.center().x, r.max.y - 12.0),
                    egui::Align2::CENTER_CENTER,
                    name,
                    font,
                    Color32::from_gray(if i == self.grid_cursor { 235 } else { 170 }),
                );
                if resp.double_clicked() {
                    opened = Some(i);
                } else if resp.clicked() {
                    clicked = Some(i);
                }
            }
            if scroll {
                ui.scroll_to_rect(cell_rect(self.grid_cursor), None);
            }
        });
        if let Some(i) = clicked {
            self.grid_cursor = i;
        }
        if let Some(i) = opened {
            self.grid = false;
            self.go_to(i);
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if let Some(doc) = &self.doc {
                if let Some(i) = self.index() {
                    ui.label(format!("{}/{}", i + 1, self.folder.len()));
                    ui.separator();
                }
                let (w, h) = doc.image.dimensions();
                ui.label(format!("{}{}", doc.name(), if doc.dirty { " *" } else { "" }));
                ui.separator();
                ui.label(format!("{w} × {h}"));
                if doc.file_size > 0 {
                    ui.separator();
                    ui.label(human_size(doc.file_size));
                }
                ui.separator();
                ui.label(format!("{:.0}%", self.view.zoom * 100.0));
                if let Some((x, y)) = self.hover {
                    let p = doc.image.get_pixel(x, y).0;
                    ui.separator();
                    ui.label(format!("{x}, {y}  rgba({}, {}, {}, {})", p[0], p[1], p[2], p[3]));
                }
                if let Some([_, _, sw, sh]) = self.selection {
                    ui.separator();
                    ui.label(format!("sel {sw} × {sh}"));
                }
            } else if self.target.is_some() && self.load_error.is_none() {
                ui.label("Loading…");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(d) = &self.doc
                    && d.decode_ms > 0.0 {
                        ui.weak(format!("decoded in {:.0} ms", d.decode_ms));
                    }
                if !self.status.is_empty() {
                    ui.label(&self.status);
                }
            });
        });
    }

    fn canvas_ui(&mut self, ui: &mut Ui) {
        let area = ui.max_rect();
        // Not focusable: focus would let egui use arrow keys for widget navigation.
        let resp = ui.allocate_rect(area, Sense::CLICK | Sense::DRAG);
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, BG);
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point();
        self.refresh_text_preview(&ctx);

        let Some(doc) = &self.doc else {
            let msg = if let Some(e) = &self.load_error {
                format!("Could not open image\n{e}")
            } else if self.target.is_some() {
                String::new() // decoding; keep the screen calm
            } else {
                "Open an image (Ctrl+O), paste one (Ctrl+V), or drop a file here.".into()
            };
            painter.text(area.center(), egui::Align2::CENTER_CENTER, msg, egui::FontId::proportional(16.0), Color32::from_gray(160));
            if resp.double_clicked() {
                self.open_dialog();
            }
            return;
        };

        let (iw, ih) = doc.image.dimensions();
        if self.view.fit {
            self.view.zoom = self.fit_zoom(area, iw, ih, ppp);
            self.view.pan = Vec2::ZERO;
        }

        // Zoom with wheel / pinch around the cursor.
        if let Some(pos) = resp.hover_pos() {
            let (scroll, pinch) = ctx.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = pinch * (scroll * 0.0025).exp();
            if (factor - 1.0).abs() > 1e-4 {
                self.zoom_about(area, pos, factor, ppp);
            }
        }

        // Pan.
        let pan_drag = resp.dragged_by(PointerButton::Middle)
            || (self.tool == Tool::Hand && resp.dragged_by(PointerButton::Primary))
            || (ctx.input(|i| i.key_down(Key::Space)) && resp.dragged_by(PointerButton::Primary));
        if pan_drag {
            self.view.pan += resp.drag_delta();
            self.view.fit = false;
        }
        if self.tool == Tool::Hand && resp.double_clicked() {
            toggle_fullscreen(&ctx);
        }
        if resp.clicked_by(PointerButton::Extra1) {
            self.navigate(-1);
        }
        if resp.clicked_by(PointerButton::Extra2) {
            self.navigate(1);
        }

        let doc = self.doc.as_ref().unwrap();
        let img_rect = self.image_rect(area, iw, ih, ppp);
        let scale = self.view.zoom / ppp;
        let to_img = |p: Pos2| Pos2::new((p.x - img_rect.min.x) / scale, (p.y - img_rect.min.y) / scale);
        let to_screen = |p: Pos2| img_rect.min + p.to_vec2() * scale;

        // Background for transparency, then the image itself.
        if doc.has_alpha {
            let checker = self.checker.get_or_insert_with(|| checker_texture(&ctx));
            let vis = img_rect.intersect(area);
            let cell = 8.0;
            let uv = Rect::from_min_max(
                ((vis.min - img_rect.min) / (cell * 2.0)).to_pos2(),
                ((vis.max - img_rect.min) / (cell * 2.0)).to_pos2(),
            );
            painter.image(checker.id(), vis, uv, Color32::WHITE);
        }
        self.canvas.paint(&painter, img_rect, iw, ih);

        // Pointer state.
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        self.hover = pointer.filter(|p| img_rect.contains(*p) && area.contains(*p)).map(|p| {
            let ip = to_img(p);
            ((ip.x as u32).min(iw - 1), (ip.y as u32).min(ih - 1))
        });

        let drawing_tool = self.tool != Tool::Hand && !ctx.input(|i| i.key_down(Key::Space));
        let (pressed_l, pressed_r) =
            ctx.input(|i| (i.pointer.button_pressed(PointerButton::Primary), i.pointer.button_pressed(PointerButton::Secondary)));
        if self.stroke.is_none() && drawing_tool && resp.hovered() && (pressed_l || pressed_r)
            && let Some(pos) = pointer {
                let button = if pressed_l { PointerButton::Primary } else { PointerButton::Secondary };
                self.begin_stroke(to_img(pos), button);
            }
        if let Some(pos) = pointer {
            self.continue_stroke(&ctx, to_img(pos));
        }

        // Overlays: shape preview, selection, brush cursor.
        let color = |b: PointerButton, app: &App| if b == PointerButton::Primary { app.primary } else { app.secondary };
        if let Some(st) = &self.stroke {
            let (a, b) = (st.start, st.last);
            let c = color(st.button, self);
            let other = color(if st.button == PointerButton::Primary { PointerButton::Secondary } else { PointerButton::Primary }, self);
            let t = self.size.max(1.0) * scale;
            let snapped = Rect::from_min_max(
                to_screen(Pos2::new(a.x.min(b.x).floor(), a.y.min(b.y).floor())),
                to_screen(Pos2::new(a.x.max(b.x).floor() + 1.0, a.y.max(b.y).floor() + 1.0)),
            );
            match st.tool {
                Tool::Line => {
                    painter.line_segment([to_screen(a), to_screen(b)], Stroke::new(t, c));
                }
                Tool::Rect => {
                    let (o, f) = self.shape_colors(c, other);
                    if let Some(f) = f {
                        painter.rect_filled(snapped, 0.0, f);
                    }
                    if let Some(o) = o {
                        painter.rect_stroke(snapped, 0.0, Stroke::new(t, o), StrokeKind::Inside);
                    }
                }
                Tool::Ellipse => {
                    let (o, f) = self.shape_colors(c, other);
                    let r = snapped.size() / 2.0;
                    if let Some(f) = f {
                        painter.add(egui::Shape::ellipse_filled(snapped.center(), r, f));
                    }
                    if let Some(o) = o {
                        let inset = (r - Vec2::splat(t / 2.0)).max(Vec2::ZERO);
                        painter.add(egui::Shape::ellipse_stroke(snapped.center(), inset, Stroke::new(t, o)));
                    }
                }
                Tool::Select => {
                    dashed_rect(&painter, snapped);
                }
                _ => {}
            }
        }
        if let Some(d) = &self.text_draft {
            let origin = to_screen(Pos2::new(d.pos.x.round(), d.pos.y.round()));
            if let Some(p) = &d.preview {
                let r = Rect::from_min_size(
                    origin + Vec2::new(p.ox as f32, p.oy as f32) * scale,
                    Vec2::new(p.img.width() as f32, p.img.height() as f32) * scale,
                );
                painter.image(p.tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                dashed_rect(&painter, r.expand(3.0));
            }
            // Caret at the text origin.
            let caret = [origin, origin + Vec2::new(0.0, self.text_size * scale)];
            painter.line_segment(caret, Stroke::new(3.0, Color32::BLACK));
            painter.line_segment(caret, Stroke::new(1.0, Color32::WHITE));
        }
        if let (Some([x, y, w, h]), None) = (self.selection, &self.stroke) {
            let r = Rect::from_min_max(
                to_screen(Pos2::new(x as f32, y as f32)),
                to_screen(Pos2::new((x + w) as f32, (y + h) as f32)),
            );
            dashed_rect(&painter, r);
        }
        if matches!(self.tool, Tool::Brush | Tool::Eraser) && drawing_tool {
            if let Some(p) = pointer.filter(|p| area.contains(*p)) {
                let r = (self.size * scale / 2.0).max(2.0);
                painter.circle_stroke(p, r, Stroke::new(1.0, Color32::from_black_alpha(180)));
                painter.circle_stroke(p, r + 1.0, Stroke::new(1.0, Color32::from_white_alpha(180)));
                ctx.set_cursor_icon(egui::CursorIcon::None);
            }
        } else if drawing_tool && resp.hovered() {
            ctx.set_cursor_icon(if self.tool == Tool::Text { egui::CursorIcon::Text } else { egui::CursorIcon::Crosshair });
        } else if pan_drag {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }

        if self.slide.as_ref().is_some_and(|s| s.paused) {
            let pos = Pos2::new(area.center().x, area.max.y - 40.0);
            let font = egui::FontId::proportional(16.0);
            let msg = "Slideshow paused: Space resumes, Esc stops";
            let galley = painter.layout_no_wrap(msg.into(), font.clone(), Color32::WHITE);
            painter.rect_filled(Rect::from_center_size(pos, galley.size() + Vec2::new(24.0, 12.0)), 6.0, Color32::from_black_alpha(170));
            painter.text(pos, egui::Align2::CENTER_CENTER, msg, font, Color32::WHITE);
        }

        if self.tool == Tool::Hand {
            resp.context_menu(|ui| {
                if ui.button("Copy").clicked() {
                    self.copy();
                }
                if ui.button("Rotate right").clicked() {
                    self.transform("Rotate right", edit::rotate_cw);
                }
                if ui.button("Rotate left").clicked() {
                    self.transform("Rotate left", edit::rotate_ccw);
                }
                if ui.button("Fit to window").clicked() {
                    self.view.fit = true;
                }
                if ui.button("Full screen").clicked() {
                    toggle_fullscreen(ui.ctx());
                }
            });
        }
    }

    /// Advances animated images and schedules the next frame.
    fn animate(&mut self, ctx: &egui::Context) {
        let Some(doc) = &mut self.doc else { return };
        // Full RAW development finished: swap it in (same view, no edits yet).
        if let Some(full) = doc.source.as_ref().and_then(|s| s.refined.get())
            && !Arc::ptr_eq(full, &doc.image)
        {
            // Keep the on-screen size when the resolution jumps.
            let ratio = doc.image.width() as f32 / full.width() as f32;
            doc.image = full.clone();
            doc.version = next_version();
            if let Some(s) = &doc.source {
                doc.format = s.format.replace("embedded preview", "developed");
            }
            if !self.view.fit {
                self.view.zoom *= ratio;
            }
            return;
        }
        let Some(frames) = doc.source.as_ref().and_then(|s| s.frames.get()) else { return };
        let now = Instant::now();
        let anim = doc.anim.get_or_insert_with(|| Anim { frame: 0, next_at: now + frames[0].delay, playing: true });
        if !anim.playing {
            return;
        }
        if now >= anim.next_at {
            anim.frame = (anim.frame + 1) % frames.len();
            let f = &frames[anim.frame];
            // Late by more than a frame (e.g. window was hidden): resync.
            anim.next_at = if now > anim.next_at + f.delay { now + f.delay } else { anim.next_at + f.delay };
            doc.image = f.image.clone();
            doc.version = next_version();
            // Same size every frame: update the existing textures in place.
            let (w, h) = doc.image.dimensions();
            self.canvas.update_region(&doc.image, 0, 0, w, h);
            self.uploaded_version = doc.version;
        }
        ctx.request_repaint_after(anim.next_at.saturating_duration_since(now));
    }

    fn toggle_playback(&mut self) {
        if let Some(a) = self.doc.as_mut().and_then(|d| d.anim.as_mut()) {
            a.playing = !a.playing;
            a.next_at = Instant::now();
        }
    }

    /// Lossless JPEG rotate/flip on the file itself (no recompression).
    fn lossless(&mut self, op: jpegtran::Op) {
        let Some(doc) = &self.doc else { return };
        let Some(path) = doc.path.clone().filter(|p| jpegtran::is_jpeg(p)) else {
            self.status = "Lossless rotation works on JPEG files".into();
            return;
        };
        if doc.dirty {
            self.status = "Save or undo your edits first: lossless rotation works on the file".into();
            return;
        }
        let t = Instant::now();
        match jpegtran::transform_file(&path, op) {
            Ok(trimmed) => {
                self.reload_from_disk(&path);
                self.status = format!(
                    "Lossless {} saved ({:.0} ms){}",
                    op.label().to_lowercase(),
                    t.elapsed().as_secs_f64() * 1000.0,
                    if trimmed { ". Trimmed the edge to whole JPEG blocks." } else { "" }
                );
            }
            Err(e) => self.status = format!("Lossless transform failed: {e}"),
        }
    }

    fn shape_colors(&self, c: Color32, other: Color32) -> (Option<Color32>, Option<Color32>) {
        match self.shape_style {
            ShapeStyle::Outline => (Some(c), None),
            ShapeStyle::Filled => (None, Some(c)),
            ShapeStyle::OutlineAndFill => (Some(c), Some(other)),
        }
    }

    fn begin_stroke(&mut self, p: Pos2, button: PointerButton) {
        if self.doc.is_none() {
            return;
        }
        let primary = button == PointerButton::Primary;
        let c = rgba(if primary { self.primary } else { self.secondary });
        match self.tool {
            Tool::Picker => {
                let doc = self.doc.as_ref().unwrap();
                let (w, h) = doc.image.dimensions();
                if p.x >= 0.0 && p.y >= 0.0 && (p.x as u32) < w && (p.y as u32) < h {
                    let px = doc.image.get_pixel(p.x as u32, p.y as u32).0;
                    let col = Color32::from_rgba_unmultiplied(px[0], px[1], px[2], px[3]);
                    if primary {
                        self.primary = col;
                    } else {
                        self.secondary = col;
                    }
                }
            }
            Tool::Fill => {
                if p.x < 0.0 || p.y < 0.0 {
                    return;
                }
                self.checkpoint();
                let tol = self.tolerance;
                self.paint_op(|img| paint::flood_fill(img, p.x as u32, p.y as u32, c, tol));
            }
            Tool::Brush | Tool::Eraser => {
                self.checkpoint();
                let c = self.brush_color(button);
                let size = self.size;
                if self.tool == Tool::Eraser && self.erase_transparent {
                    self.doc.as_mut().unwrap().has_alpha = true;
                }
                self.paint_op(|img| paint::segment(img, p, p, size, c));
                self.stroke = Some(StrokeState { tool: self.tool, button, start: p, last: p });
            }
            Tool::Text => {
                match &mut self.text_draft {
                    Some(d) => d.pos = p,
                    None => self.text_draft = Some(TextDraft { pos: p, content: String::new(), preview: None }),
                }
                self.text_focus = true;
            }
            Tool::Select | Tool::Line | Tool::Rect | Tool::Ellipse => {
                self.selection = None;
                self.stroke = Some(StrokeState { tool: self.tool, button, start: p, last: p });
            }
            Tool::Hand => {}
        }
    }

    fn brush_color(&self, button: PointerButton) -> [u8; 4] {
        let primary = button == PointerButton::Primary;
        if self.tool == Tool::Eraser {
            if self.erase_transparent {
                return [0, 0, 0, 0];
            }
            // Like classic Paint, the eraser paints with the secondary color.
            return rgba(if primary { self.secondary } else { self.primary });
        }
        rgba(if primary { self.primary } else { self.secondary })
    }

    fn continue_stroke(&mut self, ctx: &egui::Context, mut p: Pos2) {
        let Some(st) = &self.stroke else { return };
        let (tool, button, start, last) = (st.tool, st.button, st.start, st.last);
        let shift = ctx.input(|i| i.modifiers.shift);
        if shift {
            p = constrain(tool, start, p);
        }
        match tool {
            Tool::Brush | Tool::Eraser
                if p != last => {
                    let c = self.brush_color(button);
                    let size = self.size;
                    self.paint_op(|img| paint::segment(img, last, p, size, c));
                }
            _ => {}
        }
        if let Some(st) = &mut self.stroke {
            st.last = p;
        }
        if ctx.input(|i| i.pointer.button_down(button)) {
            return;
        }

        // Released: commit.
        self.stroke = None;
        let primary = button == PointerButton::Primary;
        let (c, other) = if primary { (self.primary, self.secondary) } else { (self.secondary, self.primary) };
        let (o, f) = self.shape_colors(c, other);
        let (o, f) = (o.map(rgba), f.map(rgba));
        let size = self.size;
        match tool {
            Tool::Line => {
                self.checkpoint();
                self.paint_op(|img| paint::segment(img, start, p, size, rgba(c)));
            }
            Tool::Rect => {
                self.checkpoint();
                self.paint_op(|img| paint::rect(img, start, p, size, o, f));
            }
            Tool::Ellipse => {
                self.checkpoint();
                self.paint_op(|img| paint::ellipse(img, start, p, size, o, f));
            }
            Tool::Select => {
                let Some(doc) = &self.doc else { return };
                let (w, h) = doc.image.dimensions();
                let x0 = start.x.min(p.x).floor().clamp(0.0, w as f32) as u32;
                let y0 = start.y.min(p.y).floor().clamp(0.0, h as f32) as u32;
                let x1 = (start.x.max(p.x).floor() + 1.0).clamp(0.0, w as f32) as u32;
                let y1 = (start.y.max(p.y).floor() + 1.0).clamp(0.0, h as f32) as u32;
                self.selection = (x1 > x0 + 1 && y1 > y0 + 1).then_some([x0, y0, x1 - x0, y1 - y0]);
            }
            _ => {}
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        // Resize.
        if let Some(dlg) = &mut self.resize_dlg {
            let mut open = true;
            let mut apply = false;
            egui::Window::new("Resize image").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                let (ow, oh) = dlg.orig;
                ui.label(format!("Current size: {ow} × {oh}"));
                egui::Grid::new("resize").num_columns(2).show(ui, |ui| {
                    ui.label("Width");
                    if ui.add(egui::DragValue::new(&mut dlg.w).range(1..=65535)).changed() && dlg.keep_aspect {
                        dlg.h = ((dlg.w as f64 * oh as f64 / ow as f64).round() as u32).max(1);
                    }
                    ui.end_row();
                    ui.label("Height");
                    if ui.add(egui::DragValue::new(&mut dlg.h).range(1..=65535)).changed() && dlg.keep_aspect {
                        dlg.w = ((dlg.h as f64 * ow as f64 / oh as f64).round() as u32).max(1);
                    }
                    ui.end_row();
                });
                ui.checkbox(&mut dlg.keep_aspect, "Keep aspect ratio");
                ui.horizontal(|ui| {
                    for pct in [25, 50, 75, 150, 200] {
                        if ui.button(format!("{pct}%")).clicked() {
                            dlg.w = ((ow * pct) as f32 / 100.0).round().max(1.0) as u32;
                            dlg.h = ((oh * pct) as f32 / 100.0).round().max(1.0) as u32;
                        }
                    }
                });
                egui::ComboBox::from_label("Filter").selected_text(dlg.filter.label()).show_ui(ui, |ui| {
                    for f in ResizeFilter::ALL {
                        ui.selectable_value(&mut dlg.filter, f, f.label());
                    }
                });
                ui.add_space(6.0);
                apply = ui.button("Apply").clicked();
            });
            if apply {
                let (w, h, f) = (dlg.w, dlg.h, dlg.filter);
                self.resize_dlg = None;
                self.transform("Resize", |img| edit::resize(img, w, h, f));
            } else if !open {
                self.resize_dlg = None;
            }
        }

        // New image.
        if let Some(dlg) = &mut self.new_dlg {
            let mut open = true;
            let mut create = false;
            egui::Window::new("New image").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                egui::Grid::new("new").num_columns(2).show(ui, |ui| {
                    ui.label("Width");
                    ui.add(egui::DragValue::new(&mut dlg.w).range(1..=32768));
                    ui.end_row();
                    ui.label("Height");
                    ui.add(egui::DragValue::new(&mut dlg.h).range(1..=32768));
                    ui.end_row();
                });
                ui.checkbox(&mut dlg.transparent, "Transparent background (else secondary color)");
                create = ui.button("Create").clicked();
            });
            if create {
                let fill = if dlg.transparent { [0, 0, 0, 0] } else { rgba(self.secondary) };
                let img = RgbaImage::from_pixel(dlg.w, dlg.h, image::Rgba(fill));
                self.new_dlg = None;
                self.show_tools = true;
                if self.tool == Tool::Hand {
                    self.tool = Tool::Brush;
                }
                self.replace_doc(Document::untitled(img));
            } else if !open {
                self.new_dlg = None;
            }
        }

        self.batch_window(ctx);

        if self.show_info
            && let Some(p) = self.doc.as_ref().and_then(|d| d.path.clone())
            && self.meta_info.as_ref().is_none_or(|m| m.0 != p)
        {
            let found = if metadata::is_supported(&p) {
                let kinds = std::fs::read(&p).map(|d| metadata::scan(&d)).unwrap_or_default();
                if kinds.is_empty() { "none".to_string() } else { kinds.join(", ") }
            } else {
                "not checked for this format".to_string()
            };
            self.meta_info = Some((p, found));
        }
        if self.show_info {
            let mut open = true;
            egui::Window::new("Image info").open(&mut open).resizable(false).show(ctx, |ui| {
                if let Some(d) = &self.doc {
                    egui::Grid::new("info").num_columns(2).striped(true).show(ui, |ui| {
                        let (w, h) = d.image.dimensions();
                        let mut row = |k: &str, v: String| {
                            ui.label(k);
                            ui.label(v);
                            ui.end_row();
                        };
                        row("File", d.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "(untitled)".into()));
                        row("Dimensions", format!("{w} × {h} ({:.1} MP)", (w as f64 * h as f64) / 1e6));
                        row("Format", if d.format.is_empty() { "-".into() } else { d.format.clone() });
                        if let Some(n) = d.source.as_ref().and_then(|s| s.frames.get()).map(|f| f.len()) {
                            row("Frames", n.to_string());
                        }
                        row("File size", human_size(d.file_size));
                        row("Memory", human_size(d.image.as_raw().len() as u64));
                        row("Alpha channel", if d.has_alpha { "yes" } else { "no" }.into());
                        if let Some((mp, found)) = &self.meta_info
                            && d.path.as_ref() == Some(mp)
                        {
                            row("Metadata", found.clone());
                        }
                        row("Decode time", format!("{:.1} ms", d.decode_ms));
                        row("Undo steps", d.undo.len().to_string());
                    });
                } else {
                    ui.label("No image");
                }
            });
            self.show_info = open;
        }

        if self.show_help {
            let mut open = true;
            egui::Window::new("Keyboard shortcuts").open(&mut open).resizable(false).show(ctx, |ui| {
                egui::Grid::new("help").num_columns(2).striped(true).show(ui, |ui| {
                    for (k, v) in HELP {
                        ui.strong(sc(k));
                        ui.label(*v);
                        ui.end_row();
                    }
                });
            });
            self.show_help = open;
        }

        if self.confirm_delete {
            let name = self.doc.as_ref().map(|d| d.name()).unwrap_or_default();
            let modal = egui::Modal::new(egui::Id::unique("confirm-delete")).show(ctx, |ui| {
                ui.heading("Move to trash?");
                ui.label(&name);
                ui.add_space(8.0);
                let mut answer = None;
                ui.horizontal(|ui| {
                    if ui.button("Move to trash").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(false);
                    }
                });
                answer
            });
            match modal.inner {
                Some(true) => {
                    self.confirm_delete = false;
                    self.delete_current();
                }
                Some(false) => self.confirm_delete = false,
                None if modal.should_close() => self.confirm_delete = false,
                None => {}
            }
        }

        if self.pending.is_some() {
            let name = self.doc.as_ref().map(|d| d.name()).unwrap_or_default();
            let modal = egui::Modal::new(egui::Id::unique("unsaved")).show(ctx, |ui| {
                ui.heading("Save changes?");
                ui.label(format!("\"{name}\" has unsaved edits."));
                ui.add_space(8.0);
                let mut answer = None;
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        answer = Some(0);
                    }
                    if ui.button("Don't save").clicked() {
                        answer = Some(1);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(2);
                    }
                });
                answer
            });
            let answer = modal.inner.or(modal.should_close().then_some(2));
            match answer {
                Some(0) => {
                    if self.save() {
                        let p = self.pending.take().unwrap();
                        self.run_pending(ctx, p);
                    }
                }
                Some(1) => {
                    let p = self.pending.take().unwrap();
                    self.run_pending(ctx, p);
                }
                Some(_) => self.pending = None,
                None => {}
            }
        }
    }

    fn batch_window(&mut self, ctx: &egui::Context) {
        if !self.batch.open {
            return;
        }
        let mut open = true;
        let b = &mut self.batch;
        let running = b.progress.as_ref().is_some_and(|p| !p.finished.load(Ordering::Relaxed));
        let mut start = false;
        egui::Window::new("Batch conversion").open(&mut open).default_width(480.0).show(ctx, |ui| {
            ui.add_enabled_ui(!running, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut b.mode, BatchMode::Convert, "Convert / resize");
                    ui.selectable_value(&mut b.mode, BatchMode::StripMetadata, "Remove metadata");
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.strong(format!("Input files: {}", b.inputs.len()));
                    if ui.button("Add files…").clicked()
                        && let Some(files) = rfd::FileDialog::new().add_filter("Images", loader::extensions()).pick_files() {
                            add_unique(&mut b.inputs, files);
                        }
                    if ui.button("Add folder…").clicked()
                        && let Some(dir) = rfd::FileDialog::new().pick_folder() {
                            let files = if b.recursive { list_images_recursive(&dir) } else { loader::list_images(&dir) };
                            add_unique(&mut b.inputs, files);
                        }
                    ui.checkbox(&mut b.recursive, "Include subfolders");
                    if ui.button("Clear").clicked() {
                        b.inputs.clear();
                    }
                });
                ui.weak("Tip: drop files onto the window to add them while this dialog is open.");
                egui::ScrollArea::vertical().max_height(120.0).auto_shrink([false, true]).show(ui, |ui| {
                    let mut remove = None;
                    for (i, p) in b.inputs.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.small_button("x").clicked() {
                                remove = Some(i);
                            }
                            ui.label(p.file_name().unwrap_or_default().to_string_lossy());
                        });
                    }
                    if let Some(i) = remove {
                        b.inputs.remove(i);
                    }
                });
                ui.separator();
                let strip = b.mode == BatchMode::StripMetadata;
                let in_place = strip && b.in_place;
                egui::Grid::new("batch").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    if strip {
                        ui.label("Write to");
                        ui.checkbox(&mut b.in_place, "Overwrite the original files");
                        ui.end_row();
                    }
                    ui.label("Output folder");
                    ui.add_enabled_ui(!in_place, |ui| {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut b.out_dir).desired_width(280.0));
                            if ui.button("…").clicked()
                                && let Some(d) = rfd::FileDialog::new().pick_folder() {
                                    b.out_dir = d.display().to_string();
                                }
                        });
                    });
                    ui.end_row();
                    if !in_place {
                        ui.label("File name");
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut b.pattern).desired_width(200.0));
                                egui::ComboBox::from_id_salt("presets").selected_text("Presets").width(90.0).show_ui(ui, |ui| {
                                    for p in NAME_PRESETS {
                                        if ui.selectable_label(b.pattern == *p, *p).clicked() {
                                            b.pattern = p.to_string();
                                        }
                                    }
                                });
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                for (tok, what) in NAME_TOKENS {
                                    if ui.small_button(*tok).on_hover_text(*what).clicked() {
                                        b.pattern.push_str(tok);
                                    }
                                }
                            });
                            if b.pattern.contains("{n}") {
                                ui.horizontal(|ui| {
                                    ui.label("Numbers start at");
                                    ui.add(egui::DragValue::new(&mut b.start).range(0..=1_000_000));
                                });
                            }
                            if let Some(first) = b.inputs.first() {
                                let ext = if strip {
                                    first.extension().unwrap_or_default().to_string_lossy().into_owned()
                                } else {
                                    b.format.ext().to_string()
                                };
                                let name = convert::output_name(&b.pattern, first, 0, b.inputs.len(), b.start, None);
                                ui.weak(format!("{} → {name}.{ext}", file_name(first)));
                            }
                            let key = (b.pattern.clone(), b.inputs.len(), b.start, b.mode);
                            if b.duplicates.as_ref().is_none_or(|d| d.0 != key) {
                                let total = b.inputs.len();
                                let mut seen = std::collections::HashSet::new();
                                let dups = b
                                    .inputs
                                    .iter()
                                    .enumerate()
                                    .filter(|(i, p)| {
                                        let ext = if strip { p.extension().unwrap_or_default().to_string_lossy().to_lowercase() } else { String::new() };
                                        let name = convert::output_name(&b.pattern, p, *i, total, b.start, None);
                                        !seen.insert((name.to_lowercase(), ext))
                                    })
                                    .count();
                                b.duplicates = Some((key, dups));
                            }
                            if let Some((_, d)) = b.duplicates.as_ref().filter(|d| d.1 > 0 && !convert::needs_dims(&b.pattern)) {
                                ui.colored_label(
                                    Color32::from_rgb(255, 190, 90),
                                    format!("{d} files would get a name that's already taken; add {{n}} to keep them apart."),
                                );
                            }
                        });
                        ui.end_row();
                    }
                    if strip {
                        return;
                    }
                    ui.label("Format");
                    egui::ComboBox::from_id_salt("fmt").selected_text(b.format.label()).show_ui(ui, |ui| {
                        for f in OutFormat::ALL {
                            ui.selectable_value(&mut b.format, f, f.label());
                        }
                    });
                    ui.end_row();
                    if b.format == OutFormat::Jpeg {
                        ui.label("JPEG quality");
                        ui.add(egui::Slider::new(&mut b.save.jpeg_quality, 1..=100));
                        ui.end_row();
                    }
                    if b.format == OutFormat::Png {
                        ui.label("PNG compression");
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut b.save.png_level, PngLevel::Fast, "Fast");
                            ui.selectable_value(&mut b.save.png_level, PngLevel::Default, "Default");
                            ui.selectable_value(&mut b.save.png_level, PngLevel::Best, "Best");
                        });
                        ui.end_row();
                    }
                    if b.format == OutFormat::Webp {
                        ui.label("WebP");
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut b.save.webp_lossless, "Lossless");
                            ui.add_enabled(!b.save.webp_lossless, egui::Slider::new(&mut b.save.webp_quality, 1..=100).text("quality"));
                        });
                        ui.end_row();
                    }
                    ui.label("Resize");
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut b.resize_mode, 0, "None");
                            ui.radio_value(&mut b.resize_mode, 1, "Percent");
                            ui.radio_value(&mut b.resize_mode, 2, "Fit within");
                            ui.radio_value(&mut b.resize_mode, 3, "Exact");
                        });
                        match b.resize_mode {
                            1 => {
                                ui.add(egui::Slider::new(&mut b.percent, 1.0..=400.0).suffix("%"));
                            }
                            2 | 3 => {
                                ui.horizontal(|ui| {
                                    ui.add(egui::DragValue::new(&mut b.w).range(1..=65535));
                                    ui.label("×");
                                    ui.add(egui::DragValue::new(&mut b.h).range(1..=65535));
                                    if b.resize_mode == 2 {
                                        ui.weak("(never enlarges)");
                                    }
                                });
                            }
                            _ => {}
                        }
                        if b.resize_mode != 0 {
                            egui::ComboBox::from_id_salt("bfilter").selected_text(b.filter.label()).show_ui(ui, |ui| {
                                for f in ResizeFilter::ALL {
                                    ui.selectable_value(&mut b.filter, f, f.label());
                                }
                            });
                        }
                    });
                    ui.end_row();
                    ui.label("Also");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut b.rotate_cw, "Rotate right");
                        ui.checkbox(&mut b.flip_h, "Flip horizontal");
                        ui.checkbox(&mut b.grayscale, "Grayscale");
                    });
                    ui.end_row();
                });
                if !in_place {
                    ui.checkbox(&mut b.overwrite, "Overwrite existing output files");
                }
                ui.weak(if strip {
                    "JPEG, PNG and WebP. Removes EXIF (including GPS), XMP, IPTC and comments without recompressing. \
                     ICC color profiles and the rotation tag are kept."
                } else {
                    "Converted files carry no metadata: EXIF, GPS and XMP are dropped."
                });
                ui.add_space(6.0);
                let ready = !b.inputs.is_empty() && (in_place || !b.out_dir.trim().is_empty());
                let label = if strip { "Remove metadata" } else { "Start conversion" };
                start = ui.add_enabled(ready, egui::Button::new(label)).clicked();
            });

            if let Some(p) = &b.progress {
                ui.separator();
                let done = p.done.load(Ordering::Relaxed);
                let frac = if p.total == 0 { 1.0 } else { done as f32 / p.total as f32 };
                ui.add(egui::ProgressBar::new(frac).text(format!("{done} / {}", p.total)));
                let failed = p.failed.load(Ordering::Relaxed);
                let skipped = p.skipped.load(Ordering::Relaxed);
                ui.horizontal(|ui| {
                    if p.finished.load(Ordering::Relaxed) {
                        ui.label(format!(
                            "Finished{}: {} done, {failed} failed, {skipped} skipped",
                            if p.cancel.load(Ordering::Relaxed) { " (cancelled)" } else { "" },
                            done.saturating_sub(failed + skipped)
                        ));
                        if ui.button("Open output folder").clicked() {
                            open_in_file_manager(Path::new(b.out_dir.trim()));
                        }
                    } else if ui.button("Cancel").clicked() {
                        p.cancel.store(true, Ordering::Relaxed);
                    }
                });
                let log = p.log.lock().unwrap();
                if !log.is_empty() {
                    egui::ScrollArea::vertical().id_salt("log").max_height(100.0).show(ui, |ui| {
                        for l in log.iter() {
                            ui.colored_label(Color32::from_rgb(255, 120, 100), l);
                        }
                    });
                }
            }
        });
        if start {
            let resize = match b.resize_mode {
                1 => BatchResize::Percent(b.percent),
                2 => BatchResize::Fit(b.w, b.h),
                3 => BatchResize::Exact(b.w, b.h),
                _ => BatchResize::None,
            };
            let settings = BatchSettings {
                mode: b.mode,
                in_place: b.mode == BatchMode::StripMetadata && b.in_place,
                inputs: b.inputs.clone(),
                out_dir: PathBuf::from(b.out_dir.trim()),
                format: b.format,
                save: b.save,
                resize,
                filter: b.filter,
                grayscale: b.grayscale,
                flip_h: b.flip_h,
                rotate_cw: b.rotate_cw,
                pattern: b.pattern.clone(),
                start: b.start,
                overwrite: b.overwrite,
            };
            b.progress = Some(convert::start_batch(settings, ctx.clone()));
        }
        if !open {
            if let Some(p) = &b.progress {
                p.cancel.store(true, Ordering::Relaxed);
            }
            b.open = false;
            b.progress = None;
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let files: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if files.is_empty() {
            return;
        }
        if self.batch.open && self.batch.progress.as_ref().is_none_or(|p| p.finished.load(Ordering::Relaxed)) {
            let mut all = Vec::new();
            for f in files {
                if f.is_dir() {
                    all.extend(list_images_recursive(&f));
                } else {
                    all.push(f);
                }
            }
            add_unique(&mut self.batch.inputs, all);
        } else if let Some(f) = files.into_iter().next() {
            if f.is_dir() {
                self.target = None;
                self.scan_folder(&f);
            } else {
                self.request_open(f);
            }
        }
    }
}

impl App {
    #[cfg(windows)]
    /// Before the GPU is ready: picks up finished decodes and returns the
    /// image to preview, tagged with a version that changes when it does.
    pub fn preview_image(&mut self) -> Option<(u64, Arc<RgbaImage>)> {
        let ctx = self.ctx.clone();
        self.poll_background(&ctx);
        self.doc.as_ref().map(|d| (d.version, d.image.clone()))
    }

    /// Handles files handed to us by the OS while running (macOS Finder).
    pub fn pump(&mut self) {
        #[cfg(target_os = "macos")]
        {
            let opened = crate::macos::take_opened();
            if self.batch.open && !opened.is_empty() {
                add_unique(&mut self.batch.inputs, opened);
            } else if let Some(p) = opened.into_iter().next() {
                self.request_open(p);
            }
        }
    }

    pub fn has_image(&self) -> bool {
        self.doc.is_some()
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();

        // Keyboard focus belongs to text fields only. A focused button would
        // turn arrows into focus navigation and Space/Enter into clicks,
        // fighting the viewer shortcuts.
        if !ctx.text_edit_focused() {
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
        }

        self.pump();
        self.thumbs.begin_frame();
        self.poll_background(&ctx);
        self.handle_drops(&ctx);
        self.shortcuts(&ctx);
        self.slideshow_tick(&ctx);
        // Switching away from the Text tool keeps what was typed.
        if self.tool != Tool::Text && self.text_draft.is_some() {
            self.commit_text();
        }

        self.animate(&ctx);

        // Keep GPU textures in sync with the document.
        match &self.doc {
            Some(doc) if doc.version != self.uploaded_version => {
                let t = Instant::now();
                let max_side = ctx.input(|i| i.max_texture_side);
                self.canvas.set_image(&ctx, &doc.image, max_side);
                self.uploaded_version = doc.version;
                if self.timing {
                    eprintln!("[timing] texture prep {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
                }
            }
            None if self.uploaded_version != 0 => {
                self.canvas.clear();
                self.uploaded_version = 0;
            }
            _ => {}
        }

        // Window title.
        let title = match &self.doc {
            Some(d) => format!("{}{} — {APP_NAME}", d.name(), if d.dirty { " *" } else { "" }),
            None => APP_NAME.to_string(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        let fullscreen = is_fullscreen(&ctx);
        if !fullscreen {
            egui::Panel::top("menu").show(ui, |ui| {
                self.menu_bar(ui);
                self.toolbar(ui);
            });
            egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
            if self.show_strip && !self.folder.is_empty() && !self.grid {
                egui::Panel::bottom("strip").resizable(false).exact_size(96.0).show(ui, |ui| self.strip_ui(ui));
            }
            if self.show_tools {
                egui::Panel::left("tools").resizable(false).exact_size(170.0).show(ui, |ui| self.tools_panel(ui));
            }
        }
        egui::CentralPanel::no_frame().show(ui, |ui| if self.grid { self.grid_ui(ui) } else { self.canvas_ui(ui) });

        self.dialogs(&ctx);

        // Confirm before closing with unsaved edits.
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close
            && self.doc.as_ref().is_some_and(|d| d.dirty) {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.pending = Some(Pending::Quit);
            }
    }
}

// ---------------------------------------------------------------------------
// helpers

const HELP: &[(&str, &str)] = &[
    ("→ / Space / PgDn", "Next image"),
    ("← / Backspace / PgUp", "Previous image"),
    ("Home / End", "First / last image"),
    ("Wheel, + / −", "Zoom"),
    ("0 or F / 1", "Fit / actual size"),
    ("Drag, middle-drag, Space+drag", "Pan"),
    ("Enter / F11 / double-click", "Full screen"),
    ("T", "Thumbnail strip"),
    ("G", "Grid view (arrows move, Enter opens, +/− size)"),
    ("F5", "Slideshow (Space pauses, Esc stops)"),
    ("R / L", "Rotate right / left"),
    ("H / V", "Flip horizontal / vertical"),
    ("Ctrl+Y", "Crop to selection"),
    ("Ctrl+R", "Resize"),
    ("Ctrl+Shift+R / Ctrl+Shift+L", "Lossless JPEG rotate right / left (on the file)"),
    ("P", "Pause / play animation"),
    ("Ctrl+Shift+I", "Invert colors"),
    ("Ctrl+G", "Grayscale"),
    ("Ctrl+Z / Ctrl+Shift+Z", "Undo / redo"),
    ("Ctrl+C / Ctrl+V", "Copy / paste image"),
    ("Ctrl+O / Ctrl+S / Ctrl+Shift+S", "Open / save / save as"),
    ("Ctrl+N", "New image"),
    ("E", "Paint tools, text and history"),
    ("Ctrl+E", "Remove metadata (EXIF/GPS), overwriting the file"),
    ("B", "Batch conversion"),
    ("I", "Image info"),
    ("Del", "Move file to trash"),
    ("Esc", "Close dialog / clear selection / leave full screen / quit"),
];

/// Shortcut label for this platform ("Ctrl" is "Cmd" on macOS).
fn sc(s: &str) -> String {
    if cfg!(target_os = "macos") { s.replace("Ctrl", "Cmd") } else { s.to_string() }
}

fn rgba(c: Color32) -> [u8; 4] {
    c.to_srgba_unmultiplied()
}

fn is_fullscreen(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().fullscreen.unwrap_or(false))
}

fn toggle_fullscreen(ctx: &egui::Context) {
    let fs = is_fullscreen(ctx);
    ctx.send_viewport_cmd(ViewportCommand::Fullscreen(!fs));
}

/// Shift-constrained end point: 45° lines, square boxes.
fn constrain(tool: Tool, start: Pos2, p: Pos2) -> Pos2 {
    let d = p - start;
    match tool {
        Tool::Line => {
            let angle = d.y.atan2(d.x);
            let step = std::f32::consts::FRAC_PI_4;
            let a = (angle / step).round() * step;
            let len = d.length();
            start + Vec2::new(a.cos(), a.sin()) * len
        }
        Tool::Rect | Tool::Ellipse | Tool::Select => {
            let m = d.x.abs().max(d.y.abs());
            start + Vec2::new(m * d.x.signum(), m * d.y.signum())
        }
        _ => p,
    }
}

fn dashed_rect(painter: &egui::Painter, r: Rect) {
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    painter.add(egui::Shape::line(pts.to_vec(), Stroke::new(1.0, Color32::BLACK)));
    painter.add(egui::Shape::dashed_line(&pts, Stroke::new(1.0, Color32::WHITE), 4.0, 4.0));
}

/// Shortens `text` with an ellipsis in the middle, keeping the file
/// extension, so it fits in `width` points.
fn elide(painter: &egui::Painter, text: &str, font: &egui::FontId, width: f32) -> String {
    let fits = |s: &str| painter.layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE).size().x <= width;
    if fits(text) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let tail = text.rfind('.').map_or(0, |i| text[i..].chars().count()).min(8);
    let suffix: String = chars[chars.len() - tail..].iter().collect();
    for n in (1..chars.len() - tail).rev() {
        let s: String = chars[..n].iter().collect::<String>() + "…" + &suffix;
        if fits(&s) {
            return s;
        }
    }
    "…".to_string()
}

fn file_name(p: &Path) -> String {
    p.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

/// A thumbnail fitted into `r`, with selection / hover highlight.
fn draw_thumb(ui: &Ui, thumbs: &mut Thumbs, path: &Path, r: Rect, selected: bool, hovered: bool) {
    let painter = ui.painter();
    if selected {
        painter.rect_filled(r.expand(4.0), 5.0, Color32::from_rgb(38, 79, 140));
    } else if hovered {
        painter.rect_filled(r.expand(4.0), 5.0, Color32::from_gray(52));
    }
    match thumbs.get(path).map(|t| (t.id(), t.size_vec2())) {
        Some((tex, size)) => {
            let k = (r.width() / size.x).min(r.height() / size.y);
            let dst = Rect::from_center_size(r.center(), size * k);
            painter.image(tex, dst, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        None if thumbs.failed(path) => {
            painter.text(r.center(), egui::Align2::CENTER_CENTER, "?", egui::FontId::proportional(20.0), Color32::from_gray(120));
        }
        None => {
            painter.rect_filled(r.shrink(r.width().min(r.height()) * 0.2), 3.0, Color32::from_gray(34));
        }
    }
}

fn human_size(b: u64) -> String {
    let b = b as f64;
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} GB", b / (1024.0 * 1024.0 * 1024.0))
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.2} MB", b / (1024.0 * 1024.0))
    } else if b >= 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else {
        format!("{b} B")
    }
}

fn add_unique(list: &mut Vec<PathBuf>, more: Vec<PathBuf>) {
    let mut seen: std::collections::HashSet<PathBuf> = list.iter().cloned().collect();
    for p in more {
        if loader::is_supported(&p) && seen.insert(p.clone()) {
            list.push(p);
        }
    }
}

fn list_images_recursive(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        out.extend(loader::list_images(&d));
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(e.path());
                }
            }
        }
    }
    out
}

fn open_in_file_manager(dir: &Path) {
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(dir).spawn();
}
