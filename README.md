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
| 6000×4000 JPEG       |           ~190–220 ms | 145 ms |

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
- JPEG, PNG, GIF (first frame), BMP, ICO, TIFF, WebP, TGA, QOI, PNM/PAM, HDR,
  EXR, DDS, Farbfeld. Format is detected from content; EXIF orientation is applied.
- Browse the folder in natural sort order (`photo2` before `photo10`).
- Wheel/pinch zoom around the cursor, fit-to-window, 1:1, pan, full screen.
- Huge images are split into GPU-sized tiles. Mipmaps keep zoomed-out views smooth.
- Transparency checkerboard, pixel position and colour readout, image info panel.
- Copy/paste images through the clipboard, drag and drop, move to trash.

**Editing** (undo/redo, about 1 GB of history)
- Rotate 90°/180°, flip, invert, grayscale, crop to selection, resize (Nearest,
  Bilinear, Bicubic, Lanczos3; SIMD via `fast_image_resize`), new blank image.
- Paint tools: brush, eraser (to secondary colour or transparent), line,
  rectangle, ellipse (outline / filled / outline + fill), flood fill with
  tolerance, colour picker. Left button = primary colour, right = secondary,
  Shift constrains lines to 45° and boxes to squares. Only the changed pixels
  are re-uploaded to the GPU.

**Conversion**
- Save as JPEG (quality), PNG (compression level), WebP (lossless), BMP, GIF,
  TIFF, TGA, QOI, ICO, PPM. Saves go to a temp file first and are then renamed
  over the target, so a failed save never corrupts the original.
- **Batch conversion** (`B`): files, folders (optionally recursive) or drag and
  drop. Options: output format, quality, resize (percent / fit within /
  exact), rotate, flip, grayscale, name pattern (`{name}`, `{n}`), and
  skip/overwrite existing files. Runs in parallel on all cores, with progress,
  cancel and an error log.

## Keyboard shortcuts

On macOS use Cmd instead of Ctrl. Press `F1` in the app for the full list.

| Key | Action |
|-----|--------|
| → / Space / PgDn, ← / Backspace / PgUp | Next / previous image |
| Home / End | First / last image |
| Wheel, `+` / `-` | Zoom |
| `0` or `F` / `1` | Fit / actual size |
| Enter / F11 / double-click | Full screen |
| `R` / `L`, `H` / `V` | Rotate right/left, flip horizontal/vertical |
| Ctrl+Y | Crop to selection |
| Ctrl+R | Resize |
| Ctrl+Shift+I / Ctrl+G | Invert / grayscale |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo |
| Ctrl+C / Ctrl+V | Copy / paste image |
| Ctrl+O / Ctrl+S / Ctrl+Shift+S | Open / save / save as |
| `E` | Paint tools panel |
| `B` | Batch conversion |
| `I` | Image info |
| Del | Move to trash |
| Esc | Close dialog → clear selection → leave full screen → quit |

## Building

You need Rust (stable, 2024 edition).

```sh
cargo run --release -- path/to/image.jpg
cargo test
```

**Windows** (needs MSVC Build Tools):

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

## Architecture

| File | Responsibility |
|------|----------------|
| `src/main.rs` | Event loop (winit + glutin + egui-winit + egui_glow, no eframe), startup ordering, GDI-preview → GPU hand-off |
| `src/gl.rs` | OpenGL display/context creation, including the off-thread WGL worker |
| `src/preview.rs` | Windows GDI preview painter |
| `src/loader.rs` | Background decoding, LRU cache, folder listing |
| `src/canvas.rs` | Tiled GPU textures, partial updates |
| `src/app.rs` | UI: viewer, menus, tools, dialogs, shortcuts |
| `src/edit.rs` | Whole-image operations |
| `src/paint.rs` | Raster drawing primitives and flood fill |
| `src/convert.rs` | Encoders, save, batch conversion |
| `src/macos.rs` | Receives files opened from Finder (`application:openURLs:`) |

## Status and known gaps

- **The macOS build has not been run on a Mac yet.** It type-checks for
  `aarch64-apple-darwin`, and CI builds it on `macos-14`, but the Finder "open
  file" hook in `src/macos.rs` (which adds a method to winit's app delegate)
  needs testing on real hardware.
- Animated GIF/WebP show the first frame only.
- AVIF, HEIC and camera RAW are not supported yet. AVIF/HEIC need native
  codecs; platform decoders (WIC / ImageIO) would be the natural route.
- WebP output is lossless only (limitation of the pure-Rust encoder).
- Very large JPEGs are decode-bound (~145 ms for 24 MP). A further speed-up
  would be DCT-domain downscaled decoding for the first preview (libjpeg-turbo,
  WIC or ImageIO).
- Rotations re-encode JPEGs (no lossless JPEG transform yet).

## License

MIT
