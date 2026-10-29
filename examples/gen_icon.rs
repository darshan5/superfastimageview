//! Writes `assets/icon.png` (1024px) and `assets/icon.ico` (multi-size) from
//! the procedural icon: `cargo run --example gen_icon`

#[path = "../src/icon.rs"]
mod icon;

use image::RgbaImage;
use image::codecs::ico::{IcoEncoder, IcoFrame};

fn main() {
    std::fs::create_dir_all("assets").unwrap();
    let big = RgbaImage::from_raw(1024, 1024, icon::icon_rgba(1024)).unwrap();
    big.save("assets/icon.png").unwrap();
    let frames: Vec<IcoFrame> = [16, 24, 32, 48, 64, 128, 256]
        .into_iter()
        .map(|s| IcoFrame::as_png(&icon::icon_rgba(s), s, s, image::ExtendedColorType::Rgba8).unwrap())
        .collect();
    let file = std::fs::File::create("assets/icon.ico").unwrap();
    IcoEncoder::new(file).encode_images(&frames).unwrap();
    println!("wrote assets/icon.png and assets/icon.ico");
}
