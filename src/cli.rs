//! Command-line mode for scripts and AI agents:
//! `superfastimageview <command> [options] <inputs...>`.
//!
//! `main` checks the first argument with [`is_command`] before any GUI setup,
//! so opening an image is unaffected. Nothing here touches the window system.

use std::ffi::{OsStr, OsString};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use image::codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder};
use image::{AnimationDecoder, ImageDecoder, ImageFormat, ImageReader, RgbaImage};
use rayon::prelude::*;

use crate::convert::{self, OutFormat, PngLevel, SaveOptions};
use crate::edit::{self, ResizeFilter};
use crate::jpegtran;

const EXIT_FAILED: i32 = 1;
const EXIT_USAGE: i32 = 2;

const COMMANDS: &[&str] =
    &["info", "convert", "resize", "rotate", "flip", "strip", "compress", "help", "--help", "-h", "--version"];

/// Whether the first argument starts command-line mode rather than naming an
/// image to open. A file of that name in the current folder still opens.
pub fn is_command(arg: &OsStr) -> bool {
    arg.to_str().is_some_and(|s| COMMANDS.contains(&s)) && !Path::new(arg).exists()
}

/// The name this program was started as, for the help text.
fn prog() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .filter(|s| s.to_ascii_lowercase().starts_with("superfastimageview"))
        .unwrap_or_else(|| "superfastimageview".into())
}

fn help() -> String {
    HELP.replace("superfastimageview ", &format!("{} ", prog()))
}

/// Runs the command line in `std::env::args_os()` and returns the exit code.
pub fn main() -> i32 {
    #[cfg(windows)]
    attach_console();
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let code = run(args, &mut std::io::stdout().lock(), &mut std::io::stderr().lock());
    let _ = std::io::stdout().flush();
    code
}

/// Release builds use the GUI subsystem on Windows, so there is no console.
/// When output isn't redirected, write to the console we were started from.
#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::*;
    unsafe {
        let missing = |id| {
            let h = GetStdHandle(id);
            h.is_null() || h == INVALID_HANDLE_VALUE
        };
        if missing(STD_OUTPUT_HANDLE) || missing(STD_ERROR_HANDLE) {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

const HELP: &str = r#"SuperFastImageView command line

Usage: superfastimageview <command> [options] <inputs...>

Inputs are files, folders (their images; -r includes subfolders) or wildcards
like "shots/*.png". Wildcards are expanded here, so they work in any shell
(quote them).

Commands
  info     <inputs>                   Size, format, color type, alpha, frame count,
                                      EXIF orientation and metadata of each image
  convert  <inputs> -f FORMAT         Convert to another format, optionally with edits
  resize   <inputs> SIZE-OPTION       Resize; keeps each file's format unless -f
  rotate   cw|ccw|180|auto <inputs>   Rotate. JPEGs are rotated losslessly. "auto"
                                      applies the EXIF orientation to the pixels
  flip     h|v <inputs>               Flip; lossless for JPEGs
  strip    <inputs>                   Remove metadata (EXIF, XMP, IPTC, comments)
                                      losslessly from JPEG, PNG and WebP. Keeps the
                                      color profile and the orientation
  compress <inputs> --target-size N   Highest JPEG/WebP quality that fits in N bytes
  help                                This text

Output (every command but info)
  Without -o, results go next to each input as NAME.EXT. Nothing is ever
  replaced unless --in-place or --overwrite says so.
  -o, --out PATH       Output folder (created if needed; -r keeps subfolders),
                       or the output file name when there is one input
  --pattern PATTERN    Output name without extension. Tokens: {name} {n} {parent}
                       {date} {ext} {w} {h}. Default: {name}
  --in-place           Replace the input files (same format only)
  --overwrite          Replace existing output files
  --dry-run            Show what would be written; write nothing
  --json               Print a JSON array with one object per input
  -r, --recursive      Include subfolders of folder inputs

Encoding
  -f, --format FMT     jpg png webp bmp gif tif tga qoi ico ppm. Default: the
                       extension of -o FILE, else the input's format (RAW and
                       HEIC/AVIF become jpg). convert requires one of these
  -q, --quality 1-100  JPEG (default 90) and lossy WebP (default 85)
  --lossless           Lossless WebP
  --png-level LEVEL    fast (default), default, best
  --target-size SIZE   Highest quality that fits, e.g. 200KB, 1.5MB, 50000
                       (KB = 1000 bytes). JPEG/WebP only; -q sets the maximum
                       quality (default 95). Works with convert and resize too

Edits (convert, resize, compress; applied in this order)
  --crop X,Y,W,H       Crop, in pixels of the upright image
  --rotate cw|ccw|180  Rotate
  --flip h|v           Flip
  Size, one of:
    --max N            Longest side at most N (never enlarges)
    --fit WxH          Fit inside WxH keeping the aspect ratio (never enlarges)
    --width N          Width N; height keeps the aspect ratio
    --height N         Height N; width keeps the aspect ratio
    --size WxH         Exactly WxH (may distort)
    --percent P        Scale by P percent
  --filter F           lanczos (default), bicubic, bilinear, nearest
  --grayscale          Convert to grayscale

Notes
  Reads JPEG, PNG/APNG, GIF, WebP, BMP, ICO, TIFF, TGA, QOI, PNM, HDR, EXR,
  DDS, Farbfeld, HEIC/AVIF (system codecs) and camera RAW (full resolution).
  EXIF orientation is applied when reading. Re-encoded files keep no metadata
  (EXIF, color profile), and animations become their first frame.
  Inputs are processed in parallel; results are printed in input order.
  Exit status: 0 success, 1 some inputs failed, 2 bad usage.

Windows
  Use superfastimageview-cli.exe, the console version, from scripts and
  PowerShell. SuperFastImageView.exe takes the same commands, but it's a GUI
  program, which PowerShell doesn't wait for (output and $LASTEXITCODE get
  lost unless the output is piped).

Examples
  superfastimageview info --json photos/
  superfastimageview convert "*.png" -f webp -q 80 -o web/
  superfastimageview resize big.jpg --max 1568 -o small.jpg
  superfastimageview rotate auto -r photos --in-place
  superfastimageview compress hero.jpg --target-size 200KB -o hero-small.jpg
  superfastimageview strip --in-place "*.jpg"
