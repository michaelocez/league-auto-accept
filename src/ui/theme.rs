//! Visual language: design tokens for colour, spacing, corner radii and typography.
//!
//! These are the reusable foundations the whole UI composes from, so the application has one
//! coherent, restrained look. Palette direction is a dark, quiet, native surface system; no
//! third-party visual identity is copied.

use gpui::{px, Pixels, Rgba};

// --- Colour roles ---------------------------------------------------------------------------

pub fn background() -> Rgba {
    gpui::rgb(0x08090b)
}

/// Primary card/section surface (clearly lifted from the window background).
pub fn surface() -> Rgba {
    gpui::rgb(0x16181d)
}

/// Slightly raised surface for hover and nested cards.
pub fn surface_raised() -> Rgba {
    gpui::rgb(0x1f2229)
}

/// Hairline border.
pub fn border() -> Rgba {
    gpui::rgb(0x2c303a)
}

/// Stronger border for focus/emphasis.
pub fn border_strong() -> Rgba {
    gpui::rgb(0x3a3f4b)
}

pub fn border_focus() -> Rgba {
    accent()
}

pub fn text_primary() -> Rgba {
    gpui::rgb(0xf2f3f5)
}

pub fn text_secondary() -> Rgba {
    gpui::rgb(0x9aa0aa)
}

pub fn text_muted() -> Rgba {
    gpui::rgb(0x6b7280)
}

pub fn accent() -> Rgba {
    gpui::rgb(0x5e6ad2)
}

pub fn success() -> Rgba {
    gpui::rgb(0x35b26a)
}

pub fn warning() -> Rgba {
    gpui::rgb(0x8a93ff)
}

pub fn danger() -> Rgba {
    gpui::rgb(0xe06c7a)
}

// --- Spacing scale (4px base) ----------------------------------------------------------------

pub fn space_1() -> Pixels {
    px(4.0)
}
pub fn space_2() -> Pixels {
    px(8.0)
}
pub fn space_3() -> Pixels {
    px(12.0)
}
pub fn space_4() -> Pixels {
    px(16.0)
}
pub fn space_6() -> Pixels {
    px(24.0)
}

// --- Corner radii ----------------------------------------------------------------------------

pub fn radius_sm() -> Pixels {
    px(6.0)
}
pub fn radius_md() -> Pixels {
    px(8.0)
}
pub fn radius_lg() -> Pixels {
    px(12.0)
}
