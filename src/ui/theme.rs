//! Visual language: one token system for colour, spacing, radii and typography.
//!
//! The palette is defined twice — a resolved **dark** set and a designed **light** set (light is
//! authored, never an inversion of dark). The active set is selected by the [`Appearance`] global,
//! which the UI reads via `cx.global::<Appearance>()` and which is written by [`set_appearance`].
//!
//! The palette direction (neutral layered surfaces, a monochrome accent, colour reserved for state)
//! is original to this project; no third-party visual identity is copied.

use std::sync::atomic::{AtomicU8, Ordering};

use gpui::{px, App, FontWeight, Global, Pixels, Rgba, WindowBackgroundAppearance};

use crate::config::settings::{BackdropMode, ThemeMode};

/// Light or dark appearance. Selected by the [`Appearance`] global.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Appearance {
    #[default]
    Dark,
    Light,
}

impl Global for Appearance {}

impl Appearance {
    /// Resolves this appearance to its opaque token set.
    pub fn theme(self) -> Theme {
        self.theme_with(SurfaceTreatment::Opaque)
    }

    /// Resolves this appearance + surface treatment to a concrete token set.
    pub fn theme_with(self, treatment: SurfaceTreatment) -> Theme {
        match (self, treatment) {
            (Appearance::Dark, SurfaceTreatment::Opaque) => Theme::dark(),
            (Appearance::Dark, SurfaceTreatment::Mica) => Theme::dark_mica(),
            (Appearance::Dark, SurfaceTreatment::Acrylic) => Theme::dark_acrylic(),
            (Appearance::Light, SurfaceTreatment::Opaque) => Theme::light(),
            (Appearance::Light, SurfaceTreatment::Mica) => Theme::light_mica(),
            (Appearance::Light, SurfaceTreatment::Acrylic) => Theme::light_acrylic(),
        }
    }
}

/// How surfaces are treated: flat and opaque, or translucent over an OS backdrop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SurfaceTreatment {
    /// Opaque surfaces; the default and the safe fallback.
    #[default]
    Opaque,
    /// Translucent surfaces over the Windows 11 **Mica** backdrop. Mica samples the desktop
    /// wallpaper (desaturated) and is deliberately subtle — it does not blur what is behind the
    /// window.
    Mica,
    /// Translucent surfaces over the Windows 11 **Acrylic** backdrop, which blurs the content
    /// behind the window in real time — the stronger "glass" look.
    Acrylic,
}

impl Global for SurfaceTreatment {}

impl From<ThemeMode> for Appearance {
    fn from(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Appearance::Dark,
            ThemeMode::Light => Appearance::Light,
        }
    }
}

impl From<Appearance> for ThemeMode {
    fn from(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Dark => ThemeMode::Dark,
            Appearance::Light => ThemeMode::Light,
        }
    }
}

impl From<BackdropMode> for SurfaceTreatment {
    fn from(mode: BackdropMode) -> Self {
        match mode {
            BackdropMode::Opaque => SurfaceTreatment::Opaque,
            BackdropMode::Mica => SurfaceTreatment::Mica,
            BackdropMode::Acrylic => SurfaceTreatment::Acrylic,
        }
    }
}