"#;

// ---------------------------------------------------------------------------
// Options

#[derive(Clone, Copy, PartialEq, Debug)]
enum Resize {
    Max(u32),
    Fit(u32, u32),
    Width(u32),
    Height(u32),
    Exact(u32, u32),
    Percent(f64),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Rotate {
    Cw,
    Ccw,
    Half,
    /// Apply the EXIF orientation (the `rotate auto` command).
    Auto,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Flip {
    H,
    V,
}

#[derive(Debug)]
struct Opts {
    cmd: String,
    inputs: Vec<String>,
    recursive: bool,
    json: bool,
    dry_run: bool,
    overwrite: bool,
    in_place: bool,
    out: Option<PathBuf>,
    pattern: Option<String>,
    format: Option<OutFormat>,
    quality: Option<u8>,
    lossless: bool,
    png_level: Option<PngLevel>,
    resize: Option<Resize>,
    filter: ResizeFilter,
    crop: Option<[u32; 4]>,
    rotate: Option<Rotate>,
    flip: Option<Flip>,
    grayscale: bool,
    target_size: Option<u64>,
}

const OUT_FLAGS: &[&str] = &["recursive", "json", "dry-run", "overwrite", "in-place", "out", "pattern"];
const ENCODE_FLAGS: &[&str] = &["format", "quality", "lossless", "png-level", "target-size"];
const EDIT_FLAGS: &[&str] = &["resize", "filter", "crop", "rotate", "flip", "grayscale"];

fn allowed(cmd: &str, flag: &str) -> bool {
    match cmd {
        "info" => matches!(flag, "recursive" | "json"),
        "convert" | "resize" | "compress" => {
            OUT_FLAGS.contains(&flag) || ENCODE_FLAGS.contains(&flag) || EDIT_FLAGS.contains(&flag)
        }
        "rotate" | "flip" => OUT_FLAGS.contains(&flag) || (ENCODE_FLAGS.contains(&flag) && flag != "target-size"),
        "strip" => OUT_FLAGS.contains(&flag),
        _ => false,
    }
}

fn parse_u32(s: &str, what: &str) -> Result<u32, String> {
    match s.trim().parse::<u32>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(format!("{what}: expected a positive whole number, got \"{s}\"")),
    }
}

fn parse_wxh(s: &str, what: &str) -> Result<(u32, u32), String> {
    let (w, h) = s.split_once(['x', 'X']).ok_or_else(|| format!("{what}: expected WxH, got \"{s}\""))?;
    Ok((parse_u32(w, what)?, parse_u32(h, what)?))
}

/// "200KB", "1.5MB", "200k", "50000". Decimal units: KB = 1000 bytes.
fn parse_size(s: &str) -> Result<u64, String> {
    let t = s.trim().to_ascii_lowercase();
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let mult = match unit.trim() {
        "" | "b" => 1.0,
        "k" | "kb" => 1e3,
        "m" | "mb" => 1e6,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        _ => return Err(format!("--target-size: unknown unit in \"{s}\" (use B, KB, MB, KiB or MiB)")),
    };
    match num.parse::<f64>() {
        Ok(n) if n > 0.0 => Ok((n * mult) as u64),
        _ => Err(format!("--target-size: expected a size like 200KB, got \"{s}\"")),
    }
}

fn parse_format(s: &str) -> Result<OutFormat, String> {
    let s = s.trim().trim_start_matches('.').to_ascii_lowercase();
    OutFormat::ALL
        .into_iter()
        .find(|f| f.extensions().contains(&s.as_str()) || (s == "pnm" && *f == OutFormat::Pnm))
        .ok_or_else(|| format!("unknown output format \"{s}\" (use jpg png webp bmp gif tif tga qoi ico ppm)"))
}

fn parse_rotate(s: &str, allow_auto: bool) -> Result<Rotate, String> {
    match s.to_ascii_lowercase().as_str() {
        "cw" | "90" | "right" => Ok(Rotate::Cw),
        "ccw" | "-90" | "270" | "left" => Ok(Rotate::Ccw),
        "180" => Ok(Rotate::Half),
        "auto" if allow_auto => Ok(Rotate::Auto),
        _ => Err(format!("unknown rotation \"{s}\" (use cw, ccw, 180{})", if allow_auto { " or auto" } else { "" })),
    }
}

fn parse_flip(s: &str) -> Result<Flip, String> {
    match s.to_ascii_lowercase().as_str() {
        "h" | "horizontal" => Ok(Flip::H),
        "v" | "vertical" => Ok(Flip::V),
        _ => Err(format!("unknown flip \"{s}\" (use h or v)")),
    }
}

fn parse(cmd: &str, args: Vec<String>) -> Result<Opts, String> {
    let mut o = Opts {
        cmd: cmd.to_string(),
        inputs: Vec::new(),
        recursive: false,
        json: false,
        dry_run: false,
        overwrite: false,
        in_place: false,
        out: None,
        pattern: None,
        format: None,
        quality: None,
        lossless: false,
        png_level: None,
        resize: None,
        filter: ResizeFilter::Lanczos3,
        crop: None,
        rotate: None,
        flip: None,
        grayscale: false,
        target_size: None,
    };
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "--" {
            o.inputs.extend(it.by_ref());
            break;
        }
        if !arg.starts_with('-') || arg == "-" {
            o.inputs.push(arg);
            continue;
        }
        let (flag, mut inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = || inline.take().or_else(|| it.next()).ok_or_else(|| format!("{flag} needs a value"));
        let name = match flag.as_str() {
            "-r" | "--recursive" => {
                o.recursive = true;
                "recursive"
            }
            "--json" => {
                o.json = true;
                "json"
            }
            "--dry-run" => {
                o.dry_run = true;
                "dry-run"
            }
            "--overwrite" => {
                o.overwrite = true;
                "overwrite"
            }
            "--in-place" => {
                o.in_place = true;
                "in-place"
            }
            "-o" | "--out" | "--output" => {
                o.out = Some(PathBuf::from(value()?));
                "out"
            }
            "--pattern" => {
                o.pattern = Some(value()?);
                "pattern"
            }
            "-f" | "--format" => {
                o.format = Some(parse_format(&value()?)?);
                "format"
            }
            "-q" | "--quality" => {
                let v = value()?;
                match v.trim().parse::<u8>() {
                    Ok(q @ 1..=100) => o.quality = Some(q),
                    _ => return Err(format!("--quality: expected 1 to 100, got \"{v}\"")),
                }
                "quality"
            }
            "--lossless" => {
                o.lossless = true;
                "lossless"
            }
            "--png-level" => {
                o.png_level = Some(match value()?.to_ascii_lowercase().as_str() {
                    "fast" => PngLevel::Fast,
                    "default" => PngLevel::Default,
                    "best" => PngLevel::Best,
                    v => return Err(format!("--png-level: use fast, default or best, not \"{v}\"")),
                });
                "png-level"
            }
            "--max" | "--fit" | "--width" | "--height" | "--size" | "--percent" => {
                if o.resize.is_some() {
                    return Err("give only one of --max, --fit, --width, --height, --size, --percent".into());
                }
                let v = value()?;
                o.resize = Some(match flag.as_str() {
                    "--max" => Resize::Max(parse_u32(&v, &flag)?),
                    "--fit" => {
                        let (w, h) = parse_wxh(&v, &flag)?;
                        Resize::Fit(w, h)
                    }
                    "--width" => Resize::Width(parse_u32(&v, &flag)?),
                    "--height" => Resize::Height(parse_u32(&v, &flag)?),
                    "--size" => {
                        let (w, h) = parse_wxh(&v, &flag)?;
                        Resize::Exact(w, h)
                    }
                    _ => match v.trim().trim_end_matches('%').parse::<f64>() {
                        Ok(p) if p > 0.0 && p.is_finite() => Resize::Percent(p),
                        _ => return Err(format!("--percent: expected a positive number, got \"{v}\"")),
                    },
                });
                "resize"
            }
            "--filter" => {
                o.filter = match value()?.to_ascii_lowercase().as_str() {
                    "lanczos" | "lanczos3" => ResizeFilter::Lanczos3,
                    "bicubic" | "cubic" => ResizeFilter::Bicubic,
                    "bilinear" | "linear" => ResizeFilter::Bilinear,
                    "nearest" => ResizeFilter::Nearest,
                    v => return Err(format!("--filter: use lanczos, bicubic, bilinear or nearest, not \"{v}\"")),
                };
                "filter"
            }
            "--crop" => {
                let v = value()?;
                let parts: Vec<&str> = v.split(',').collect();
                let bad = || format!("--crop: expected X,Y,W,H, got \"{v}\"");
                if parts.len() != 4 {
                    return Err(bad());
                }
                let mut c = [0u32; 4];
                for (slot, p) in c.iter_mut().zip(&parts) {
                    *slot = p.trim().parse().map_err(|_| bad())?;
                }
                if c[2] == 0 || c[3] == 0 {
                    return Err(bad());
                }
                o.crop = Some(c);
                "crop"
            }
            "--rotate" => {
                o.rotate = Some(parse_rotate(&value()?, false)?);
                "rotate"
            }
            "--flip" => {
                o.flip = Some(parse_flip(&value()?)?);
                "flip"
            }
            "--grayscale" | "--greyscale" => {
                o.grayscale = true;
                "grayscale"
            }
            "--target-size" => {
                o.target_size = Some(parse_size(&value()?)?);
                "target-size"
            }
            _ => return Err(format!("unknown option {flag}")),
        };
        if !allowed(cmd, name) {
            return Err(format!("{flag} is not an option of `{cmd}`"));
        }
    }

    // The direction argument of rotate/flip.
    if cmd == "rotate" || cmd == "flip" {
        if o.inputs.is_empty() {
            return Err(format!("`{cmd}` needs a direction: {}", if cmd == "rotate" { "cw, ccw, 180 or auto" } else { "h or v" }));
        }
        let dir = o.inputs.remove(0);
        if cmd == "rotate" {
            o.rotate = Some(parse_rotate(&dir, true)?);
        } else {
            o.flip = Some(parse_flip(&dir)?);
        }
    }
    if o.inputs.is_empty() {
        return Err("no input files".into());
    }
    if cmd == "resize" && o.resize.is_none() {
        return Err("`resize` needs a size: --max, --fit, --width, --height, --size or --percent".into());
    }
    if cmd == "compress" && o.target_size.is_none() {
        return Err("`compress` needs --target-size, e.g. --target-size 200KB".into());
    }
    if o.target_size.is_some() && o.lossless {
        return Err("--target-size can't be combined with --lossless".into());
    }
    if o.in_place && (o.out.is_some() || o.pattern.is_some()) {
        return Err("--in-place can't be combined with -o or --pattern".into());
    }
    if let Some(f) = o.format
        && o.target_size.is_some()
        && !matches!(f, OutFormat::Jpeg | OutFormat::Webp)
    {
        return Err("--target-size needs JPEG or WebP output".into());
    }
    Ok(o)
}

// ---------------------------------------------------------------------------
// Inputs

struct Input {
    path: PathBuf,
    /// Subfolder below the folder argument it was found in (for -o with -r).
    sub: PathBuf,
}

/// Case-insensitive `*` / `?` match.
fn wildcard(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let s: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, si));
            pi += 1;
        } else if let Some((sp, ss)) = star {
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, ss + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

fn walk(dir: &Path, sub: &Path, recursive: bool, out: &mut Vec<Input>) {
    out.extend(crate::loader::list_images(dir).into_iter().map(|path| Input { path, sub: sub.to_path_buf() }));
    if !recursive {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut dirs: Vec<(String, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| (e.file_name().to_string_lossy().to_lowercase(), e.path()))
        .collect();
    dirs.sort_by(|a, b| natord::compare(&a.0, &b.0));
    for (_, d) in dirs {
        let name = d.file_name().unwrap_or_default().to_owned();
        walk(&d, &sub.join(name), recursive, out);
    }
}

/// Expands folders and wildcards. Returns the inputs and warnings.
fn expand(args: &[String], recursive: bool) -> (Vec<Input>, Vec<String>) {
    let mut inputs = Vec::new();
    let mut warnings = Vec::new();
    for arg in args {
        let path = Path::new(arg);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if (name.contains('*') || name.contains('?')) && !path.exists() {
            let parent = path.parent().unwrap_or(Path::new(""));
            let read_dir = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
            let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(read_dir)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| wildcard(&name, n))
                .map(|n| (n.to_lowercase(), parent.join(&n)))
                .filter(|(_, p)| crate::loader::is_supported(p))
                .collect();
            if found.is_empty() {
                warnings.push(format!("no images match {arg}"));
            }
            found.sort_by(|a, b| natord::compare(&a.0, &b.0));
            inputs.extend(found.into_iter().map(|(_, path)| Input { path, sub: PathBuf::new() }));
        } else if path.is_dir() {
            let before = inputs.len();
            walk(path, Path::new(""), recursive, &mut inputs);
            if inputs.len() == before {
                warnings.push(format!("no images in {arg}"));
            }
        } else {
            // Missing files are reported per input, so the rest still run.
            inputs.push(Input { path: path.to_path_buf(), sub: PathBuf::new() });
        }
    }
    let mut seen = std::collections::HashSet::new();
    inputs.retain(|i| seen.insert(i.path.clone()));
    (inputs, warnings)
}

// ---------------------------------------------------------------------------
// Results

enum Val {
    Str(String),
    Num(u64),
    Bool(bool),
    List(Vec<String>),
}

struct Res {
    input: PathBuf,
    output: Option<PathBuf>,
    error: Option<String>,
    fields: Vec<(&'static str, Val)>,
    /// Human-readable details for the text output.
    text: String,
}

impl Res {
    fn failed(input: &Path, error: String) -> Res {
        Res { input: input.to_path_buf(), output: None, error: Some(error), fields: Vec::new(), text: String::new() }
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn to_json(r: &Res) -> String {
    let mut parts = vec![
        format!("\"input\":{}", json_str(&r.input.to_string_lossy())),
        format!("\"ok\":{}", r.error.is_none()),
    ];
    if let Some(o) = &r.output {
        parts.push(format!("\"output\":{}", json_str(&o.to_string_lossy())));
    }
    for (k, v) in &r.fields {
        let v = match v {
            Val::Str(s) => json_str(s),
            Val::Num(n) => n.to_string(),
            Val::Bool(b) => b.to_string(),
            Val::List(l) => format!("[{}]", l.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(",")),
        };
        parts.push(format!("\"{k}\":{v}"));
    }
    if let Some(e) = &r.error {
        parts.push(format!("\"error\":{}", json_str(e)));
    }
    format!("{{{}}}", parts.join(","))
}

fn human_size(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.2} MB", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.1} KB", n as f64 / 1e3),
        n => format!("{n} B"),
    }
}

// ---------------------------------------------------------------------------
// Running

pub fn run(args: Vec<OsString>, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let args: Vec<String> = match args.into_iter().map(|a| a.into_string()).collect() {
        Ok(a) => a,
        Err(bad) => {
            let _ = writeln!(err, "error: argument is not valid Unicode: {}", bad.to_string_lossy());
            return EXIT_USAGE;
        }
    };
    let Some((cmd, rest)) = args.split_first() else {
        let _ = write!(out, "{}", help());
        return 0;
    };
    match cmd.as_str() {
        "help" | "--help" | "-h" => {
            let _ = write!(out, "{}", help());
            return 0;
        }
        "--version" => {
            let _ = writeln!(out, "{} {}", prog(), env!("CARGO_PKG_VERSION"));
            return 0;
        }
        _ => {}
    }
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        let _ = write!(out, "{}", help());
        return 0;
    }
    let opts = match parse(cmd, rest.to_vec()) {
        Ok(o) => o,
        Err(e) => {
            let _ = writeln!(err, "error: {e}\nRun `superfastimageview help` for usage.");
            return EXIT_USAGE;
        }
    };

    let (inputs, warnings) = expand(&opts.inputs, opts.recursive);
    for w in &warnings {
        let _ = writeln!(err, "warning: {w}");
    }
    if inputs.is_empty() {
        let _ = writeln!(err, "error: no input images");
        return EXIT_FAILED;
    }
    // -o names one file: only valid with a single input.
    let target = file_target(&opts, inputs.len());
    if inputs.len() > 1
        && let Some(o) = &opts.out
        && !o.is_dir()
        && OutFormat::from_path(o).is_some()
    {
        let _ = writeln!(err, "error: -o {} looks like a file, but there are {} inputs; give a folder", o.display(), inputs.len());
        return EXIT_USAGE;
    }
    if opts.cmd == "convert" && opts.format.is_none() && target.as_deref().and_then(OutFormat::from_path).is_none() {
        let _ = writeln!(err, "error: `convert` needs -f FORMAT (or -o FILE with an image extension)");
        return EXIT_USAGE;
    }

    let conflicts = if opts.cmd == "info" { vec![None; inputs.len()] } else { find_conflicts(&opts, &inputs, target.as_deref()) };
    let total = inputs.len();
    let results: Vec<Res> = inputs
        .par_iter()
        .enumerate()
        .map(|(i, inp)| {
            if let Some(c) = &conflicts[i] {
                return Res::failed(&inp.path, c.clone());
            }
            let r = match opts.cmd.as_str() {
                "info" => info(&inp.path),
                "strip" => strip(&opts, inp, i, total, target.as_deref()),
                _ => process(&opts, inp, i, total, target.as_deref()),
            };
            r.unwrap_or_else(|e| Res::failed(&inp.path, e))
        })
        .collect();

    let failed = results.iter().filter(|r| r.error.is_some()).count();
    if opts.json {
        let _ = writeln!(out, "[");
        for (i, r) in results.iter().enumerate() {
            let _ = writeln!(out, "{}{}", to_json(r), if i + 1 < results.len() { "," } else { "" });
        }
        let _ = writeln!(out, "]");
    } else {
        for r in &results {
            let input = r.input.display();
            let _ = match (&r.error, &r.output) {
                (Some(e), _) => writeln!(out, "FAILED {input}: {e}"),
                (None, Some(o)) => writeln!(out, "{input} -> {}  {}", o.display(), r.text),
                (None, None) => writeln!(out, "{input}: {}", r.text),
            };
        }
        if total > 1 || failed > 0 {
            let _ = writeln!(err, "{} ok, {failed} failed", total - failed);
        }
    }
    if failed > 0 { EXIT_FAILED } else { 0 }
}

