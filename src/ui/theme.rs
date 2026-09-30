//! Visual language: one token system for colour, spacing, radii and typography.
//!
//! The palette is defined twice — a resolved **dark** set and a designed **light** set (light is
//! authored, never an inversion of dark). The active set is selected by the [`Appearance`] global,
//! which the UI reads via `cx.global::<Appearance>()` and which is written by [`set_appearance`].
//!
//! The palette direction (neutral layered surfaces, a restrained indigo accent, colour reserved for
//! state) is original to this project; no third-party visual identity is copied.

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

    /// Toggles between light and dark.
    pub fn toggled(self) -> Self {
        match self {
            Appearance::Dark => Appearance::Light,
            Appearance::Light => Appearance::Dark,
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
    /// Floating/overlay surface (menus, popovers).
    pub overlay: Rgba,
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
    /// Focus ring colour.
    pub focus_ring: Rgba,
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
            window_bg: rgb(0x0c0d10),
            sidebar_bg: rgb(0x0a0b0d),
            bg: rgb(0x101115),
            bg_elevated: rgb(0x16181d),
            surface: rgb(0x15171c),
            surface_hover: rgb(0x1c1f26),
            surface_active: rgb(0x23272f),
            overlay: rgb(0x1b1e24),
            border: rgb(0x24272e),
            border_strong: rgb(0x353a44),
            text: rgb(0xf3f4f6),
            text_muted: rgb(0x9aa1ad),
            text_faint: rgb(0x686f7b),
            accent: rgb(0x6e79e0),
            accent_hover: rgb(0x7f89e8),
            accent_fg: rgb(0xffffff),
            accent_subtle: rgb(0x262a45),
            success: rgb(0x45c184),
            warning: rgb(0xd9a256),
            danger: rgb(0xe06c7a),
            focus_ring: rgb(0x6e79e0),
        }
    }

    pub const fn light() -> Self {
        Self {
            window_bg: rgb(0xf3f4f7),
            sidebar_bg: rgb(0xeceef2),
            bg: rgb(0xf7f8fa),
            bg_elevated: rgb(0xffffff),
            surface: rgb(0xffffff),
            surface_hover: rgb(0xf1f2f5),
            surface_active: rgb(0xe7e9ee),
            overlay: rgb(0xffffff),
            border: rgb(0xe3e5ea),
            border_strong: rgb(0xccd0d8),
            text: rgb(0x1a1d23),
            text_muted: rgb(0x5b6270),
            text_faint: rgb(0x8a909c),
            accent: rgb(0x5560d8),
            accent_hover: rgb(0x4650c6),
            accent_fg: rgb(0xffffff),
            accent_subtle: rgb(0xe6e8fb),
            success: rgb(0x1f9d5b),
            warning: rgb(0xb3761f),
            danger: rgb(0xcf4a5a),
            focus_ring: rgb(0x5560d8),
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

/// The active resolved token set.
pub fn theme() -> Theme {
    current().theme()
}

// The process-local mirror lets transitional free-function accessors below work before every view
// reads the `Appearance` global directly. `set_appearance` is the only writer, so it cannot drift.
static ACTIVE: AtomicU8 = AtomicU8::new(Appearance::Dark as u8);

// --- Transitional colour accessors (mapped onto the token roles above) ------------------------
// These keep existing screens compiling while the component/theme migration lands; new code should
// read `cx.global::<Appearance>().theme()` (or a passed-in `Theme`).

pub fn background() -> Rgba {
    theme().bg
}

pub fn surface() -> Rgba {
    theme().surface
}

pub fn surface_raised() -> Rgba {
    theme().surface_hover
}

pub fn border() -> Rgba {
    theme().border
}

pub fn border_strong() -> Rgba {
    theme().border_strong
}

pub fn border_focus() -> Rgba {
    theme().accent
}

pub fn text_primary() -> Rgba {
    theme().text
}

pub fn text_secondary() -> Rgba {
    theme().text_muted
}

pub fn text_muted() -> Rgba {
    theme().text_faint
}

pub fn accent() -> Rgba {
    theme().accent
}

pub fn success() -> Rgba {
    theme().success
}

pub fn warning() -> Rgba {
    theme().warning
}

pub fn danger() -> Rgba {
    theme().danger
}

// --- Spacing scale (4px base) ----------------------------------------------------------------

pub fn space_0_5() -> Pixels {
    px(2.0)
}
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
pub fn space_10() -> Pixels {
    px(40.0)
}

// --- Corner radii ----------------------------------------------------------------------------

pub fn radius_xs() -> Pixels {
    px(4.0)
}
pub fn radius_sm() -> Pixels {
    px(6.0)
}
pub fn radius_md() -> Pixels {
    px(8.0)
}
pub fn radius_lg() -> Pixels {
    px(12.0)
}
pub fn radius_xl() -> Pixels {
    px(16.0)
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
