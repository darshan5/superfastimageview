//! Lossless metadata removal (EXIF, XMP, IPTC, comments, text chunks) for
//! JPEG, PNG and WebP. The compressed image data is copied unchanged, and the
//! ICC color profile is kept so colors don't shift.
//!
//! If the EXIF data rotates the image, a minimal EXIF block holding only the
//! orientation tag is written back, so the photo stays upright without any
//! pixel being touched.

use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Jpeg,
    Png,
    Webp,
}

fn kind(data: &[u8]) -> Option<Kind> {
    if data.starts_with(&[0xFF, 0xD8]) {
        Some(Kind::Jpeg)
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Kind::Png)
    } else if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some(Kind::Webp)
    } else {
        None
    }
}

pub fn is_supported(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["jpg", "jpeg", "jpe", "jfif", "png", "webp"].iter().any(|x| x.eq_ignore_ascii_case(e))
    })
}

/// The kinds of metadata present, for display ("EXIF", "XMP", ...).
pub fn scan(data: &[u8]) -> Vec<&'static str> {
    let mut found = Vec::new();
    let mut add = |s: &'static str| {
        if !found.contains(&s) {
            found.push(s);
        }
    };
    match kind(data) {
        Some(Kind::Jpeg) => {
            for seg in jpeg_header_segments(data) {
                if let Some(s) = jpeg_segment_name(seg.marker, seg.body) {
                    add(s);
                }
            }
        }
        Some(Kind::Png) => {
            for c in png_chunks(data) {
                match c.ty {
                    b"eXIf" => add(exif_label(c.body)),
                    b"tEXt" | b"zTXt" | b"iTXt" => add("Text"),
                    b"tIME" => add("Time"),
                    b"iCCP" => add("ICC profile"),
                    _ => {}
                }
            }
        }
        Some(Kind::Webp) => {
            for c in webp_chunks(data) {
                match c.ty {
                    b"EXIF" => add(exif_label(c.body)),
                    b"XMP " => add("XMP"),
                    b"ICCP" => add("ICC profile"),
                    _ => {}
                }
            }
        }
        None => {}
    }
    found
}

/// The result of [`strip`].
pub struct Stripped {
    pub data: Vec<u8>,
    /// Bytes saved.
    pub removed: usize,
    /// The orientation tag was kept (in an otherwise empty EXIF block).
    pub kept_orientation: bool,
}

/// Removes metadata. Returns `Ok(None)` if there was nothing to remove.
pub fn strip(data: &[u8]) -> Result<Option<Stripped>, String> {
    let k = kind(data).ok_or("metadata removal supports JPEG, PNG and WebP")?;
    let orientation = match k {
        Kind::Jpeg => crate::jpegtran::orientation(data),
        Kind::Png | Kind::Webp => exif_orientation(k, data).unwrap_or(1),
    };
    let keep = (2..=8).contains(&orientation).then(|| minimal_exif(orientation));
    let keep = keep.as_deref();
    let out = match k {
        Kind::Jpeg => strip_jpeg(data, keep)?,
        Kind::Png => strip_png(data, keep),
        Kind::Webp => strip_webp(data, keep)?,
    };
    if out == data {
        return Ok(None);
    }
    Ok(Some(Stripped { removed: data.len().saturating_sub(out.len()), data: out, kept_orientation: keep.is_some() }))
}

/// Strips the file in place (temp file + rename). `Ok(None)`: nothing to remove.
pub fn strip_file(src: &Path, dst: &Path) -> Result<Option<Stripped>, String> {
    let data = std::fs::read(src).map_err(|e| e.to_string())?;
    let Some(s) = strip(&data)? else {
        if src != dst {
            std::fs::copy(src, dst).map_err(|e| e.to_string())?;
        }
        return Ok(None);
    };
    let tmp = dst.with_extension("sfiv-tmp");
    std::fs::write(&tmp, &s.data).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dst).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(Some(s))
}

// ---------------------------------------------------------------------------
// JPEG

struct Segment<'a> {
    marker: u8,
    body: &'a [u8],
}