/// `-o` as an output file: one input, and not a folder.
fn file_target(o: &Opts, inputs: usize) -> Option<PathBuf> {
    let out = o.out.as_ref()?;
    let s = out.to_string_lossy();
    if inputs != 1 || out.is_dir() || s.ends_with('/') || s.ends_with('\\') || out.extension().is_none() {
        return None;
    }
    Some(out.clone())
}

fn input_ext(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The output format for `input` when -f isn't given.
fn out_format(o: &Opts, input: &Path, target: Option<&Path>) -> Result<OutFormat, String> {
    if let Some(f) = o.format {
        return Ok(f);
    }
    if let Some(t) = target {
        return OutFormat::from_path(t).ok_or_else(|| format!("can't tell the format from {}; add -f", t.display()));
    }
    Ok(OutFormat::from_path(input).unwrap_or_else(|| {
        // Formats we can read but not write: photos become JPEG, the rest PNG.
        if crate::raw::is_raw(input) || crate::platform::is_platform_format(input) { OutFormat::Jpeg } else { OutFormat::Png }
    }))
}

/// The input's own extension if it already names `fmt` (keeps ".jpeg").
fn ext_for(fmt: OutFormat, input: &Path) -> String {
    let ext = input_ext(input);
    if fmt.extensions().contains(&ext.to_ascii_lowercase().as_str()) { ext } else { fmt.ext().to_string() }
}

fn out_path(o: &Opts, inp: &Input, index: usize, total: usize, ext: &str, target: Option<&Path>, dims: Option<(u32, u32)>) -> PathBuf {
    if o.in_place {
        return inp.path.clone();
    }
    if let Some(t) = target {
        return t.to_path_buf();
    }
    let dir = match &o.out {
        Some(d) => d.join(&inp.sub),
        None => inp.path.parent().unwrap_or(Path::new("")).to_path_buf(),
    };
    let pattern = o.pattern.as_deref().unwrap_or("{name}");
    dir.join(format!("{}.{ext}", convert::output_name(pattern, &inp.path, index, total, 1, dims)))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn check_out(o: &Opts, input: &Path, out: &Path) -> Result<(), String> {
    if o.in_place {
        return Ok(());
    }
    if same_file(input, out) {
        return Err(format!(
            "the output {} would replace the input; use --in-place, -o FOLDER or --pattern",
            out.display()
        ));
    }
    if !o.overwrite && out.exists() {
        return Err(format!("{} already exists (use --overwrite)", out.display()));
    }
    Ok(())
}

/// Inputs that would write the same output file as an earlier input.
fn find_conflicts(o: &Opts, inputs: &[Input], target: Option<&Path>) -> Vec<Option<String>> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let pattern = o.pattern.as_deref().unwrap_or("{name}");
    inputs
        .iter()
        .enumerate()
        .map(|(i, inp)| {
            if o.in_place || convert::needs_dims(pattern) {
                return None;
            }
            let ext = if o.cmd == "strip" {
                input_ext(&inp.path)
            } else {
                ext_for(out_format(o, &inp.path, target).ok()?, &inp.path)
            };
            let p = out_path(o, inp, i, inputs.len(), &ext, target, None);
            let key = p.to_string_lossy().to_lowercase();
            match seen.get(&key) {
                Some(&first) => Some(format!(
                    "{} would also be written by {}; use --pattern (e.g. \"{{name}}_{{ext}}\")",
                    p.display(),
                    inputs[first].path.display()
                )),
                None => {
                    seen.insert(key, i);
                    None
                }
            }
        })
        .collect()
}

/// Writes via a temporary file and rename, so a failure never leaves a
/// half-written file (or destroys an input being replaced).
fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".sfiv-tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, data).map_err(|e| format!("can't write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("can't write {}: {e}", path.display())
    })
}

