//! Preferences kept between runs, as `key=value` lines in a small text file.

use std::path::PathBuf;

#[derive(Clone, PartialEq)]
pub struct Settings {
    /// The mouse wheel zooms; otherwise it goes to the next/previous image.
    pub wheel_zoom: bool,
    /// Resize the window to each image instead of keeping its size.
    pub window_fit: bool,
    pub fit_upscale: bool,
    pub show_strip: bool,
    pub slide_secs: f32,
    pub slide_loop: bool,
    pub slide_fullscreen: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            wheel_zoom: false,
            window_fit: false,
            fit_upscale: false,
            show_strip: false,
            slide_secs: 4.0,
            slide_loop: true,
            slide_fullscreen: true,
        }
    }
}

/// Where settings and the crash log live.
pub fn dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));
    #[cfg(not(any(windows, target_os = "macos")))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    Some(base?.join("superfastimageview"))
}

fn path() -> Option<PathBuf> {
    Some(dir()?.join("settings.txt"))
}

impl Settings {
    pub fn load() -> Self {
        let mut s = Self::default();
        let Some(text) = path().and_then(|p| std::fs::read_to_string(p).ok()) else { return s };
        s.parse(&text);
        s
    }

    fn parse(&mut self, text: &str) {
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let b = v == "true";
            match k {
                "wheel_zoom" => self.wheel_zoom = b,
                "window_fit" => self.window_fit = b,
                "fit_upscale" => self.fit_upscale = b,
                "show_strip" => self.show_strip = b,
                "slide_secs" => {
                    if let Ok(x) = v.parse::<f32>() {
                        self.slide_secs = x.clamp(0.5, 60.0);
                    }
                }
                "slide_loop" => self.slide_loop = b,
                "slide_fullscreen" => self.slide_fullscreen = b,
                _ => {}
            }
        }
    }

    fn to_text(&self) -> String {
        format!(
            "wheel_zoom={}\nwindow_fit={}\nfit_upscale={}\nshow_strip={}\nslide_secs={}\nslide_loop={}\nslide_fullscreen={}\n",
            self.wheel_zoom, self.window_fit, self.fit_upscale, self.show_strip, self.slide_secs, self.slide_loop, self.slide_fullscreen
        )
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, self.to_text());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let s = Settings { wheel_zoom: true, window_fit: true, slide_secs: 2.5, slide_loop: false, ..Default::default() };
        let mut t = Settings::default();
        t.parse(&s.to_text());
        assert!(t == s);
        t.parse("garbage\nslide_secs=999\nunknown=1");
        assert_eq!(t.slide_secs, 60.0);
    }
}
