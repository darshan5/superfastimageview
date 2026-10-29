//! Generates a folder of synthetic test images for benchmarking load times:
//! `cargo run --release --example make_test_images -- <out_dir>`

use image::{Rgba, RgbaImage};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "test-images".into());
    let out = std::path::Path::new(&out);
    std::fs::create_dir_all(out).unwrap();
    let sizes = [(6000, 4000, "24mp"), (4000, 3000, "12mp"), (1920, 1080, "hd"), (640, 480, "small")];
    for (w, h, name) in sizes {
        let img = RgbaImage::from_fn(w, h, |x, y| {
            let fx = x as f32 / w as f32;
            let fy = y as f32 / h as f32;
            let ring = (((fx - 0.5).powi(2) + (fy - 0.5).powi(2)).sqrt() * 60.0).sin() * 0.5 + 0.5;
            Rgba([(fx * 255.0) as u8, (ring * 255.0) as u8, (fy * 255.0) as u8, 255])
        });
        image::DynamicImage::ImageRgba8(img.clone()).to_rgb8().save(out.join(format!("{name}.jpg"))).unwrap();
        if w <= 4000 {
            img.save(out.join(format!("{name}.png"))).unwrap();
        }
    }
    let alpha = RgbaImage::from_fn(800, 600, |x, y| {
        let d = ((x as f32 - 400.0).powi(2) + (y as f32 - 300.0).powi(2)).sqrt();
        Rgba([255, 120, 0, (255.0 - d.min(255.0)) as u8])
    });
    alpha.save(out.join("alpha.png")).unwrap();
    println!("wrote test images to {}", out.display());
}
