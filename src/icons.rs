//! Toolbar and tool icons: a subset of Phosphor Icons (MIT, see
//! assets/icons-LICENSE.txt) holding only the glyphs below.
//!
//! The glyphs sit in the Private Use Area, which egui's own emoji font also
//! uses, so they get a font family of their own instead of joining the
//! proportional fallback chain.

use egui::{FontData, FontFamily, FontInsert, FontPriority, InsertFontFamily, RichText};

const FONT: &[u8] = include_bytes!("../assets/icons.ttf");
const FAMILY: &str = "icons";

pub const OPEN: char = '\u{e256}';
pub const SAVE: char = '\u{e248}';
pub const PREV: char = '\u{e138}';
pub const NEXT: char = '\u{e13a}';
pub const ZOOM_IN: char = '\u{e310}';
pub const ZOOM_OUT: char = '\u{e30e}';
pub const FIT: char = '\u{e1ce}';
pub const ROTATE_L: char = '\u{e038}';
pub const ROTATE_R: char = '\u{e036}';
pub const FLIP_H: char = '\u{ed6a}';
pub const FLIP_V: char = '\u{ed6c}';
pub const SELECT: char = '\u{e69a}';
pub const CROP: char = '\u{e1d4}';
pub const RESIZE: char = '\u{ed6e}';
pub const UNDO: char = '\u{e08a}';
pub const REDO: char = '\u{e08c}';
pub const EDIT: char = '\u{e6f0}';
pub const BATCH: char = '\u{e466}';
pub const THUMBS: char = '\u{e792}';
pub const GRID: char = '\u{e464}';
pub const SLIDESHOW: char = '\u{e3d0}';
pub const HAND: char = '\u{e298}';
pub const PICKER: char = '\u{e568}';
pub const BRUSH: char = '\u{e6f0}';
pub const ERASER: char = '\u{e21e}';
pub const LINE: char = '\u{e6d2}';
pub const RECT: char = '\u{e3f0}';
pub const ELLIPSE: char = '\u{e18a}';
pub const FILL: char = '\u{e392}';
pub const TEXT: char = '\u{e48a}';
pub const SWAP: char = '\u{e0a0}';

/// Registers the icon font. Cheap: egui parses it when the font atlas is
/// first built, which happens after the first image is already on screen.
pub fn install(ctx: &egui::Context) {
    ctx.add_font(FontInsert::new(
        FAMILY,
        FontData::from_static(FONT),
        vec![InsertFontFamily { family: FontFamily::Name(FAMILY.into()), priority: FontPriority::Highest }],
    ));
}

/// An icon glyph at toolbar size.
pub fn icon(c: char) -> RichText {
    RichText::new(c.to_string()).family(FontFamily::Name(FAMILY.into())).size(17.0)
}
