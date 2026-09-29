//! Design tokens for the Phase 1 spike UI. The full theme system is Phase 4.
//!
//! Colours follow the *direction* of the Electron app's restrained dark palette without
//! copying any other project's visual identity.

use gpui::Rgba;

pub fn background() -> Rgba {
    gpui::rgb(0x010102)
}

pub fn surface() -> Rgba {
    gpui::rgb(0x0f1011)
}

pub fn surface_raised() -> Rgba {
    gpui::rgb(0x141516)
}

pub fn border() -> Rgba {
    gpui::rgb(0x23252a)
}

pub fn text_primary() -> Rgba {
    gpui::rgb(0xf7f8f8)
}

pub fn text_secondary() -> Rgba {
    gpui::rgb(0x8a8f98)
}

pub fn accent() -> Rgba {
    gpui::rgb(0x5e6ad2)
}

pub fn success() -> Rgba {
    gpui::rgb(0x27a644)
}

pub fn warning() -> Rgba {
    gpui::rgb(0x828fff)
}

pub fn danger() -> Rgba {
    gpui::rgb(0xe898a2)
}
