//! OpenGL context creation.
//!
//! On Windows the expensive part of WGL setup (loading the GPU driver and
//! choosing a pixel format, often 200-400 ms) runs on a worker thread against
//! a hidden dummy window, while the main thread shows the real window and the
//! first image straight away. The finished context is then moved to the main
//! thread and bound to the real window.

use std::num::NonZeroU32;
use std::sync::Arc;

use glutin::config::{Config, ConfigTemplateBuilder, GlConfig};
use glutin::context::{
    ContextApi, ContextAttributesBuilder, GlProfile, NotCurrentContext, NotCurrentGlContext, PossiblyCurrentContext,
};
use glutin::display::{Display, DisplayApiPreference, GlDisplay};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

/// Display + config + context, created but not yet bound to a window.
pub struct GlParts {
    display: Display,
    config: Config,
    context: NotCurrentContext,
}

pub struct GlState {
    pub gl: Arc<glow::Context>,
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
}

/// Creates display, config and context. `window` is used for pixel format
/// selection where the platform needs one (WGL).
pub fn create(display_handle: RawDisplayHandle, window: Option<RawWindowHandle>) -> Result<GlParts, String> {
    #[cfg(windows)]
    let pref = DisplayApiPreference::Wgl(window);
    #[cfg(target_os = "macos")]
    let pref = DisplayApiPreference::Cgl;
    #[cfg(all(unix, not(target_os = "macos")))]
    let pref = DisplayApiPreference::EglThenGlx(Box::new(winit::platform::x11::register_xlib_error_hook));

    let display = unsafe { Display::new(display_handle, pref) }.map_err(|e| format!("GL display: {e}"))?;
    let mut template = ConfigTemplateBuilder::new()
        .prefer_hardware_accelerated(Some(true))
        .with_depth_size(0)
        .with_stencil_size(0)
        .with_transparency(false);
    if let Some(w) = window {
        template = template.compatible_with_native_window(w);
    }
    let config = unsafe { display.find_configs(template.build()) }
        .map_err(|e| format!("GL configs: {e}"))?
        .reduce(|best, c| {
            // Prefer the fewest multisamples; we don't need MSAA.
            if c.num_samples() < best.num_samples() { c } else { best }
        })
        .ok_or("no suitable GL config")?;

    // Same fallback chain as eframe: default, then GLES, then compatibility.
    let attempts = [
        ContextAttributesBuilder::new().build(window),
        ContextAttributesBuilder::new().with_context_api(ContextApi::Gles(None)).build(window),
        ContextAttributesBuilder::new().with_profile(GlProfile::Compatibility).build(window),
    ];
    let mut last_err = String::new();
    let mut context = None;
    for attrs in &attempts {
        match unsafe { display.create_context(&config, attrs) } {
            Ok(c) => {
                context = Some(c);
                break;
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    let context = context.ok_or_else(|| format!("GL context: {last_err}"))?;
    Ok(GlParts { display, config, context })
}

/// Binds the context to the real window and loads GL functions.
pub fn finish(parts: GlParts, window: RawWindowHandle, size: (u32, u32)) -> Result<GlState, String> {
    let (w, h) = (NonZeroU32::new(size.0.max(1)).unwrap(), NonZeroU32::new(size.1.max(1)).unwrap());
    let attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(window, w, h);
    let surface = unsafe { parts.display.create_window_surface(&parts.config, &attrs) }
        .map_err(|e| format!("GL surface: {e}"))?;
    let context = parts.context.make_current(&surface).map_err(|e| format!("make current: {e}"))?;
    let _ = surface.set_swap_interval(&context, SwapInterval::Wait(NonZeroU32::new(1).unwrap()));
    let display = parts.display;
    let gl = unsafe { glow::Context::from_loader_function_cstr(|s| display.get_proc_address(s)) };
    Ok(GlState { gl: Arc::new(gl), surface, context })
}

impl GlState {
    pub fn resize(&self, w: u32, h: u32) {
        if let (Some(w), Some(h)) = (NonZeroU32::new(w), NonZeroU32::new(h)) {
            self.surface.resize(&self.context, w, h);
        }
    }

    pub fn swap(&self) {
        let _ = self.surface.swap_buffers(&self.context);
    }
}

/// Windows: runs `create` on a worker thread against a hidden window.
#[cfg(windows)]
pub fn spawn_worker(on_done: impl FnOnce(Result<GlParts, String>) + Send + 'static) {
    use raw_window_handle::{Win32WindowHandle, WindowsDisplayHandle};
    use std::num::NonZeroIsize;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    std::thread::Builder::new()
        .name("gl-init".into())
        .spawn(move || unsafe {
            let hinstance = GetModuleHandleW(std::ptr::null());
            let class: Vec<u16> = "SFIV_GL_INIT\0".encode_utf16().collect();
            let wc = WNDCLASSW {
                style: CS_OWNDC,
                lpfnWndProc: Some(DefWindowProcW),
                hInstance: hinstance,
                lpszClassName: class.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                16,
                16,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinstance,
                std::ptr::null(),
            );
            let result = match NonZeroIsize::new(hwnd as isize) {
                Some(h) => {
                    let mut wh = Win32WindowHandle::new(h);
                    wh.hinstance = NonZeroIsize::new(hinstance as isize);
                    create(RawDisplayHandle::Windows(WindowsDisplayHandle::new()), Some(RawWindowHandle::Win32(wh)))
                }
                None => Err("could not create GL init window".into()),
            };
            on_done(result);
            // The context's config refers to this window's DC, so keep it alive
            // and keep pumping its messages so broadcasts never block on us.
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                DispatchMessageW(&msg);
            }
        })
        .expect("spawn gl-init thread");
}

// The parts are created on the worker thread and used once on the main thread;
// WGL objects are not tied to the creating thread when not current.
#[cfg(windows)]
unsafe impl Send for GlParts {}