/// Marker segments before the first scan.
fn jpeg_header_segments(d: &[u8]) -> Vec<Segment<'_>> {
    let mut out = Vec::new();
    let mut i = 2;
    while i + 4 <= d.len() && d[i] == 0xFF {
        let marker = d[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        let len = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
        let end = i + 2 + len;
        if len < 2 || end > d.len() {
            break;
        }
        out.push(Segment { marker, body: &d[i + 4..end] });
        i = end;
    }
    out
}

/// What a segment holds if it's removable metadata.
fn jpeg_segment_name(marker: u8, body: &[u8]) -> Option<&'static str> {
    match marker {
        0xE1 if body.starts_with(b"Exif\0") => Some(exif_label(body)),
        0xE1 if body.starts_with(b"http://ns.adobe.com/xap/") || body.starts_with(b"http://ns.adobe.com/xmp/") => Some("XMP"),
        0xE1 => Some("APP1 data"),
        0xE2 if body.starts_with(b"ICC_PROFILE\0") => None, // kept
        0xE2 if body.starts_with(b"MPF\0") => Some("MPF"),
        0xE2 => Some("APP2 data"),
        0xED => Some("IPTC"),
        0xFE => Some("Comment"),
        // APP0 (JFIF) and APP14 (Adobe color transform) affect decoding.
        0xE3..=0xEC | 0xEF => Some("Maker data"),
        _ => None,
    }
}

/// `keep`: an EXIF (TIFF) block to write in place of the old ones.
fn strip_jpeg(d: &[u8], keep: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(d.len());
    out.extend_from_slice(&d[..2]);
    // EXIF goes right after SOI, or after a JFIF APP0.
    let mut pending = keep.map(|tiff| {
        let mut seg = vec![0xFF, 0xE1];
        seg.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
        seg.extend_from_slice(b"Exif\0\0");
        seg.extend_from_slice(tiff);
        seg
    });
    let mut i = 2;
    // Header segments, dropping metadata.
    loop {
        if i + 4 > d.len() || d[i] != 0xFF {
            return Err("malformed JPEG".into());
        }
        let marker = d[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if marker != 0xE0
            && let Some(seg) = pending.take()
        {
            out.extend_from_slice(&seg);
        }
        if marker == 0xDA {
            break;
        }
        let len = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
        let end = i + 2 + len;
        if len < 2 || end > d.len() {
            return Err("malformed JPEG".into());
        }
        if jpeg_segment_name(marker, &d[i + 4..end]).is_none() {
            out.extend_from_slice(&d[i..end]);
        }
        i = end;
    }
    // Scans and the tables between them (progressive JPEGs), up to EOI.
    // Whatever follows EOI (e.g. MPF secondary images with their own EXIF)
    // is dropped.
    while i < d.len() {
        if d[i] == 0xFF && i + 1 < d.len() {
            let m = d[i + 1];
            match m {
                0x00 | 0xD0..=0xD7 | 0xFF => {
                    out.push(0xFF);
                    i += 1;
                }
                0xD9 => {
                    out.extend_from_slice(&[0xFF, 0xD9]);
                    return Ok(out);
                }
                _ => {
                    if i + 4 > d.len() {
                        return Err("malformed JPEG".into());
                    }
                    let len = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
                    let end = (i + 2 + len).min(d.len());
                    if jpeg_segment_name(m, &d[i + 4..end]).is_none() {
                        out.extend_from_slice(&d[i..end]);
                    }
                    i = end;
                }
            }
        } else {
            out.push(d[i]);
            i += 1;
        }
    }
    // Truncated file: keep what there was.
    Ok(out)
}

// ---------------------------------------------------------------------------
// PNG

struct Chunk<'a> {
    ty: &'a [u8; 4],
    raw: &'a [u8],
    body: &'a [u8],
}

