//! Lossless JPEG rotate/flip, like `jpegtran`: the DCT coefficients are
//! rearranged by libjpeg-turbo, so no generation loss happens. Works directly
//! on the file.
//!
//! The requested operation is relative to what the user *sees*, which already
//! includes the file's EXIF orientation. We compose the two into one transform
//! of the stored pixels and reset the EXIF orientation tag to 1 ("normal").

use std::path::Path;

use turbojpeg::{Transform, TransformOp};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    RotateCw,
    RotateCcw,
    Rotate180,
    FlipH,
    FlipV,
}

impl Op {
    pub const ALL: [Op; 5] = [Op::RotateCw, Op::RotateCcw, Op::Rotate180, Op::FlipH, Op::FlipV];

    pub fn label(self) -> &'static str {
        match self {
            Op::RotateCw => "Rotate right",
            Op::RotateCcw => "Rotate left",
            Op::Rotate180 => "Rotate 180°",
            Op::FlipH => "Flip horizontal",
            Op::FlipV => "Flip vertical",
        }
    }
}

/// Transforms of the square as 2×2 matrices on (x, y), y pointing down.
type M = [[i32; 2]; 2];
const I: M = [[1, 0], [0, 1]];
const FLIP_H: M = [[-1, 0], [0, 1]];
const FLIP_V: M = [[1, 0], [0, -1]];
const ROT90: M = [[0, -1], [1, 0]];
const ROT180: M = [[-1, 0], [0, -1]];
const ROT270: M = [[0, 1], [-1, 0]];
const TRANSPOSE: M = [[0, 1], [1, 0]];
const TRANSVERSE: M = [[0, -1], [-1, 0]];

fn mul(a: M, b: M) -> M {
    let mut r = [[0; 2]; 2];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = a[i][0] * b[0][j] + a[i][1] * b[1][j];
        }
    }
    r
}

/// The transform that turns stored pixels into the displayed image.
fn orientation_matrix(o: u16) -> M {
    match o {
        2 => FLIP_H,
        3 => ROT180,
        4 => FLIP_V,
        5 => TRANSPOSE,
        6 => ROT90,
        7 => TRANSVERSE,
        8 => ROT270,
        _ => I,
    }
}

fn op_matrix(op: Op) -> M {
    match op {
        Op::RotateCw => ROT90,
        Op::RotateCcw => ROT270,
        Op::Rotate180 => ROT180,
        Op::FlipH => FLIP_H,
        Op::FlipV => FLIP_V,
    }
}

fn tj_op(m: M) -> TransformOp {
    match m {
        FLIP_H => TransformOp::Hflip,
        FLIP_V => TransformOp::Vflip,
        ROT90 => TransformOp::Rot90,
        ROT180 => TransformOp::Rot180,
        ROT270 => TransformOp::Rot270,
        TRANSPOSE => TransformOp::Transpose,
        TRANSVERSE => TransformOp::Transverse,
        _ => TransformOp::None,
    }
}

pub fn is_jpeg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["jpg", "jpeg", "jpe", "jfif"].iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Losslessly transforms JPEG bytes. Returns the new bytes and whether a
/// partial edge block had to be trimmed (a few pixels at most) because the
/// image size isn't a multiple of the JPEG block size.
pub fn transform(jpeg: &[u8], op: Op) -> Result<(Vec<u8>, bool), String> {
    let orientation = find_orientation(jpeg);
    let m = mul(op_matrix(op), orientation_matrix(orientation.map_or(1, |o| o.1)));
    let tj = tj_op(m);

    let (mut out, trimmed) = if tj == TransformOp::None {
        (jpeg.to_vec(), false)
    } else {
        let mut t = Transform::op(tj);
        t.perfect = true;
        match turbojpeg::transform(&t, jpeg) {
            Ok(buf) => (buf.to_vec(), false),
            Err(_) => {
                t.perfect = false;
                t.trim = true;
                let buf = turbojpeg::transform(&t, jpeg).map_err(|e| format!("lossless transform: {e}"))?;
                (buf.to_vec(), true)
            }
        }
    };
    // Markers are copied as-is, so the old orientation tag came along: the
    // pixels are now stored upright.
    if let Some((offset, _, little_endian)) = find_orientation(&out) {
        let one = if little_endian { 1u16.to_le_bytes() } else { 1u16.to_be_bytes() };
        out[offset..offset + 2].copy_from_slice(&one);
    }
    Ok((out, trimmed))
}

/// Transforms the file in place (via a temporary file and rename).
pub fn transform_file(path: &Path, op: Op) -> Result<bool, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let (out, trimmed) = transform(&data, op)?;
    let tmp = path.with_extension("jpg.sfiv-tmp");
    std::fs::write(&tmp, &out).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(trimmed)
}

