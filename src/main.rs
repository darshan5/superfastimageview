#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod canvas;
mod convert;
mod edit;
mod gl;
mod icon;
mod jpegtran;
mod loader;
mod metadata;
#[cfg(target_os = "macos")]
mod macos;
mod paint;
mod platform;
mod raw;
mod text;
mod thumbs;
#[cfg(windows)]
mod preview;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use egui::{ViewportCommand, ViewportId};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

#[cfg(windows)]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const APP_NAME: &str = "SuperFastImageView";

enum UserEvent {
    Repaint(Instant),
    #[cfg(windows)]
    GlReady,
}

struct Gpu {
    gl: gl::GlState,
    painter: egui_glow::Painter,
    egui_winit: egui_winit::State,
}

struct Runner {
    started: Instant,
    timing: bool,
    ctx: egui::Context,
    app: app::App,
    title: String,
    window: Option<Arc<Window>>,
    /// Taken when the GL worker is started (after the window exists: driver
    /// loading holds the DLL loader lock and would stall window creation).
    #[cfg_attr(not(windows), allow(dead_code))]
    gl_start: Option<(Sender<Result<gl::GlParts, String>>, EventLoopProxy<UserEvent>)>,
    gl_rx: Receiver<Result<gl::GlParts, String>>,
    gpu: Option<Gpu>,
    viewport_info: egui::ViewportInfo,
    close_pending: bool,
    next_repaint: Option<Instant>,
    first_image_shown: bool,
    /// Test hook: `SFIV_SHOT=a.png@800,b.png@2000` saves rendered frames.
    shots: Vec<(PathBuf, Instant)>,
    #[cfg(windows)]
    preview_version: u64,
}

fn parse_shots(started: Instant) -> Vec<(PathBuf, Instant)> {
    let Some(spec) = std::env::var_os("SFIV_SHOT") else { return Vec::new() };
    spec.to_string_lossy()
        .split(',')
        .filter_map(|s| {
            let (path, ms) = s.rsplit_once('@')?;
            Some((PathBuf::from(path), started + Duration::from_millis(ms.trim().parse().ok()?)))
        })
        .collect()
}