fn png_chunks(d: &[u8]) -> Vec<Chunk<'_>> {
    let mut out = Vec::new();
    let mut i = 8;
    while i + 12 <= d.len() {
        let len = u32::from_be_bytes(d[i..i + 4].try_into().unwrap()) as usize;
        let end = i + 12 + len;
        if end > d.len() {
            break;
        }
        out.push(Chunk { ty: d[i + 4..i + 8].try_into().unwrap(), raw: &d[i..end], body: &d[i + 8..i + 8 + len] });
        i = end;
    }
    out
}

fn strip_png(d: &[u8], keep: Option<&[u8]>) -> Vec<u8> {
    let mut out = d[..8].to_vec();
    for c in png_chunks(d) {
        if !matches!(c.ty, b"eXIf" | b"tEXt" | b"zTXt" | b"iTXt" | b"tIME") {
            out.extend_from_slice(c.raw);
        }
        // eXIf must come before the image data; right after IHDR is fine.
        if c.ty == b"IHDR"
            && let Some(tiff) = keep
        {
            out.extend_from_slice(&(tiff.len() as u32).to_be_bytes());
            let start = out.len();
            out.extend_from_slice(b"eXIf");
            out.extend_from_slice(tiff);
            let crc = crc32(&out[start..]);
            out.extend_from_slice(&crc.to_be_bytes());
        }
    }
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

// ---------------------------------------------------------------------------
// WebP

fn webp_chunks(d: &[u8]) -> Vec<Chunk<'_>> {
    let mut out = Vec::new();
    let mut i = 12;
    while i + 8 <= d.len() {
        let len = u32::from_le_bytes(d[i + 4..i + 8].try_into().unwrap()) as usize;
        let end = (i + 8 + len + (len & 1)).min(d.len());
        if i + 8 + len > d.len() {
            break;
        }
        out.push(Chunk { ty: d[i..i + 4].try_into().unwrap(), raw: &d[i..end], body: &d[i + 8..i + 8 + len] });
        i = end;
    }
    out
}

fn strip_webp(d: &[u8], keep: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let chunks = webp_chunks(d);
    // Only the extended format (VP8X) may carry EXIF.
    let keep = keep.filter(|_| chunks.iter().any(|c| c.ty == b"VP8X"));
    let mut out = d[..12].to_vec();
    for c in &chunks {
        match c.ty {
            b"EXIF" | b"XMP " => {}
            b"VP8X" => {
                let start = out.len();
                out.extend_from_slice(c.raw);
                // Clear the XMP (0x04) and, unless kept, EXIF (0x08) flags.
                if let Some(flags) = out.get_mut(start + 8) {
                    *flags &= if keep.is_some() { !0x04 } else { !0x0C };
                }
            }
            _ => out.extend_from_slice(c.raw),
        }
    }
    if let Some(tiff) = keep {
        out.extend_from_slice(b"EXIF");
        out.extend_from_slice(&(tiff.len() as u32).to_le_bytes());
        out.extend_from_slice(tiff);
        if tiff.len() % 2 == 1 {
            out.push(0);
        }
    }
    let size = u32::try_from(out.len() - 8).map_err(|_| "file too large")?;
    out[4..8].copy_from_slice(&size.to_le_bytes());
    Ok(out)
}

// ---------------------------------------------------------------------------
// EXIF orientation in PNG / WebP

fn exif_orientation(k: Kind, d: &[u8]) -> Option<u16> {
    let exif = match k {
        Kind::Png => png_chunks(d).into_iter().find(|c| c.ty == b"eXIf")?.body,
        Kind::Webp => webp_chunks(d).into_iter().find(|c| c.ty == b"EXIF")?.body,
        Kind::Jpeg => return None,
    };
    let tiff = exif.strip_prefix(b"Exif\0\0").unwrap_or(exif);
    tiff_orientation(tiff)
}

/// A little-endian TIFF block whose only tag is the orientation.
fn minimal_exif(orientation: u16) -> Vec<u8> {
    let mut t = b"II*\0".to_vec();
    t.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset
    t.extend_from_slice(&1u16.to_le_bytes()); // one entry
    t.extend_from_slice(&0x0112u16.to_le_bytes());
    t.extend_from_slice(&3u16.to_le_bytes()); // SHORT
    t.extend_from_slice(&1u32.to_le_bytes());
    t.extend_from_slice(&orientation.to_le_bytes());
    t.extend_from_slice(&[0, 0]);
    t.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
    t
}