impl From<SurfaceTreatment> for BackdropMode {
    fn from(treatment: SurfaceTreatment) -> Self {
        match treatment {
            SurfaceTreatment::Opaque => BackdropMode::Opaque,
            SurfaceTreatment::Mica => BackdropMode::Mica,
            SurfaceTreatment::Acrylic => BackdropMode::Acrylic,
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
    /// Resolves the theme for the current [`Appearance`] + [`SurfaceTreatment`] globals.
    pub fn of(cx: &App) -> Theme {
        let appearance = cx.try_global::<Appearance>().copied().unwrap_or_default();
        let treatment = cx
            .try_global::<SurfaceTreatment>()
            .copied()
            .unwrap_or_default();
        appearance.theme_with(treatment)
    }

    pub const fn dark() -> Self {
        Self {
            // Chrome (title bar + sidebar) sits on the window ground; the content pane is a hair
            // lighter. The two are separated by a hairline, not by a heavy filled slab.
            window_bg: rgb(0x0e0e10),
            sidebar_bg: rgb(0x0e0e10),
            bg: rgb(0x131315),
            bg_elevated: rgb(0x1a1a1d),
            surface: rgb(0x17171a),
            surface_hover: rgb(0x1f1f23),
            surface_active: rgb(0x2a2a30),
            border: rgb(0x26262b),
            border_strong: rgb(0x3a3a42),
            text: rgb(0xf5f5f7),
            text_muted: rgb(0xa2a2ab),
            text_faint: rgb(0x71717a),
            // Neutral, near-white accent: a monochrome action colour rather than a brand hue.
            accent: rgb(0xfafafa),
            accent_hover: rgb(0xffffff),
            accent_fg: rgb(0x131315),
            accent_subtle: rgb(0x26262b),
            success: rgb(0x5ec98f),
            warning: rgb(0xd8b15f),
            danger: rgb(0xe08383),
        }
    }

    pub const fn light() -> Self {
        Self {
            window_bg: rgb(0xededf0),
            sidebar_bg: rgb(0xededf0),
            bg: rgb(0xf7f7f8),
            bg_elevated: rgb(0xffffff),
            surface: rgb(0xffffff),
            surface_hover: rgb(0xf0f0f2),
            surface_active: rgb(0xe6e6ea),
            border: rgb(0xe4e4e8),
            border_strong: rgb(0xcfcfd6),
            text: rgb(0x1c1c1f),
            text_muted: rgb(0x5f5f68),
            text_faint: rgb(0x8e8e98),
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

    /// Dark Mica: the chrome/page is a light tint over Mica, with grouped surfaces a little more
    /// opaque so text stays legible while the wallpaper still reads through.
    pub const fn dark_mica() -> Self {
        Self {
            window_bg: rgb_a(0x0e0e10, 0.0),
            sidebar_bg: rgb_a(0x0e0e10, 0.20),
            bg: rgb_a(0x131315, 0.24),
            bg_elevated: rgb_a(0x1a1a1d, 0.52),
            surface: rgb_a(0x17171a, 0.48),
            surface_hover: rgb_a(0x1f1f23, 0.62),
            surface_active: rgb_a(0x2a2a30, 0.72),
            border: rgb_a(0xffffff, 0.10),
            border_strong: rgb_a(0xffffff, 0.18),
            accent_subtle: rgb_a(0xffffff, 0.10),
            // Lighter secondary text so it stays legible over a translucent (not pure-black) ground.
            text_muted: rgb(0xb6b6bf),
            text_faint: rgb(0x8c8c95),
            ..Self::dark()
        }
    }

    /// Light Mica: over a light backdrop, grouped surfaces stay near-opaque to keep text contrast.
    pub const fn light_mica() -> Self {
        Self {
            window_bg: rgb_a(0xededf0, 0.0),
            sidebar_bg: rgb_a(0xededf0, 0.62),
            bg: rgb_a(0xf7f7f8, 0.64),
            bg_elevated: rgb_a(0xffffff, 0.82),
            surface: rgb_a(0xffffff, 0.82),
            surface_hover: rgb_a(0xf0f0f2, 0.88),
            surface_active: rgb_a(0xe6e6ea, 0.92),
            border: rgb_a(0x000000, 0.10),
            border_strong: rgb_a(0x000000, 0.18),
            accent_subtle: rgb_a(0x000000, 0.06),
            // Darker secondary text so it stays legible over a mid-grey translucent ground.
            text: rgb(0x121215),
            text_muted: rgb(0x44444c),
            text_faint: rgb(0x5f5f68),
            ..Self::light()
        }
    }

    /// Dark Acrylic: acrylic blur is much stronger than Mica, so the surfaces step back a little
    /// further to let the blurred backdrop read while keeping text contrast.
    pub const fn dark_acrylic() -> Self {
        Self {
            window_bg: rgb_a(0x0e0e10, 0.0),
            // A more translucent nav rail, but the content ground stays dark so the window reads
            // dark even over a bright wallpaper.
            sidebar_bg: rgb_a(0x0e0e10, 0.30),
            bg: rgb_a(0x131315, 0.42),
            bg_elevated: rgb_a(0x1a1a1d, 0.30),
            surface: rgb_a(0x17171a, 0.28),
            surface_hover: rgb_a(0x1f1f23, 0.42),
            surface_active: rgb_a(0x2a2a30, 0.55),
            border: rgb_a(0xffffff, 0.14),
            border_strong: rgb_a(0xffffff, 0.22),
            accent_subtle: rgb_a(0xffffff, 0.12),
            text_muted: rgb(0xb6b6bf),
            text_faint: rgb(0x8c8c95),
            ..Self::dark()
        }
    }

    /// Light Acrylic: a translucent nav rail and ground, with cards lifted so text keeps contrast.
    pub const fn light_acrylic() -> Self {
        Self {
            window_bg: rgb_a(0xededf0, 0.0),
            sidebar_bg: rgb_a(0xededf0, 0.24),
            bg: rgb_a(0xf7f7f8, 0.42),
            bg_elevated: rgb_a(0xffffff, 0.66),
            surface: rgb_a(0xffffff, 0.64),
            surface_hover: rgb_a(0xf0f0f2, 0.74),
            surface_active: rgb_a(0xe6e6ea, 0.82),
            border: rgb_a(0x000000, 0.14),
            border_strong: rgb_a(0x000000, 0.24),
            accent_subtle: rgb_a(0x000000, 0.08),
            text: rgb(0x121215),
            text_muted: rgb(0x44444c),
            text_faint: rgb(0x5f5f68),
            ..Self::light()
        }
    }
}

const fn rgb(hex: u32) -> Rgba {
    rgb_a(hex, 1.0)
}

/// Like [`rgb`] but with an explicit alpha, used for the translucent (Mica/Acrylic) surfaces.
const fn rgb_a(hex: u32, a: f32) -> Rgba {
    Rgba {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a,
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

/// Writes the active surface treatment to both the GPUI global and the process-local mirror.
pub fn set_treatment(cx: &mut App, treatment: SurfaceTreatment) {
    cx.set_global(treatment);
    TREATMENT.store(treatment as u8, Ordering::Relaxed);
}

/// The currently active surface treatment.
pub fn current_treatment() -> SurfaceTreatment {
    match TREATMENT.load(Ordering::Relaxed) {
        1 => SurfaceTreatment::Mica,
        2 => SurfaceTreatment::Acrylic,
        _ => SurfaceTreatment::Opaque,
    }
}

/// The OS window background that matches a theme + treatment. Both Mica and Acrylic clear the
/// window with the Mica appearance (see `platform::glass`); the actual DWM backdrop type — Mica or
/// the Acrylic transient backdrop — is set explicitly there. The caller falls back to
/// [`WindowBackgroundAppearance::Opaque`] where the backdrop is unavailable.
pub fn window_background(
    appearance: Appearance,
    treatment: SurfaceTreatment,
) -> WindowBackgroundAppearance {
    match treatment {
        SurfaceTreatment::Opaque => WindowBackgroundAppearance::Opaque,
        SurfaceTreatment::Mica | SurfaceTreatment::Acrylic => match appearance {
            Appearance::Dark => WindowBackgroundAppearance::MicaBackdrop,
            Appearance::Light => WindowBackgroundAppearance::MicaAltBackdrop,
        },
    }
}

// The process-local mirrors let `current()`/`current_treatment()` work outside a GPUI context.
// `set_appearance`/`set_treatment` are the only writers, so they cannot drift from the globals.
static ACTIVE: AtomicU8 = AtomicU8::new(Appearance::Dark as u8);
static TREATMENT: AtomicU8 = AtomicU8::new(SurfaceTreatment::Opaque as u8);

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
