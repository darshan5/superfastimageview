//! macOS: receive files opened from Finder (double-click, "Open With", drops
//! on the Dock icon). AppKit delivers these as `application:openURLs:` on the
//! app delegate, which winit 0.30 doesn't implement, so we add that method to
//! winit's delegate class before the event loop starts running.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::sel;
use objc2_foundation::{NSArray, NSURL};

static OPENED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static WAKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Call after the winit event loop is built and before it runs.
pub fn install_open_handler(wake: impl Fn() + Send + Sync + 'static) {
    let _ = WAKE.set(Box::new(wake));
    let Some(cls) = AnyClass::get(c"WinitApplicationDelegate") else { return };
    type Handler = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut NSArray<NSURL>);
    unsafe {
        let imp: Imp = std::mem::transmute::<Handler, Imp>(open_urls);
        // v = void return, @ = self, : = _cmd, @ = application, @ = urls
        objc2::ffi::class_addMethod(
            cls as *const AnyClass as *mut AnyClass,
            sel!(application:openURLs:),
            imp,
            c"v@:@@".as_ptr(),
        );
    }
}

unsafe extern "C-unwind" fn open_urls(_this: *mut AnyObject, _cmd: Sel, _app: *mut AnyObject, urls: *mut NSArray<NSURL>) {
    let Some(urls) = (unsafe { urls.as_ref() }) else { return };
    if let Ok(mut q) = OPENED.lock() {
        for url in urls.iter() {
            if let Some(p) = url.path() {
                q.push(PathBuf::from(p.to_string()));
            }
        }
    }
    if let Some(wake) = WAKE.get() {
        wake();
    }
}

/// Files opened from Finder since the last call.
pub fn take_opened() -> Vec<PathBuf> {
    OPENED.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}
