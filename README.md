# SuperFastImageView

A native Rust image viewer for **Windows** and **macOS (Apple Silicon)** built
for one thing above all: **getting the first image on screen as fast as
possible**. It also does basic editing (rotate, flip, invert, grayscale, crop,
resize, brush/line/rectangle/ellipse/eraser/fill) and single or batch format
conversion, in the spirit of IrfanView.

## Speed

Measured on Windows 11, GTX 1650 SUPER, release build, from process start
until pixels are on screen (`SFIV_TIMING=1` prints these):

| Image                | First image on screen | Decode |
|----------------------|----------------------:|-------:|
| 640×480 PNG          |               ~110 ms |   3 ms |
| 1920×1080 JPEG       |               ~110 ms |  13 ms |
| 4000×3000 JPEG       |               ~120 ms |  70 ms |
| 6000×4000 JPEG       |           ~170–220 ms | 145 ms |
| Sony ARW (embedded preview, full develop follows) | ~105 ms | 73 ms |
| 24 MP Canon CR3 / Nikon NEF (embedded preview) | not measured | ~130 ms |
| 12 MP HEIC (Windows codec) | ~730 ms | ~630 ms |

The equivalent stock `eframe` app took ~370–410 ms on the same machine,
because creating an OpenGL context costs ~300 ms on this driver. How we avoid
waiting for it:

1. **Decode starts first.** `main` kicks off decoding the file on a worker
   thread before the window, event loop or GPU exist.
2. **GPU setup runs off the main thread.** On Windows, the expensive WGL work
   (driver load, pixel format, context) runs on a worker against a hidden
   window. It starts right after the main window is created, because driver
   loading holds the DLL loader lock and would otherwise stall window creation.
3. **Instant GDI preview.** Until the GPU is ready, the decoded image is
   painted into the window with plain GDI (fast 2×2-tap downscale). The real UI
   then takes over in the same window.
4. **Fast everything else.** Uses zune-jpeg (via `image`), a single read
   syscall per file, multi-threaded texture preparation, mimalloc on Windows,
   fat LTO, and `opt-level=3` for dependencies even in debug builds.
5. **Instant navigation.** Neighbouring images in the folder are pre-decoded in
   the background and kept in a small LRU cache (1.5 GB budget).

On macOS, OpenGL setup is much cheaper and runs inline. Decoding still starts
before the window.

## Features

**Viewing**
- JPEG, PNG/APNG, GIF, WebP, BMP, ICO, TIFF, TGA, QOI, PNM/PAM, HDR, EXR, DDS,
  Farbfeld. Format is detected from content; EXIF orientation is applied.
- **Animated GIF, WebP and APNG** play back (`P` pauses). The first frame shows
  immediately and the rest decode in the background.
- **HEIC/HEIF and AVIF** through the operating system's codecs: WIC on Windows,
  ImageIO on macOS. On Windows, HEIC needs Microsoft's *HEIF Image Extensions* +
  *HEVC Video Extensions* (AVIF needs *AV1 Video Extension*); the app tells you
  if they're missing.
- **Camera RAW** (CR2, CR3, NEF, ARW, DNG, RAF, ORF, RW2, PEF, SRW and ~20 more)
  via the pure-Rust `rawler`. The camera's embedded JPEG preview is shown
  instantly. If that preview is much smaller than the sensor, the full RAW is
  developed in the background (~0.5 s for 24 MP) and swapped in. Batch
  conversion always uses full resolution.
- Browse the folder in natural sort order (`photo2` before `photo10`).
- The mouse wheel goes to the next / previous image in the folder (wrapping
  around), or zooms around the cursor (View menu). Ctrl+wheel and pinch always
  zoom. Fit-to-window, 1:1, full screen.
- The image stays put: it is centered when it fits, and a zoomed-in image can
  only be dragged as far as its edges.
- Window size: keep it fixed, or fit the window to each image (100%, or as
  large as fits on the screen).
- View preferences are saved between runs (`%APPDATA%\superfastimageview` on
  Windows, `~/Library/Application Support/superfastimageview` on macOS).
- Huge images are split into GPU-sized tiles. Mipmaps keep zoomed-out views smooth.
- Transparency checkerboard, pixel position and colour readout, image info panel.
- Copy/paste images through the clipboard, drag and drop, move to trash.
- **Thumbnail strip** (`T`) and **grid view** (`G`; arrows move, Enter opens,
  `+`/`-` resize). Thumbnails are decoded on a few background threads, only
  for what is on screen. JPEGs use libjpeg-turbo's 1/8-scale DCT decode (a few
  ms even for 24 MP), RAW files their embedded previews. Nothing starts until
  you open one of these views, so startup is unaffected.