fn main() {
    let started = Instant::now();
    let initial = std::env::args_os().nth(1).map(PathBuf::from);

    // Start decoding before anything else so it overlaps window and GL setup.
    let mut loader = loader::Loader::new();
    if let Some(p) = initial.as_ref().filter(|p| p.is_file()) {
        loader.request(p);
    }

    let event_loop = EventLoop::<UserEvent>::with_user_event().build().expect("event loop");
    if std::env::var_os("SFIV_TIMING").is_some() {
        eprintln!("[timing] event loop {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
    }
    let proxy = event_loop.create_proxy();

    let (gl_tx, gl_rx) = channel();
    #[cfg(target_os = "macos")]
    {
        let proxy = proxy.clone();
        macos::install_open_handler(move || {
            let _ = proxy.send_event(UserEvent::Repaint(Instant::now()));
        });
    }

    let ctx = egui::Context::default();
    ctx.set_embed_viewports(true);
    let repaint_proxy = proxy.clone();
    ctx.set_request_repaint_callback(move |info| {
        let _ = repaint_proxy.send_event(UserEvent::Repaint(Instant::now() + info.delay));
    });
    let wake_proxy = proxy.clone();
    loader.set_waker(Arc::new(move || {
        let _ = wake_proxy.send_event(UserEvent::Repaint(Instant::now()));
    }));
    let title = match initial.as_ref().filter(|p| p.is_file()).and_then(|p| p.file_name()) {
        Some(name) => format!("{} — {APP_NAME}", name.to_string_lossy()),
        None => APP_NAME.to_string(),
    };
    let app = app::App::new(&ctx, loader, initial);

    let mut runner = Runner {
        started,
        timing: std::env::var_os("SFIV_TIMING").is_some(),
        ctx,
        app,
        title,
        window: None,
        gl_start: Some((gl_tx, proxy)),
        gl_rx,
        gpu: None,
        viewport_info: egui::ViewportInfo::default(),
        close_pending: false,
        next_repaint: None,
        first_image_shown: false,
        shots: parse_shots(started),
        #[cfg(windows)]
        preview_version: 0,
    };
    if let Some(t) = runner.shots.iter().map(|s| s.1).min() {
        runner.next_repaint = Some(t);
    }
    event_loop.run_app(&mut runner).expect("run app");
}

impl Runner {
    fn ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }

    fn create_window(&mut self, el: &ActiveEventLoop) {
        let size = winit::dpi::LogicalSize::new(1280.0, 860.0);
        let mut attrs = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(size)
            .with_min_inner_size(winit::dpi::LogicalSize::new(480.0, 320.0))
            .with_theme(Some(winit::window::Theme::Dark))
            .with_window_icon(winit::window::Icon::from_rgba(icon::icon_rgba(64), 64, 64).ok());
        if let Some(m) = el.primary_monitor() {
            let s = m.scale_factor();
            let ms = m.size();
            let w = (size.width * s) as i32;
            let h = (size.height * s) as i32;
            let x = m.position().x + (ms.width as i32 - w).max(0) / 2;
            let y = m.position().y + (ms.height as i32 - h).max(0) / 2;
            attrs = attrs.with_position(winit::dpi::PhysicalPosition::new(x, y));
        }
        let window = Arc::new(el.create_window(attrs).expect("create window"));
        if self.timing {
            eprintln!("[timing] window created {:.1} ms", self.ms());
        }
        self.window = Some(window);
        self.paint_preview(true);
    }

    fn raw_window(&self) -> RawWindowHandle {
        self.window.as_ref().unwrap().window_handle().expect("window handle").as_raw()
    }

    /// Windows: draws the current image with GDI until the GPU is ready.
    #[cfg(windows)]
    fn paint_preview(&mut self, force: bool) {
        if self.gpu.is_some() {
            return;
        }
        let Some(window) = &self.window else { return };
        let img = self.app.preview_image();
        let version = img.as_ref().map_or(0, |(v, _)| *v);
        if !force && version == self.preview_version {
            return;
        }
        self.preview_version = version;
        if let RawWindowHandle::Win32(h) = self.raw_window() {
            preview::paint(h.hwnd.get() as _, img.as_ref().map(|(_, i)| &**i), window.scale_factor() as f32);
            if img.is_some() && !self.first_image_shown {
                self.first_image_shown = true;
                if self.timing {
                    eprintln!("[timing] first image on screen (GDI preview) {:.1} ms", self.ms());
                }
            }
        }
    }

    #[cfg(not(windows))]
    fn paint_preview(&mut self, _force: bool) {}

    fn init_gpu(&mut self, parts: Result<gl::GlParts, String>) {
        let window = self.window.clone().unwrap();
        let size = window.inner_size();
        let state = parts.and_then(|p| gl::finish(p, self.raw_window(), (size.width, size.height)));
        let state = match state {
            Ok(s) => s,
            Err(e) => {
                rfd::MessageDialog::new()
                    .set_title(APP_NAME)
                    .set_description(format!("Could not initialise OpenGL:\n{e}"))
                    .show();
                std::process::exit(1);
            }
        };
        let painter = egui_glow::Painter::new(state.gl.clone(), "", None, true).expect("egui_glow painter");
        let egui_winit = egui_winit::State::new(
            self.ctx.clone(),
            ViewportId::ROOT,
            &*window,
            Some(window.scale_factor() as f32),
            Some(winit::window::Theme::Dark),
            Some(painter.max_texture_side()),
        );
        if self.timing {
            eprintln!("[timing] GPU ready {:.1} ms", self.ms());
        }
        egui_winit::update_viewport_info(&mut self.viewport_info, &self.ctx, &window, true);
        self.gpu = Some(Gpu { gl: state, painter, egui_winit });
        window.request_redraw();
    }

    fn redraw(&mut self, el: &ActiveEventLoop) {
        let (Some(window), Some(gpu)) = (self.window.clone(), self.gpu.as_mut()) else {
            self.paint_preview(true);
            return;
        };
        egui_winit::update_viewport_info(&mut self.viewport_info, &self.ctx, &window, false);
        self.viewport_info.events.clear();
        let closing = self.close_pending;
        if closing {
            self.viewport_info.events.push(egui::ViewportEvent::Close);
        }
        let mut input = gpu.egui_winit.take_egui_input(&window);
        input.viewports.insert(ViewportId::ROOT, self.viewport_info.clone());

        let app = &mut self.app;
        let mut out = self.ctx.run_ui(input, |ui| app.ui(ui));
        gpu.egui_winit.handle_platform_output(&window, std::mem::take(&mut out.platform_output));

        let prims = self.ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        let size = window.inner_size();
        let px = [size.width, size.height];
        gpu.painter.clear(px, [0.094, 0.094, 0.094, 1.0]);
        gpu.painter.paint_and_update_textures(px, out.pixels_per_point, &prims, &mut out.textures_delta);
        let now = Instant::now();
        if let Some(i) = self.shots.iter().position(|s| s.1 <= now) {
            let (path, _) = self.shots.remove(i);
            save_frame(&gpu.gl.gl, px, &path);
        }
        if let Some(t) = self.shots.iter().map(|s| s.1).min() {
            self.next_repaint = Some(self.next_repaint.map_or(t, |n| n.min(t)));
        }
        window.pre_present_notify();
        gpu.gl.swap();

        if !self.first_image_shown && self.app.has_image() {
            self.first_image_shown = true;
            if self.timing {
                eprintln!("[timing] first image on screen (GPU) {:.1} ms", self.ms());
            }
        }

        let Some(vo) = out.viewport_output.remove(&ViewportId::ROOT) else { return };
        let mut cancel = false;
        let mut close = false;
        let commands: Vec<ViewportCommand> = vo
            .commands
            .into_iter()
            .filter(|c| match c {
                ViewportCommand::CancelClose => {
                    cancel = true;
                    false
                }
                ViewportCommand::Close => {
                    close = true;
                    false
                }
                _ => true,
            })
            .collect();
        let mut actions = Vec::new();
        egui_winit::process_viewport_commands(&self.ctx, &mut self.viewport_info, commands, &window, &mut actions);

        if closing {
            if cancel {
                self.close_pending = false;
            } else {
                self.exit(el);
                return;
            }
        }
        if close {
            self.close_pending = true;
            window.request_redraw();
        }
        if vo.repaint_delay.is_zero() {
            window.request_redraw();
        } else if vo.repaint_delay < Duration::from_secs(3600) {
            self.schedule(Instant::now() + vo.repaint_delay);
        }
    }

    fn schedule(&mut self, when: Instant) {
        self.next_repaint = Some(self.next_repaint.map_or(when, |t| t.min(when)));
    }

    fn exit(&mut self, el: &ActiveEventLoop) {
        if let Some(gpu) = &mut self.gpu {
            gpu.painter.destroy();
        }
        el.exit();
    }
}