fn dry_run(input: &Path, out: PathBuf) -> Res {
    Res {
        input: input.to_path_buf(),
        output: Some(out),
        error: None,
        fields: vec![("dry_run", Val::Bool(true))],
        text: "(dry run)".into(),
    }
}

// ---------------------------------------------------------------------------
// info

fn count_frames(fmt: ImageFormat, data: &[u8]) -> Option<usize> {
    let n = match fmt {
        ImageFormat::Gif => GifDecoder::new(Cursor::new(data)).ok()?.into_frames().take_while(|f| f.is_ok()).count(),
        ImageFormat::WebP => {
            let d = WebPDecoder::new(Cursor::new(data)).ok()?;
            if !d.has_animation() {
                return Some(1);
            }
            d.into_frames().take_while(|f| f.is_ok()).count()
        }
        ImageFormat::Png => {
            let d = PngDecoder::new(Cursor::new(data)).ok()?;
            if !d.is_apng().ok()? {
                return Some(1);
            }
            d.apng().ok()?.into_frames().take_while(|f| f.is_ok()).count()
        }
        _ => 1,
    };
    Some(n.max(1))
}

fn info(path: &Path) -> Result<Res, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let file_size = data.len() as u64;
    let metadata: Vec<String> = crate::metadata::scan(&data).into_iter().map(String::from).collect();

    // Fast path: read only the header.
    let header = (!crate::raw::is_raw(path) && !crate::platform::is_platform_format(path) && !crate::platform::sniff(&data))
        .then(|| ImageReader::new(Cursor::new(&data[..])).with_guessed_format().ok())
        .flatten()
        .and_then(|r| {
            let fmt = r.format()?;
            let mut dec = r.into_decoder().ok()?;
            let orientation = dec.orientation().map_or(1, |o| o.to_exif() as u16);
            Some((fmt, dec.dimensions(), dec.color_type(), orientation))
        });

    let (format, (w, h), color, has_alpha, frames, orientation) = match header {
        Some((fmt, (w, h), color, orientation)) => {
            // Report the size as displayed, after EXIF rotation.
            let dims = if orientation >= 5 { (h, w) } else { (w, h) };
            let frames = count_frames(fmt, &data).unwrap_or(1);
            (format!("{fmt:?}").to_uppercase(), dims, format!("{color:?}"), color.has_alpha(), frames, orientation)
        }
        None => {
            let d = crate::loader::decode(path)?;
            let orientation = if crate::jpegtran::is_jpeg(path) { jpegtran::orientation(&data) } else { 1 };
            (d.format.clone(), d.image.dimensions(), "Rgba8".to_string(), d.has_alpha, 1, orientation)
        }
    };

    let mut text = format!("{format} {w}x{h}, {color}, {}", human_size(file_size));
    if frames > 1 {
        text += &format!(", {frames} frames");
    }
    if orientation != 1 {
        text += &format!(", EXIF orientation {orientation}");
    }
    if !metadata.is_empty() {
        text += &format!(", metadata: {}", metadata.join(", "));
    }
    Ok(Res {
        input: path.to_path_buf(),
        output: None,
        error: None,
        fields: vec![
            ("format", Val::Str(format)),
            ("width", Val::Num(w as u64)),
            ("height", Val::Num(h as u64)),
            ("color", Val::Str(color)),
            ("has_alpha", Val::Bool(has_alpha)),
            ("frames", Val::Num(frames as u64)),
            ("orientation", Val::Num(orientation as u64)),
            ("file_size", Val::Num(file_size)),
            ("metadata", Val::List(metadata)),
        ],
        text,
    })
}