- **Slideshow** (`F5`): adjustable delay, loop, full screen. Space pauses,
  arrows step, Esc stops. The timer starts once each image is on screen, so
  slow decodes don't shorten the display time.

**Editing** (undo/redo, about 1 GB of history, with a clickable **History**
list in the edit panel)
- **Lossless JPEG rotate/flip** (Image → Lossless JPEG transform,
  Ctrl+Shift+R / Ctrl+Shift+L). It rearranges DCT blocks with libjpeg-turbo, like
  `jpegtran`, so there's no recompression. It composes with the file's EXIF
  orientation and resets the tag. When the size isn't a multiple of the JPEG
  block size, the partial edge block (at most 15 px) is trimmed, as
  `jpegtran -trim` does.
- Rotate 90°/180°, flip, invert, grayscale, crop to selection, resize (Nearest,
  Bilinear, Bicubic, Lanczos3; SIMD via `fast_image_resize`), new blank image.
- Paint tools: brush, eraser (to secondary colour or transparent), line,
  rectangle, ellipse (outline / filled / outline + fill), flood fill with
  tolerance, colour picker. Left button = primary colour, right = secondary,
  Shift constrains lines to 45° and boxes to squares. Only the changed pixels
  are re-uploaded to the GPU.
- **Text tool**: click to place, click again to move, multi-line, any size,
  anti-aliased. Fonts: two built in (Ubuntu Light and Hack, from egui) plus
  common system fonts found on the machine (Arial, Segoe UI, Times, Georgia,
  Consolas, Impact, Helvetica, Menlo, …). The preview is rendered exactly as it
  will be drawn. Ctrl+Enter applies.

**Conversion**
- Save as JPEG (quality), PNG (compression level), WebP (lossy with quality, or
  lossless; libwebp), BMP, GIF,
  TIFF, TGA, QOI, ICO, PPM. Saves go to a temp file first and are then renamed
  over the target, so a failed save never corrupts the original.
- **Batch conversion** (`B`): files, folders (optionally recursive) or drag and
  drop. Options: output format, quality, resize (percent / fit within /
  exact), rotate, flip, grayscale, and skip/overwrite existing files. Runs in
  parallel on all cores, with progress, cancel and an error log.
- **File name patterns** for batch output: `{name}`, `{n}` (counter, zero
  padded, adjustable start), `{parent}` (folder), `{date}` (file date),
  `{ext}`, `{w}`/`{h}` (output size). Presets, a live example, and a warning
  when several files would get the same name.
- **Metadata removal**, lossless: Ctrl+E on the current file (overwrites it),
  or as a batch task (in place, or into another folder). Removes EXIF
  (including GPS), XMP, IPTC, comments and PNG text chunks from JPEG, PNG and
  WebP without touching the compressed image data. ICC colour profiles are
  kept, and so is the rotation: a rotated photo gets a minimal EXIF block
  holding only the orientation tag. The info panel (`I`) lists the metadata a
  file contains.
- Saving or converting never writes metadata, so converted files carry no
  EXIF/GPS either.

## Keyboard shortcuts

On macOS use Cmd instead of Ctrl. Press `F1` in the app for the full list.

| Key | Action |
|-----|--------|
| → / Space / PgDn, ← / Backspace / PgUp | Next / previous image |
| Home / End | First / last image |
| Wheel | Next / previous image (or zoom, see View menu) |
| Ctrl+wheel, `+` / `-` | Zoom |
| `0` or `F` / `1` | Fit / actual size |
| Enter / F11 / double-click | Full screen |
| `R` / `L`, `H` / `V` | Rotate right/left, flip horizontal/vertical |
| Ctrl+Shift+R / Ctrl+Shift+L | Lossless JPEG rotate right / left (rewrites the file) |
| `P` | Pause / play animation |
| Ctrl+Y | Crop to selection |
| Ctrl+R | Resize |
| Ctrl+Shift+I / Ctrl+G | Invert / grayscale |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo |
| Ctrl+C / Ctrl+V | Copy / paste image |
| Ctrl+O / Ctrl+S / Ctrl+Shift+S | Open / save / save as |
| `E` | Paint tools, text, history |
| `T` / `G` | Thumbnail strip / grid view |
| F5 | Slideshow (Space pauses, Esc stops) |
| Ctrl+E | Remove metadata, overwriting the file |
| Ctrl+Enter | Apply text (Text tool) |
| `B` | Batch conversion |
| `I` | Image info |
| Del | Move to trash |
| Esc | Close dialog → clear selection → leave full screen → quit |