/// Reads back the frame being presented and writes it as a PNG.
fn save_frame(gl: &glow::Context, [w, h]: [u32; 2], path: &std::path::Path) {
    use glow::HasContext;
    let mut buf = vec![0u8; (w * h * 4) as usize];
    unsafe {
        gl.read_pixels(0, 0, w as i32, h as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut buf)));
    }
    if let Some(mut img) = image::RgbaImage::from_raw(w, h, buf) {
        image::imageops::flip_vertical_in_place(&mut img);
        for p in img.pixels_mut() {
            p.0[3] = 255;
        }
        let _ = img.save(path);
    }
}

impl ApplicationHandler<UserEvent> for Runner {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // Files opened from Finder arrive before this; start decoding them now.
        self.app.pump();
        self.create_window(el);
        #[cfg(windows)]
        if let Some((tx, proxy)) = self.gl_start.take() {
            gl::spawn_worker(move |r| {
                let _ = tx.send(r);
                let _ = proxy.send_event(UserEvent::GlReady);
            });
        }
        #[cfg(not(windows))]
        {
            use raw_window_handle::HasDisplayHandle;
            let display = el.display_handle().expect("display handle").as_raw();
            let parts = gl::create(display, Some(self.raw_window()));
            self.init_gpu(parts);
        }
        // The worker may already be done.
        if let Ok(parts) = self.gl_rx.try_recv() {
            self.init_gpu(parts);
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, event: UserEvent) {
        match event {
            #[cfg(windows)]
            UserEvent::GlReady => {
                if self.gpu.is_none() && self.window.is_some()
                    && let Ok(parts) = self.gl_rx.try_recv() {
                        self.init_gpu(parts);
                    }
            }
            UserEvent::Repaint(when) => {
                if self.gpu.is_none() {
                    self.paint_preview(false);
                } else if when <= Instant::now() {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                } else {
                    self.schedule(when);
                }
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else { return };
        if let Some(gpu) = &mut self.gpu {
            let resp = gpu.egui_winit.on_window_event(&window, &event);
            if resp.repaint {
                window.request_redraw();
            }
        }
        match event {
            WindowEvent::CloseRequested => {
                if self.gpu.is_none() {
                    el.exit();
                } else {
                    self.close_pending = true;
                    window.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &self.gpu {
                    gpu.gl.resize(size.width, size.height);
                    window.request_redraw();
                } else {
                    self.paint_preview(true);
                }
            }
            WindowEvent::RedrawRequested => self.redraw(el),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        match self.next_repaint {
            Some(t) if t <= Instant::now() => {
                self.next_repaint = None;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
                el.set_control_flow(ControlFlow::Wait);
            }
            Some(t) => el.set_control_flow(ControlFlow::WaitUntil(t)),
            None => el.set_control_flow(ControlFlow::Wait),
        }
    }
}
