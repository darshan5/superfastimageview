//! Formats decoded by the operating system's own codecs: HEIC/HEIF and AVIF.
//!
//! - Windows: WIC. HEIC needs Microsoft's "HEIF Image Extensions" and "HEVC
//!   Video Extensions" (Store), AVIF needs "AV1 Video Extension". Many PCs ship
//!   with them; we show a helpful error if they're missing.
//! - macOS: ImageIO, which supports HEIC (10.13+) and AVIF (13+) natively.

use std::path::Path;

use image::RgbaImage;

pub const EXTENSIONS: &[&str] = &["heic", "heif", "hif", "avif"];

pub fn is_platform_format(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Also detects HEIF/AVIF files by content (ISO-BMFF `ftyp` brand), so
/// misnamed files still open.
pub fn sniff(data: &[u8]) -> bool {
    if data.len() < 12 || &data[4..8] != b"ftyp" {
        return false;
    }
    matches!(&data[8..12], b"heic" | b"heix" | b"hevc" | b"heim" | b"heis" | b"mif1" | b"msf1" | b"avif" | b"avis")
}

#[cfg(windows)]
pub fn decode(path: &Path) -> Result<RgbaImage, String> {
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::*;
    use windows::core::HSTRING;

    const COMPONENT_NOT_FOUND: i32 = 0x8898_2F50_u32 as i32; // WINCODEC_ERR_COMPONENTNOTFOUND

    let wic = || -> windows::core::Result<RgbaImage> {
        unsafe {
            // Decode threads are short-lived; joining the MTA is cheap.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let decoder = factory.CreateDecoderFromFilename(
                &HSTRING::from(path.as_os_str()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )?;
            let frame = decoder.GetFrame(0)?;
            let conv = factory.CreateFormatConverter()?;
            conv.Initialize(&frame, &GUID_WICPixelFormat32bppRGBA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
            let (mut w, mut h) = (0u32, 0u32);
            conv.GetSize(&mut w, &mut h)?;
            let mut buf = vec![0u8; w as usize * h as usize * 4];
            conv.CopyPixels(std::ptr::null(), w * 4, &mut buf)?;
            Ok(RgbaImage::from_raw(w, h, buf).expect("buffer size matches"))
        }
    };
    wic().map_err(|e| {
        if e.code().0 == COMPONENT_NOT_FOUND {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if ext == "avif" {
                "AVIF needs the free \"AV1 Video Extension\" from the Microsoft Store.".to_string()
            } else {
                "HEIC needs \"HEIF Image Extensions\" and \"HEVC Video Extensions\" from the Microsoft Store.".to_string()
            }
        } else {
            format!("Windows codec: {e}")
        }
    })
}

#[cfg(target_os = "macos")]
pub fn decode(path: &Path) -> Result<RgbaImage, String> {
    use std::ffi::c_void;
    use std::os::unix::ffi::OsStrExt;

    type Ref = *const c_void;
    #[repr(C)]
    struct CGRect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFURLCreateFromFileSystemRepresentation(alloc: Ref, buf: *const u8, len: isize, is_dir: u8) -> Ref;
        fn CFRelease(r: Ref);
        fn CFDictionaryGetValue(dict: Ref, key: Ref) -> Ref;
        fn CFNumberGetValue(num: Ref, kind: isize, out: *mut c_void) -> u8;
    }
    #[link(name = "ImageIO", kind = "framework")]
    unsafe extern "C" {
        fn CGImageSourceCreateWithURL(url: Ref, options: Ref) -> Ref;
        fn CGImageSourceCreateImageAtIndex(src: Ref, index: usize, options: Ref) -> Ref;
        fn CGImageSourceCopyPropertiesAtIndex(src: Ref, index: usize, options: Ref) -> Ref;
        static kCGImagePropertyOrientation: Ref;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGImageGetWidth(img: Ref) -> usize;
        fn CGImageGetHeight(img: Ref) -> usize;
        fn CGColorSpaceCreateWithName(name: Ref) -> Ref;
        static kCGColorSpaceSRGB: Ref;
        fn CGBitmapContextCreate(data: *mut c_void, w: usize, h: usize, bpc: usize, bpr: usize, cs: Ref, info: u32) -> Ref;
        fn CGContextDrawImage(ctx: Ref, rect: CGRect, img: Ref);
    }
    const K_CF_NUMBER_SINT32: isize = 3;
    const K_CG_IMAGE_ALPHA_PREMULTIPLIED_LAST: u32 = 1;

    unsafe {
        let bytes = path.as_os_str().as_bytes();
        let url = CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize, 0);
        if url.is_null() {
            return Err("invalid path".into());
        }
        let src = CGImageSourceCreateWithURL(url, std::ptr::null());
        CFRelease(url);
        if src.is_null() {
            return Err("ImageIO cannot read this file".into());
        }
        let mut orientation = 1i32;
        let props = CGImageSourceCopyPropertiesAtIndex(src, 0, std::ptr::null());
        if !props.is_null() {
            let n = CFDictionaryGetValue(props, kCGImagePropertyOrientation);
            if !n.is_null() {
                CFNumberGetValue(n, K_CF_NUMBER_SINT32, (&mut orientation as *mut i32).cast());
            }
            CFRelease(props);
        }
        let img = CGImageSourceCreateImageAtIndex(src, 0, std::ptr::null());
        CFRelease(src);
        if img.is_null() {
            return Err("ImageIO could not decode this file".into());
        }
        let (w, h) = (CGImageGetWidth(img), CGImageGetHeight(img));
        let mut buf = vec![0u8; w * h * 4];
        let cs = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        let ctx = CGBitmapContextCreate(buf.as_mut_ptr().cast(), w, h, 8, w * 4, cs, K_CG_IMAGE_ALPHA_PREMULTIPLIED_LAST);
        if !ctx.is_null() {
            CGContextDrawImage(ctx, CGRect { x: 0.0, y: 0.0, w: w as f64, h: h as f64 }, img);
            CFRelease(ctx);
        }
        CFRelease(cs);
        CFRelease(img);
        if ctx.is_null() {
            return Err("could not create bitmap context".into());
        }
        // CoreGraphics gives premultiplied alpha; we store straight alpha.
        for p in buf.chunks_exact_mut(4) {
            let a = p[3] as u32;
            if a != 0 && a != 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        let mut out = image::DynamicImage::ImageRgba8(RgbaImage::from_raw(w as u32, h as u32, buf).unwrap());
        if let Some(o) = image::metadata::Orientation::from_exif(orientation as u8) {
            out.apply_orientation(o);
        }
        Ok(out.into_rgba8())
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn decode(_path: &Path) -> Result<RgbaImage, String> {
    Err("HEIC/AVIF are only supported on Windows and macOS".into())
}
