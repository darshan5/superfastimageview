//! Camera RAW (CR2, CR3, NEF, ARW, DNG, RAF, ORF, RW2, ...) via the pure-Rust
//! `rawler` crate.
//!
//! For speed we first show the camera's embedded JPEG preview: it's what the
//! camera itself rendered and decodes in milliseconds. If that preview is
//! much smaller than the sensor (some cameras embed only ~1.7 MP), the full
//! sensor data is developed afterwards in the background (~0.5 s for 24 MP).

use std::path::Path;
use std::sync::Arc;

use image::DynamicImage;
use image::metadata::Orientation;
use rawler::decoders::{Decoder, RawDecodeParams};
use rawler::imgop::develop::RawDevelop;
use rawler::rawsource::RawSource;

pub const EXTENSIONS: &[&str] = &[
    "3fr", "ari", "arw", "cr2", "cr3", "crw", "dcr", "dng", "erf", "fff", "iiq", "kdc", "mef", "mos", "mrw",
    "nef", "nrw", "orf", "ori", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf", "srw", "x3f",
];

pub fn is_raw(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

pub struct Preview {
    pub image: DynamicImage,
    /// "embedded preview" or "developed".
    pub what: &'static str,
    /// The preview is well below sensor resolution; call [`develop`] for full detail.
    pub needs_develop: bool,
}

fn open(data: &Arc<Vec<u8>>) -> Result<(RawSource, Box<dyn Decoder>), String> {
    let src = RawSource::new_from_shared_vec(data.clone());
    let decoder = rawler::get_decoder(&src).map_err(|e| format!("RAW: {e}"))?;
    Ok((src, decoder))
}

/// The fastest good-looking image in the file.
pub fn decode(data: &Arc<Vec<u8>>) -> Result<Preview, String> {
    let (src, decoder) = open(data)?;
    let params = RawDecodeParams::default();

    // Metadata-only decode ("dummy") is cheap: orientation and sensor size.
    let meta = decoder.raw_image(&src, &params, true).ok();
    let orientation = meta.as_ref().and_then(|r| Orientation::from_exif(r.orientation.to_u16() as u8));
    let sensor_long = meta.as_ref().map_or(0, |r| r.width.max(r.height) as u32);

    let preview = decoder
        .full_image(&src, &params)
        .ok()
        .flatten()
        .or_else(|| decoder.preview_image(&src, &params).ok().flatten())
        // Skip tiny thumbnails.
        .filter(|img| img.width().max(img.height()) >= 1024);

    match preview {
        Some(mut image) => {
            if let Some(o) = orientation {
                image.apply_orientation(o);
            }
            let needs_develop = image.width().max(image.height()) * 2 < sensor_long;
            Ok(Preview { image, what: "embedded preview", needs_develop })
        }
        None => Ok(Preview { image: develop_with(&src, decoder.as_ref(), orientation)?, what: "developed", needs_develop: false }),
    }
}

/// Develops the full sensor data (demosaic, white balance, color, gamma).
pub fn develop(data: &Arc<Vec<u8>>) -> Result<DynamicImage, String> {
    let (src, decoder) = open(data)?;
    let meta = decoder.raw_image(&src, &RawDecodeParams::default(), true).ok();
    let orientation = meta.and_then(|r| Orientation::from_exif(r.orientation.to_u16() as u8));
    develop_with(&src, decoder.as_ref(), orientation)
}

fn develop_with(src: &RawSource, decoder: &dyn Decoder, orientation: Option<Orientation>) -> Result<DynamicImage, String> {
    let raw = decoder.raw_image(src, &RawDecodeParams::default(), false).map_err(|e| format!("RAW: {e}"))?;
    let mut img = RawDevelop::default()
        .develop_intermediate(&raw)
        .map_err(|e| format!("RAW develop: {e}"))?
        .to_dynamic_image()
        .ok_or("RAW develop produced no image")?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    Ok(img)
}

/// A small embedded image for thumbnails: the smallest one that is at least
/// `min_side` pixels on its long side, developing only as a last resort.
pub fn thumbnail(data: &Arc<Vec<u8>>, min_side: u32) -> Result<DynamicImage, String> {
    let (src, decoder) = open(data)?;
    let params = RawDecodeParams::default();
    let meta = decoder.raw_image(&src, &params, true).ok();
    let orientation = meta.and_then(|r| Orientation::from_exif(r.orientation.to_u16() as u8));
    let big_enough = |img: &DynamicImage| img.width().max(img.height()) >= min_side;
    let found = decoder
        .thumbnail_image(&src, &params)
        .ok()
        .flatten()
        .filter(big_enough)
        .or_else(|| decoder.preview_image(&src, &params).ok().flatten().filter(big_enough))
        .or_else(|| decoder.full_image(&src, &params).ok().flatten());
    match found {
        Some(mut img) => {
            if let Some(o) = orientation {
                img.apply_orientation(o);
            }
            Ok(img)
        }
        None => develop_with(&src, decoder.as_ref(), orientation),
    }
}