/// "Orientation" for a block holding nothing but that tag, else "EXIF".
fn exif_label(exif: &[u8]) -> &'static str {
    let t = exif.strip_prefix(b"Exif\0\0").unwrap_or(exif);
    let only_orientation = (|| {
        let le = t.get(0..2)? == b"II";
        let r16 = |o: usize| -> Option<u16> {
            let b: [u8; 2] = t.get(o..o + 2)?.try_into().ok()?;
            Some(if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
        };
        let r32 = |o: usize| -> Option<u32> {
            let b: [u8; 4] = t.get(o..o + 4)?.try_into().ok()?;
            Some(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
        };
        let ifd = r32(4)? as usize;
        Some(r16(ifd)? == 1 && r16(ifd + 2)? == 0x0112 && r32(ifd + 14)? == 0)
    })();
    if only_orientation == Some(true) { "Orientation" } else { "EXIF" }
}

/// Reads the orientation tag from IFD0 of a TIFF/EXIF block.
pub fn tiff_orientation(t: &[u8]) -> Option<u16> {
    let le = match t.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let r16 = |o: usize| -> Option<u16> {
        let b: [u8; 2] = t.get(o..o + 2)?.try_into().ok()?;
        Some(if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    };
    let r32 = |o: usize| -> Option<u32> {
        let b: [u8; 4] = t.get(o..o + 4)?.try_into().ok()?;
        Some(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    };
    let ifd = r32(4)? as usize;
    for k in 0..r16(ifd)? as usize {
        let e = ifd + 2 + k * 12;
        if r16(e)? == 0x0112 {
            return r16(e + 8);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal EXIF block with an orientation tag.
    fn exif(orientation: u16) -> Vec<u8> {
        let mut t = b"Exif\0\0II*\0\x08\0\0\0\x01\0".to_vec();
        t.extend_from_slice(&0x0112u16.to_le_bytes());
        t.extend_from_slice(&3u16.to_le_bytes()); // SHORT
        t.extend_from_slice(&1u32.to_le_bytes());
        t.extend_from_slice(&orientation.to_le_bytes());
        t.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        t
    }

    fn with_segment(jpeg: &[u8], marker: u8, body: &[u8]) -> Vec<u8> {
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xFF, marker]);
        out.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    fn sample_jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 7) as u8, (y * 5) as u8, 90]));
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
            .encode_image(&img)
            .unwrap();
        out
    }

    #[test]
    fn strips_jpeg_and_keeps_pixels() {
        let base = sample_jpeg(64, 48);
        let mut j = with_segment(&base, 0xE1, &exif(1));
        j = with_segment(&j, 0xFE, b"hello");
        j = with_segment(&j, 0xE2, b"ICC_PROFILE\0\x01\x01fake");
        j.extend_from_slice(b"trailing junk");
        assert_eq!(scan(&j), vec!["Comment", "Orientation"]);
        let s = strip(&j).unwrap().unwrap();
        assert!(!s.kept_orientation);
        assert!(scan(&s.data).is_empty());
        assert!(s.data.ends_with(&[0xFF, 0xD9]));
        // ICC kept; image data unchanged.
        assert!(s.data.windows(11).any(|w| w == b"ICC_PROFILE"));
        let a = image::load_from_memory(&base).unwrap().to_rgb8();
        let b = image::load_from_memory(&s.data).unwrap().to_rgb8();
        assert_eq!(a, b);
        // Nothing left to do the second time.
        assert!(strip(&s.data).unwrap().is_none());
    }

    #[test]
    fn rotated_files_keep_only_orientation() {
        let base = sample_jpeg(64, 48);
        let mut j = with_segment(&base, 0xE1, &exif(6));
        j = with_segment(&j, 0xFE, b"comment");
        let s = strip(&j).unwrap().unwrap();
        assert!(s.kept_orientation);
        assert_eq!(scan(&s.data), vec!["Orientation"]);
        assert_eq!(crate::jpegtran::orientation(&s.data), 6);
        // Pixel data untouched.
        let a = image::load_from_memory(&base).unwrap().to_rgb8();
        let b = image::load_from_memory(&s.data).unwrap().to_rgb8();
        assert_eq!(a, b);
        assert!(strip(&s.data).unwrap().is_none());

        // PNG: the new eXIf chunk has a valid CRC and decodes rotated.
        let img = image::RgbaImage::from_pixel(8, 4, image::Rgba([1, 2, 3, 255]));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let mut p = png[..33].to_vec();
        p.extend_from_slice(&((exif(6).len() - 6) as u32).to_be_bytes());
        p.extend_from_slice(b"eXIf");
        p.extend_from_slice(&exif(6)[6..]);
        p.extend_from_slice(&[0, 0, 0, 0]);
        p.extend_from_slice(&png[33..]);
        let s = strip(&p).unwrap().unwrap();
        assert_eq!(scan(&s.data), vec!["Orientation"]);
        let c = png_chunks(&s.data).into_iter().find(|c| c.ty == b"eXIf").unwrap();
        let crc = u32::from_be_bytes(c.raw[c.raw.len() - 4..].try_into().unwrap());
        assert_eq!(crc, crc32(&c.raw[4..c.raw.len() - 4]));
        let path = std::env::temp_dir().join("sfiv-test-orient.png");
        std::fs::write(&path, &s.data).unwrap();
        assert_eq!(crate::loader::decode(&path).unwrap().image.dimensions(), (4, 8));
    }

    /// `SFIV_SAMPLES=<dir> cargo test strip_samples -- --nocapture`: strips
    /// copies of real files and checks the pixels survive.
    #[test]
    fn strip_samples() {
        let Some(dir) = std::env::var_os("SFIV_SAMPLES") else { return };
        let tmp = std::env::temp_dir().join("sfiv-test-strip");
        std::fs::create_dir_all(&tmp).unwrap();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let src = e.path();
            if !is_supported(&src) {
                continue;
            }
            let dst = tmp.join(e.file_name());
            let before = std::fs::read(&src).unwrap();
            strip_file(&src, &dst).unwrap();
            let after = std::fs::read(&dst).unwrap();
            let a = crate::loader::decode(&src).unwrap();
            let b = crate::loader::decode(&dst).unwrap();
            assert!(a.image == b.image, "{src:?} pixels changed");
            println!("{:?}: {:?} -> {:?}, {} -> {} bytes", e.file_name(), scan(&before), scan(&after), before.len(), after.len());
        }
    }

    #[test]
    fn strips_png_and_webp() {
        let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        // Insert a tEXt and an eXIf chunk after IHDR (8 + 25 bytes).
        let chunk = |ty: &[u8], body: &[u8]| {
            let mut c = (body.len() as u32).to_be_bytes().to_vec();
            c.extend_from_slice(ty);
            c.extend_from_slice(body);
            c.extend_from_slice(&[0, 0, 0, 0]); // CRC isn't checked here
            c
        };
        let mut p = png[..33].to_vec();
        p.extend(chunk(b"tEXt", b"Author\0me"));
        p.extend(chunk(b"eXIf", &exif(1)[6..]));
        p.extend_from_slice(&png[33..]);
        assert_eq!(scan(&p), vec!["Text", "Orientation"]);
        let s = strip(&p).unwrap().unwrap();
        assert_eq!(s.data, png);

        let webp = webp::Encoder::from_rgba(img.as_raw(), 8, 8).encode_lossless().to_vec();
        let mut w = webp.clone();
        let x = exif(1);
        w.extend_from_slice(b"EXIF");
        w.extend_from_slice(&(x.len() as u32).to_le_bytes());
        w.extend_from_slice(&x);
        let size = (w.len() - 8) as u32;
        w[4..8].copy_from_slice(&size.to_le_bytes());
        assert_eq!(scan(&w), vec!["Orientation"]);
        assert_eq!(strip(&w).unwrap().unwrap().data, webp);
    }
}
