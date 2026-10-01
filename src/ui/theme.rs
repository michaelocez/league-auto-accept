//! Visual language: one token system for colour, spacing, radii and typography.
//!
//! The palette is defined twice — a resolved **dark** set and a designed **light** set (light is
//! authored, never an inversion of dark). The active set is selected by the [`Appearance`] global,
//! which the UI reads via `cx.global::<Appearance>()` and which is written by [`set_appearance`].
//!
//! The palette direction (neutral layered surfaces, a monochrome accent, colour reserved for state)
//! is original to this project; no third-party visual identity is copied.

use std::sync::atomic::{AtomicU8, Ordering};

use gpui::{px, App, FontWeight, Global, Pixels, Rgba};

/// Light or dark appearance. Selected by the [`Appearance`] global.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Appearance {
    #[default]
    Dark,
    Light,
}

impl Global for Appearance {}

impl Appearance {
    /// Resolves this appearance to its concrete colour token set.
    pub fn theme(self) -> Theme {
        match self {
            Appearance::Dark => Theme::dark(),
            Appearance::Light => Theme::light(),
        }
    }
}

/// The fully resolved colour tokens for one appearance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// The window/page background behind everything.
    pub window_bg: Rgba,
    /// The navigation sidebar surface (slightly offset from the page background).
    pub sidebar_bg: Rgba,
    /// The main content background.
    pub bg: Rgba,
    /// A background lifted one step (e.g. a section that should read as raised).
    pub bg_elevated: Rgba,
    /// A base surface for grouped content.
    pub surface: Rgba,
    /// Surface under the pointer.
    pub surface_hover: Rgba,
    /// Surface while pressed or selected.
    pub surface_active: Rgba,
    /// Hairline separator.
    pub border: Rgba,
    /// Stronger border for emphasis/focus.
    pub border_strong: Rgba,
    /// Primary foreground text.
    pub text: Rgba,
    /// Secondary/muted text.
    pub text_muted: Rgba,
    /// Faint text (timestamps, hints).
    pub text_faint: Rgba,
    /// Brand/action accent.
    pub accent: Rgba,
    /// Accent under the pointer.
    pub accent_hover: Rgba,
    /// Foreground used on top of the accent.
    pub accent_fg: Rgba,
    /// A subdued accent-tinted surface (e.g. selected nav item background).
    pub accent_subtle: Rgba,
    /// Success state.
    pub success: Rgba,
    /// Warning state.
    pub warning: Rgba,
    /// Danger/error state.
    pub danger: Rgba,
}

impl Theme {
    /// Resolves the theme for the current [`Appearance`] global, falling back to dark if unset.
    pub fn of(cx: &App) -> Theme {
        cx.try_global::<Appearance>()
            .copied()
            .unwrap_or_default()
            .theme()
    }

    pub const fn dark() -> Self {
        Self {
            window_bg: rgb(0x0b0b0c),
            sidebar_bg: rgb(0x0d0d0f),
            bg: rgb(0x111113),
            bg_elevated: rgb(0x17171a),
            surface: rgb(0x161619),
            surface_hover: rgb(0x1e1e22),
            surface_active: rgb(0x26262b),
            border: rgb(0x242428),
            border_strong: rgb(0x36363c),
            text: rgb(0xf4f4f5),
            text_muted: rgb(0xa0a0a8),
            text_faint: rgb(0x6e6e76),
            // Neutral, near-white accent: a monochrome action colour rather than a brand hue.
            accent: rgb(0xe8e8ea),
            accent_hover: rgb(0xffffff),
            accent_fg: rgb(0x131316),
            accent_subtle: rgb(0x212127),
            success: rgb(0x63c08d),
            warning: rgb(0xd2ab60),
            danger: rgb(0xdd8080),
        }
    }

    pub const fn light() -> Self {
        Self {
            window_bg: rgb(0xf1f1f2),
            sidebar_bg: rgb(0xeaeaec),
            bg: rgb(0xf7f7f8),
            bg_elevated: rgb(0xffffff),
            surface: rgb(0xffffff),
            surface_hover: rgb(0xf0f0f2),
            surface_active: rgb(0xe6e6ea),
            border: rgb(0xe2e2e5),
            border_strong: rgb(0xcfcfd4),
            text: rgb(0x1b1b1e),
            text_muted: rgb(0x5a5a63),
            text_faint: rgb(0x8a8a93),
            // Neutral, near-black accent in light mode.
            accent: rgb(0x1c1c1f),
            accent_hover: rgb(0x000000),
            accent_fg: rgb(0xffffff),
            accent_subtle: rgb(0xe6e6ea),
            success: rgb(0x2f9e63),
            warning: rgb(0xa8761f),
            danger: rgb(0xc94a4a),
        }
    }
}

const fn rgb(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

/// Writes the active appearance to both the GPUI global and the process-local mirror.
pub fn set_appearance(cx: &mut App, appearance: Appearance) {
    cx.set_global(appearance);
    ACTIVE.store(appearance as u8, Ordering::Relaxed);
}

/// The currently active appearance.
pub fn current() -> Appearance {
    if ACTIVE.load(Ordering::Relaxed) == Appearance::Light as u8 {
        Appearance::Light
    } else {
        Appearance::Dark
    }
}

// The process-local mirror lets `current()` work outside a GPUI context. `set_appearance` is the
// only writer, so it cannot drift from the `Appearance` global.
static ACTIVE: AtomicU8 = AtomicU8::new(Appearance::Dark as u8);

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
pub fn space_5() -> Pixels {
    px(20.0)
}
pub fn space_6() -> Pixels {
    px(24.0)
}
pub fn space_8() -> Pixels {
    px(32.0)
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

// --- Type scale ------------------------------------------------------------------------------

pub fn text_display() -> Pixels {
    px(28.0)
}
pub fn text_title() -> Pixels {
    px(20.0)
}
pub fn text_heading() -> Pixels {
    px(16.0)
}
pub fn text_body() -> Pixels {
    px(14.0)
}
pub fn text_small() -> Pixels {
    px(13.0)
}
pub fn text_caption() -> Pixels {
    px(12.0)
}

pub fn weight_regular() -> FontWeight {
    FontWeight::NORMAL
}
pub fn weight_medium() -> FontWeight {
    FontWeight::MEDIUM
}
pub fn weight_semibold() -> FontWeight {
    FontWeight::SEMIBOLD
}