## Building

You need Rust (stable, 2024 edition) and **CMake** (for libjpeg-turbo).
NASM is optional: without it libjpeg-turbo builds without SIMD, which only the
lossless transforms use.

```sh
cargo run --release -- path/to/image.jpg
cargo test
```

**Windows** (needs MSVC Build Tools; their CMake works if it is on `PATH`, e.g.
`...\BuildTools\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin`).
The C runtime is linked statically (`.cargo/config.toml`), so the exe needs no
VC++ redistributable:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1   # -> dist\SuperFastImageView.exe
```

The icon and version info are embedded by `build.rs`. To make it the default
viewer, use *Open with → Choose another app* on an image.

**macOS, Apple Silicon** (run on a Mac):

```sh
scripts/bundle-macos.sh          # -> dist/SuperFastImageView.app
scripts/bundle-macos.sh --dmg    # also builds a .dmg
```

The bundle declares the supported image types, so it appears under
*Open With*. It is ad-hoc signed. For distribution, sign it with a Developer
ID and notarize it.

CI (`.github/workflows/build.yml`) runs tests and builds both platforms on every push.

Other tools:
- `cargo run --release --example make_test_images -- <dir>` writes synthetic
  6000×4000, 4000×3000, 1080p and alpha test images for benchmarking.
- `cargo run --example gen_icon` regenerates `assets/icon.png` and `assets/icon.ico`.
- `SFIV_TIMING=1` prints startup timings to stderr.
- `SFIV_SHOT=a.png@800,b.png@2000` saves the rendered frame at those times
  (ms after start), for automated visual checks.
- `SFIV_SAMPLES=<dir> cargo test --release <decode_samples|thumb_samples|strip_samples> -- --nocapture`
  decodes every file in a folder of real-world samples and prints timings.

## Architecture

| File | Responsibility |
|------|----------------|
| `src/main.rs` | Event loop (winit + glutin + egui-winit + egui_glow, no eframe), startup ordering, GDI-preview → GPU hand-off |
| `src/gl.rs` | OpenGL display/context creation, including the off-thread WGL worker |
| `src/preview.rs` | Windows GDI preview painter |
| `src/loader.rs` | Background decoding (format routing, animations), LRU cache, folder listing |
| `src/raw.rs` | Camera RAW: embedded preview, full development |
| `src/platform.rs` | HEIC/HEIF/AVIF via WIC (Windows) and ImageIO (macOS) |
| `src/jpegtran.rs` | Lossless JPEG transforms and EXIF orientation handling |
| `src/metadata.rs` | Lossless metadata removal for JPEG, PNG, WebP |
| `src/thumbs.rs` | Background thumbnail decoding and texture cache |
| `src/text.rs` | Font list and text rendering for the Text tool |
| `src/settings.rs` | Preferences saved between runs |
| `src/canvas.rs` | Tiled GPU textures, partial updates |
| `src/app.rs` | UI: viewer, menus, tools, dialogs, shortcuts |
| `src/edit.rs` | Whole-image operations |
| `src/paint.rs` | Raster drawing primitives and flood fill |
| `src/convert.rs` | Encoders, save, batch conversion |
| `src/macos.rs` | Receives files opened from Finder (`application:openURLs:`) |

## Status and known gaps

- **The macOS app has not been run on a Mac yet.** CI builds it and runs the
  tests on `macos-14`. Still untested on real hardware: the Finder "open file"
  hook in `src/macos.rs` and HEIC/AVIF decoding through ImageIO.
- HEIC decoding on Windows goes through the system HEVC codec and is slower
  than JPEG (~630 ms for 12 MP).
- Saving an animation writes the current frame only.
- RAW development uses rawler's default pipeline; there are no exposure or
  white-balance controls.
- Very large JPEGs are decode-bound (~145 ms for 24 MP). The scaled decode
  the thumbnails use could also give a faster first preview.
- Saving keeps no metadata at all (there's no "keep EXIF" option yet).
- Placed text can't be edited after it's applied (it becomes pixels; use
  undo).

## License

MIT