// ---------------------------------------------------------------------------
// strip

fn strip(o: &Opts, inp: &Input, index: usize, total: usize, target: Option<&Path>) -> Result<Res, String> {
    let out = out_path(o, inp, index, total, &input_ext(&inp.path), target, None);
    check_out(o, &inp.path, &out)?;
    if o.dry_run {
        return Ok(dry_run(&inp.path, out));
    }
    let data = std::fs::read(&inp.path).map_err(|e| e.to_string())?;
    let found: Vec<String> = crate::metadata::scan(&data).into_iter().map(String::from).collect();
    let (removed, kept_orientation) = match crate::metadata::strip(&data)? {
        Some(s) => {
            write_atomic(&out, &s.data)?;
            (s.removed as u64, s.kept_orientation)
        }
        None => {
            if !o.in_place {
                write_atomic(&out, &data)?;
            }
            (0, false)
        }
    };
    let mut text = if removed > 0 { format!("removed {}", human_size(removed)) } else { "no metadata to remove".into() };
    if !found.is_empty() {
        text += &format!(" (had: {})", found.join(", "));
    }
    if kept_orientation {
        text += ", kept orientation";
    }
    Ok(Res {
        input: inp.path.clone(),
        output: Some(out),
        error: None,
        fields: vec![
            ("removed_bytes", Val::Num(removed)),
            ("found", Val::List(found)),
            ("kept_orientation", Val::Bool(kept_orientation)),
        ],
        text,
    })
}