/// Finds the EXIF orientation entry in a JPEG: (byte offset of its value,
/// value, little-endian).
fn find_orientation(jpg: &[u8]) -> Option<(usize, u16, bool)> {
    if jpg.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    while i + 4 <= jpg.len() {
        if jpg[i] != 0xFF {
            return None;
        }
        let marker = jpg[i + 1];
        if marker == 0xFF {
            i += 1; // fill byte
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            return None; // image data starts; no EXIF after this
        }
        let len = u16::from_be_bytes([jpg[i + 2], jpg[i + 3]]) as usize;
        let seg = i + 4;
        let end = i + 2 + len;
        if end > jpg.len() || len < 2 {
            return None;
        }
        if marker == 0xE1 && jpg.get(seg..seg + 6)? == b"Exif\0\0" {
            let tiff = seg + 6;
            let le = match jpg.get(tiff..tiff + 2)? {
                b"II" => true,
                b"MM" => false,
                _ => return None,
            };
            let r16 = |o: usize| -> Option<u16> {
                let b: [u8; 2] = jpg.get(o..o + 2)?.try_into().ok()?;
                Some(if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
            };
            let r32 = |o: usize| -> Option<u32> {
                let b: [u8; 4] = jpg.get(o..o + 4)?.try_into().ok()?;
                Some(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
            };
            let ifd = tiff + r32(tiff + 4)? as usize;
            let count = r16(ifd)? as usize;
            for k in 0..count {
                let e = ifd + 2 + k * 12;
                if e + 12 > end {
                    return None;
                }
                if r16(e)? == 0x0112 {
                    return Some((e + 8, r16(e + 8)?, le));
                }
            }
            return None;
        }
        i = end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, RgbImage};

    fn jpeg(w: u32, h: u32) -> (RgbImage, Vec<u8>) {
        let img = RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, ((x / 8 + y / 8) % 2 * 200) as u8]));
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95).encode_image(&img).unwrap();
        (img, out)
    }

    /// Inserts an EXIF APP1 with the given orientation right after SOI.
    fn with_orientation(jpg: &[u8], o: u16) -> Vec<u8> {
        let mut tiff = b"MM\0\x2a\0\0\0\x08".to_vec();
        tiff.extend_from_slice(&1u16.to_be_bytes()); // one entry
        tiff.extend_from_slice(&0x0112u16.to_be_bytes());
        tiff.extend_from_slice(&3u16.to_be_bytes()); // SHORT
        tiff.extend_from_slice(&1u32.to_be_bytes());
        tiff.extend_from_slice(&o.to_be_bytes());
        tiff.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(tiff);
        let mut out = jpg[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        out.extend(app1);
        out.extend_from_slice(&jpg[2..]);
        out
    }

    fn decode(b: &[u8]) -> image::DynamicImage {
        let mut d = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(b)).unwrap();
        let o = image::ImageDecoder::orientation(&mut d).unwrap();
        let mut img = image::DynamicImage::from_decoder(d).unwrap();
        img.apply_orientation(o);
        img
    }

    fn mean_diff(a: &image::DynamicImage, b: &image::DynamicImage) -> f64 {
        assert_eq!(a.dimensions(), b.dimensions());
        let (a, b) = (a.to_rgb8(), b.to_rgb8());
        let sum: u64 = a.as_raw().iter().zip(b.as_raw()).map(|(x, y)| x.abs_diff(*y) as u64).sum();
        sum as f64 / a.as_raw().len() as f64
    }

    #[test]
    fn rotate_matches_pixel_rotation() {
        let (_, src) = jpeg(64, 48);
        let before = decode(&src);
        let (out, trimmed) = transform(&src, Op::RotateCw).unwrap();
        assert!(!trimmed);
        assert!(mean_diff(&decode(&out), &before.rotate90()) < 2.0);
        let (out, _) = transform(&src, Op::FlipH).unwrap();
        assert!(mean_diff(&decode(&out), &before.fliph()) < 2.0);
    }

    #[test]
    fn composes_with_exif_orientation() {
        let (_, src) = jpeg(64, 48);
        let tagged = with_orientation(&src, 6); // displayed rotated 90° CW
        assert_eq!(find_orientation(&tagged).map(|o| o.1), Some(6));
        let shown = decode(&tagged);
        assert_eq!(shown.dimensions(), (48, 64));
        let (out, _) = transform(&tagged, Op::RotateCw).unwrap();
        assert_eq!(find_orientation(&out).map(|o| o.1), Some(1));
        assert!(mean_diff(&decode(&out), &shown.rotate90()) < 2.0);
        // Undoing the tag's rotation needs no DCT work at all.
        let (out, _) = transform(&tagged, Op::RotateCcw).unwrap();
        assert!(mean_diff(&decode(&out), &shown.rotate270()) < 2.0);
    }

    #[test]
    fn trims_partial_blocks() {
        let (_, src) = jpeg(100, 75);
        let (out, trimmed) = transform(&src, Op::RotateCw).unwrap();
        assert!(trimmed);
        let d = decode(&out);
        assert!(d.width() <= 75 && d.width() >= 75 - 16 && d.height() <= 100 && d.height() >= 100 - 16);
    }
}