// ---------------------------------------------------------------------------
// convert / resize / rotate / flip / compress

fn resized_dims(r: Resize, w: u32, h: u32) -> Option<(u32, u32)> {
    let scale = |s: f64| (((w as f64) * s).round().max(1.0) as u32, ((h as f64) * s).round().max(1.0) as u32);
    match r {
        Resize::Max(n) => {
            let m = w.max(h);
            (m > n).then(|| scale(n as f64 / m as f64))
        }
        Resize::Fit(bw, bh) => {
            let s = (bw as f64 / w as f64).min(bh as f64 / h as f64);
            (s < 1.0).then(|| scale(s))
        }
        Resize::Width(n) => Some((n, ((h as f64) * n as f64 / w as f64).round().max(1.0) as u32)),
        Resize::Height(n) => Some((((w as f64) * n as f64 / h as f64).round().max(1.0) as u32, n)),
        Resize::Exact(a, b) => Some((a, b)),
        Resize::Percent(p) => Some(scale(p / 100.0)),
    }
}

fn save_options(o: &Opts) -> SaveOptions {
    let d = SaveOptions::default();
    SaveOptions {
        jpeg_quality: o.quality.unwrap_or(d.jpeg_quality),
        webp_quality: o.quality.unwrap_or(d.webp_quality),
        webp_lossless: o.lossless,
        png_level: o.png_level.unwrap_or(d.png_level),
    }
}

/// The highest quality (up to `max_q`) whose encoding fits in `target` bytes.
fn encode_to_size(img: &RgbaImage, fmt: OutFormat, base: SaveOptions, target: u64, max_q: u8) -> Result<(Vec<u8>, u8), String> {
    let (mut lo, mut hi) = (1u8, max_q);
    let mut best = None;
    let mut smallest = u64::MAX;
    while lo <= hi {
        let q = (lo + hi) / 2;
        let opts = SaveOptions { jpeg_quality: q, webp_quality: q, ..base };
        let bytes = convert::encode_vec(img, fmt, &opts)?;
        if bytes.len() as u64 <= target {
            best = Some((bytes, q));
            lo = q + 1;
        } else {
            smallest = smallest.min(bytes.len() as u64);
            hi = q - 1;
        }
    }
    best.ok_or_else(|| {
        format!(
            "can't fit in {} even at quality 1 ({} at {}x{}); make it smaller too, e.g. --max {}",
            human_size(target),
            human_size(smallest),
            img.width(),
            img.height(),
            (img.width().max(img.height()) * 2 / 3).max(1)
        )
    })
}

fn is_jpeg_data(data: &[u8]) -> bool {
    data.starts_with(&[0xFF, 0xD8, 0xFF])
}

fn process(o: &Opts, inp: &Input, index: usize, total: usize, target: Option<&Path>) -> Result<Res, String> {
    let input = &inp.path;
    let fmt = out_format(o, input, target)?;
    let ext = ext_for(fmt, input);
    if o.in_place && !fmt.extensions().contains(&input_ext(input).to_ascii_lowercase().as_str()) {
        return Err(format!("--in-place can't change the format to {}", fmt.label()));
    }
    if o.target_size.is_some() && !matches!(fmt, OutFormat::Jpeg | OutFormat::Webp) {
        return Err(format!("--target-size needs JPEG or WebP output, not {}; add -f jpg or -f webp", fmt.label()));
    }
    let pattern = o.pattern.as_deref().unwrap_or("{name}");
    let early = (!convert::needs_dims(pattern)).then(|| out_path(o, inp, index, total, &ext, target, None));
    if let Some(p) = &early {
        check_out(o, input, p)?;
    }
    if o.dry_run {
        let p = early.unwrap_or_else(|| out_path(o, inp, index, total, &ext, target, None));
        return Ok(dry_run(input, p));
    }
    let data = std::fs::read(input).map_err(|e| e.to_string())?;

    // rotate/flip of a JPEG into a JPEG: rearrange the DCT blocks, no re-encode.
    if (o.cmd == "rotate" || o.cmd == "flip") && fmt == OutFormat::Jpeg && o.quality.is_none() && is_jpeg_data(&data) {
        let (bytes, trimmed) = match (o.rotate, o.flip) {
            (Some(Rotate::Auto), _) => jpegtran::auto_orient(&data)?,
            (Some(Rotate::Cw), _) => jpegtran::transform(&data, jpegtran::Op::RotateCw)?,
            (Some(Rotate::Ccw), _) => jpegtran::transform(&data, jpegtran::Op::RotateCcw)?,
            (Some(Rotate::Half), _) => jpegtran::transform(&data, jpegtran::Op::Rotate180)?,
            (None, Some(Flip::H)) => jpegtran::transform(&data, jpegtran::Op::FlipH)?,
            (None, Some(Flip::V)) => jpegtran::transform(&data, jpegtran::Op::FlipV)?,
            (None, None) => unreachable!("rotate/flip always have a direction"),
        };
        let (w, h) = ImageReader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_dimensions().ok())
            .unwrap_or((0, 0));
        let out = match early {
            Some(p) => p,
            None => {
                let p = out_path(o, inp, index, total, &ext, target, Some((w, h)));
                check_out(o, input, &p)?;
                p
            }
        };
        write_atomic(&out, &bytes)?;
        let mut text = format!("{w}x{h}, {}, lossless", human_size(bytes.len() as u64));
        if trimmed {
            text += " (trimmed partial edge blocks)";
        }
        return Ok(Res {
            input: input.clone(),
            output: Some(out),
            error: None,
            fields: vec![
                ("format", Val::Str(fmt.label().into())),
                ("width", Val::Num(w as u64)),
                ("height", Val::Num(h as u64)),
                ("bytes", Val::Num(bytes.len() as u64)),
                ("lossless", Val::Bool(true)),
                ("trimmed", Val::Bool(trimmed)),
            ],
            text,
        });
    }

    let decoded = crate::loader::decode(input)?;
    drop(data);
    let mut img = std::sync::Arc::try_unwrap(decoded.image).unwrap_or_else(|a| (*a).clone());
    if let Some([x, y, w, h]) = o.crop {
        let (iw, ih) = img.dimensions();
        if x.checked_add(w).is_none_or(|r| r > iw) || y.checked_add(h).is_none_or(|b| b > ih) {
            return Err(format!("--crop {x},{y},{w},{h} is outside the {iw}x{ih} image"));
        }
        img = edit::crop(&img, x, y, w, h);
    }
    match o.rotate {
        Some(Rotate::Cw) => img = edit::rotate_cw(&img),
        Some(Rotate::Ccw) => img = edit::rotate_ccw(&img),
        Some(Rotate::Half) => edit::rotate_180(&mut img),
        Some(Rotate::Auto) | None => {} // Orientation was applied when decoding.
    }
    match o.flip {
        Some(Flip::H) => edit::flip_h(&mut img),
        Some(Flip::V) => edit::flip_v(&mut img),
        None => {}
    }
    if let Some(r) = o.resize
        && let Some((w, h)) = resized_dims(r, img.width(), img.height())
        && (w, h) != img.dimensions()
    {
        img = edit::resize(&img, w, h, o.filter);
    }
    if o.grayscale {
        edit::grayscale(&mut img);
    }

    let (w, h) = img.dimensions();
    let out = match early {
        Some(p) => p,
        None => {
            let p = out_path(o, inp, index, total, &ext, target, Some((w, h)));
            check_out(o, input, &p)?;
            p
        }
    };
    let opts = save_options(o);
    let (bytes, quality) = match o.target_size {
        Some(t) => {
            let (b, q) = encode_to_size(&img, fmt, opts, t, o.quality.unwrap_or(95))?;
            (b, Some(q))
        }
        None => (convert::encode_vec(&img, fmt, &opts)?, None),
    };
    write_atomic(&out, &bytes)?;

    let mut text = format!("{w}x{h} {}, {}", fmt.label(), human_size(bytes.len() as u64));
    let mut fields = vec![
        ("format", Val::Str(fmt.label().into())),
        ("width", Val::Num(w as u64)),
        ("height", Val::Num(h as u64)),
        ("bytes", Val::Num(bytes.len() as u64)),
    ];
    if let Some(q) = quality {
        text += &format!(", quality {q}");
        fields.push(("quality", Val::Num(q as u64)));
    }
    Ok(Res { input: input.clone(), output: Some(out), error: None, fields, text })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_args(args: &[&str]) -> (i32, String, String) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(args.iter().map(OsString::from).collect(), &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sfiv-cli-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn noisy(w: u32, h: u32) -> RgbaImage {
        let mut s = 12345u32;
        RgbaImage::from_fn(w, h, |_, _| {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            image::Rgba([(s >> 16) as u8, (s >> 8) as u8, s as u8, 255])
        })
    }

    #[test]
    fn parsing() {
        assert_eq!(parse_size("200KB").unwrap(), 200_000);
        assert_eq!(parse_size("1.5mb").unwrap(), 1_500_000);
        assert_eq!(parse_size("2KiB").unwrap(), 2048);
        assert_eq!(parse_size("5000").unwrap(), 5000);
        assert!(parse_size("12 parsecs").is_err());
        assert_eq!(parse_format("JPEG").unwrap(), OutFormat::Jpeg);
        assert_eq!(parse_format(".tiff").unwrap(), OutFormat::Tiff);
        assert!(wildcard("*.PNG", "shot_01.png"));
        assert!(wildcard("shot_??.png", "shot_01.png"));
        assert!(!wildcard("*.png", "shot.jpg"));
        assert!(wildcard("*a*b*", "xxaxxbxx"));
        assert_eq!(resized_dims(Resize::Max(100), 400, 200), Some((100, 50)));
        assert_eq!(resized_dims(Resize::Max(1000), 400, 200), None);
        assert_eq!(resized_dims(Resize::Width(200), 400, 100), Some((200, 50)));
        assert_eq!(resized_dims(Resize::Fit(100, 100), 400, 200), Some((100, 50)));
        assert!(parse("resize", vec!["a.png".into()]).is_err());
        assert!(parse("info", vec!["-q".into(), "50".into(), "a.png".into()]).is_err());
        assert!(parse("rotate", vec!["sideways".into(), "a.png".into()]).is_err());
        assert!(is_command(OsStr::new("convert")) && !is_command(OsStr::new("photo.jpg")));
    }

    #[test]
    fn commands_end_to_end() {
        let dir = test_dir("e2e");
        let d = dir.to_string_lossy().to_string();
        for i in 0..3 {
            noisy(300, 200).save(dir.join(format!("img{i}.png"))).unwrap();
        }
        let a = format!("{d}/img0.png");

        // info
        let (code, out, _) = run_args(&["info", "--json", &a]);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("\"width\":300") && out.contains("\"format\":\"PNG\""), "{out}");

        // convert a wildcard into a folder, then again: outputs exist.
        let web = format!("{d}/web");
        let (code, out, err) = run_args(&["convert", &format!("{d}/*.png"), "-f", "webp", "-o", &web]);
        assert_eq!(code, 0, "{out}{err}");
        assert!(dir.join("web/img2.webp").exists());
        let (code, out, _) = run_args(&["convert", &format!("{d}/*.png"), "-f", "webp", "-o", &web]);
        assert_eq!(code, 1);
        assert!(out.contains("already exists"), "{out}");

        // convert without -f is a usage error; resize keeps the format.
        assert_eq!(run_args(&["convert", &a]).0, 2);
        let (code, out, _) = run_args(&["resize", &a, "--max", "150", "--pattern", "{name}_{w}x{h}"]);
        assert_eq!(code, 0, "{out}");
        assert!(dir.join("img0_150x100.png").exists(), "{out}");

        // Writing over the input needs --in-place.
        let (code, out, _) = run_args(&["resize", &a, "--percent", "50"]);
        assert_eq!(code, 1);
        assert!(out.contains("would replace the input"), "{out}");
        let (code, _, _) = run_args(&["resize", &a, "--percent", "50", "--in-place"]);
        assert_eq!(code, 0);
        assert_eq!(image::image_dimensions(&a).unwrap(), (150, 100));

        // JPEG rotate is lossless, PNG rotate re-encodes.
        let jpg = format!("{d}/photo.jpg");
        noisy(320, 160).save(dir.join("aligned.png")).unwrap();
        let (code, out, _) = run_args(&["convert", &format!("{d}/aligned.png"), "-o", &jpg]);
        assert_eq!(code, 0, "{out}");
        let (code, out, _) = run_args(&["rotate", "cw", &jpg, "--in-place", "--json"]);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("\"lossless\":true") && out.contains("\"trimmed\":false"), "{out}");
        assert_eq!(image::image_dimensions(&jpg).unwrap(), (160, 320));
        // Sizes that aren't a multiple of the JPEG block size lose the partial edge.
        let odd = format!("{d}/odd.jpg");
        run_args(&["convert", &a, "-o", &odd]);
        let (_, out, _) = run_args(&["rotate", "cw", &odd, "--in-place", "--json"]);
        assert!(out.contains("\"trimmed\":true"), "{out}");
        let (code, _, _) = run_args(&["flip", "h", &format!("{d}/img1.png"), "-o", &format!("{d}/flipped")]);
        assert_eq!(code, 0);
        assert!(dir.join("flipped/img1.png").exists());

        // Target size.
        noisy(600, 400).save(dir.join("big.png")).unwrap();
        let big = format!("{d}/big.png");
        let (code, out, err) = run_args(&["compress", &big, "-f", "jpg", "--target-size", "60KB", "--json"]);
        assert_eq!(code, 0, "{out}{err}");
        assert!(std::fs::metadata(dir.join("big.jpg")).unwrap().len() <= 60_000);
        assert!(out.contains("\"quality\":"), "{out}");
        let (code, out, _) = run_args(&["compress", &big, "-f", "jpg", "--target-size", "1KB", "-o", &format!("{d}/tiny.jpg")]);
        assert_eq!(code, 1);
        assert!(out.contains("even at quality 1"), "{out}");
        assert_eq!(run_args(&["compress", &big, "--target-size", "1KB"]).0, 1); // PNG output

        // Two inputs that would write the same file.
        noisy(10, 10).save(dir.join("img0.bmp")).unwrap();
        let (code, out, _) = run_args(&["convert", &a, &format!("{d}/img0.bmp"), "-f", "qoi", "-o", &format!("{d}/q")]);
        assert_eq!(code, 1);
        assert!(out.contains("would also be written"), "{out}");

        // Strip and dry run.
        let (code, out, _) = run_args(&["strip", &jpg, "-o", &format!("{d}/clean"), "--dry-run"]);
        assert_eq!(code, 0, "{out}");
        assert!(!dir.join("clean").exists());
        let (code, out, _) = run_args(&["strip", &jpg, "-o", &format!("{d}/clean")]);
        assert_eq!(code, 0, "{out}");
        assert!(dir.join("clean/photo.jpg").exists());

        // Folder input with -r keeps subfolders under -o.
        std::fs::create_dir_all(dir.join("tree/sub")).unwrap();
        noisy(20, 20).save(dir.join("tree/sub/deep.png")).unwrap();
        let (code, out, _) = run_args(&["convert", "-r", &format!("{d}/tree"), "-f", "jpg", "-o", &format!("{d}/tree-out")]);
        assert_eq!(code, 0, "{out}");
        assert!(dir.join("tree-out/sub/deep.jpg").exists());
    }
}
