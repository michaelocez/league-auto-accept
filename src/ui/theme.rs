//! Visual language: one token system for colour, spacing, radii, typography, sizes and motion.
//!
//! The palette is defined twice — a resolved **dark** set and a designed **light** set (light is
//! authored, never an inversion of dark). The active set is selected by the [`Appearance`] global,
//! which the UI reads via `cx.global::<Appearance>()` and which is written by [`set_appearance`].
//!
//! The palette direction (neutral layered surfaces, a monochrome accent, colour reserved for state)
//! is original to this project; no third-party visual identity is copied.

use std::sync::atomic::{AtomicU8, Ordering};

use gpui::{px, App, FontWeight, Global, Pixels, Rgba, SpringConfig, WindowBackgroundAppearance};

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
    /// Accent while pressed: one step past hover, so a click reads as a press rather than a flicker.
    pub accent_active: Rgba,
    /// Foreground used on top of the accent.
    pub accent_fg: Rgba,
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
            // The rail is the deepest surface; the page beside it is a hair lighter and the cards
            // step up again, so the layered fills — not a hairline — are what separate them.
            sidebar_bg: rgb(0x0a0a0d),
            bg: rgb(0x101014),
            bg_elevated: rgb(0x17171c),
            surface: rgb(0x141419),
            surface_hover: rgb(0x1c1c22),
            surface_active: rgb(0x26262e),
            border: rgb(0x1f1f26),
            border_strong: rgb(0x33333d),
            text: rgb(0xf5f5f7),
            text_muted: rgb(0xa6a6b0),
            text_faint: rgb(0x74747f),
            // Neutral, near-white accent: a monochrome action colour rather than a brand hue.
            accent: rgb(0xfafafa),
            accent_hover: rgb(0xffffff),
            accent_active: rgb(0xe4e4e7),
            accent_fg: rgb(0x101014),
            success: rgb(0x5ec98f),
            warning: rgb(0xd8b15f),
            danger: rgb(0xe08383),
        }
    }

    pub const fn light() -> Self {
        Self {
            sidebar_bg: rgb(0xe9e9ee),
            bg: rgb(0xf5f5f7),
            bg_elevated: rgb(0xffffff),
            surface: rgb(0xffffff),
            surface_hover: rgb(0xececef),
            surface_active: rgb(0xe2e2e7),
            border: rgb(0xdedee4),
            border_strong: rgb(0xc6c6ce),
            text: rgb(0x1a1a1e),
            text_muted: rgb(0x5b5b66),
            text_faint: rgb(0x8a8a95),
            // Neutral, near-black accent in light mode.
            accent: rgb(0x1a1a1e),
            accent_hover: rgb(0x000000),
            accent_active: rgb(0x33333a),
            accent_fg: rgb(0xffffff),
            success: rgb(0x2f9e63),
            warning: rgb(0xa8761f),
            danger: rgb(0xc94a4a),
        }
    }

    /// Dark Mica: the chrome/page is a light tint over Mica, with grouped surfaces a little more
    /// opaque so text stays legible while the wallpaper still reads through.
    pub const fn dark_mica() -> Self {
        Self {
            // The rail and the page carry the same tint, so the window reads as one glass sheet and
            // only the base colour separates them — the rail is defined by its items, not by a band.
            sidebar_bg: rgb_a(0x0a0a0d, 0.16),
            bg: rgb_a(0x101014, 0.16),
            bg_elevated: rgb_a(0x17171c, 0.24),
            surface: rgb_a(0x141419, 0.22),
            surface_hover: rgb_a(0x1c1c22, 0.32),
            surface_active: rgb_a(0x26262e, 0.44),
            border: rgb_a(0xffffff, 0.10),
            border_strong: rgb_a(0xffffff, 0.18),
            // Lighter secondary text so it stays legible over a translucent (not pure-black) ground.
            text_muted: rgb(0xb9b9c2),
            text_faint: rgb(0x8f8f99),
            ..Self::dark()
        }
    }

    /// Light Mica: over a light backdrop, grouped surfaces stay near-opaque to keep text contrast.
    pub const fn light_mica() -> Self {
        Self {
            sidebar_bg: rgb_a(0xe9e9ee, 0.40),
            bg: rgb_a(0xf5f5f7, 0.40),
            bg_elevated: rgb_a(0xffffff, 0.52),
            surface: rgb_a(0xffffff, 0.50),
            surface_hover: rgb_a(0xececef, 0.62),
            surface_active: rgb_a(0xe2e2e7, 0.72),
            border: rgb_a(0x000000, 0.10),
            border_strong: rgb_a(0x000000, 0.18),
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
            // Acrylic blurs what is behind the window, so the tint can sit far back: the rail and
            // page keep the same light tint and the cards carry just enough to hold text.
            sidebar_bg: rgb_a(0x0a0a0d, 0.14),
            bg: rgb_a(0x101014, 0.14),
            bg_elevated: rgb_a(0x17171c, 0.22),
            surface: rgb_a(0x141419, 0.20),
            surface_hover: rgb_a(0x1c1c22, 0.30),
            surface_active: rgb_a(0x26262e, 0.42),
            border: rgb_a(0xffffff, 0.14),
            border_strong: rgb_a(0xffffff, 0.22),
            text_muted: rgb(0xb6b6bf),
            text_faint: rgb(0x8c8c95),
            ..Self::dark()
        }
    }

    /// Light Acrylic: a translucent nav rail and ground, with cards lifted so text keeps contrast.
    pub const fn light_acrylic() -> Self {
        Self {
            sidebar_bg: rgb_a(0xe9e9ee, 0.18),
            bg: rgb_a(0xf5f5f7, 0.18),
            bg_elevated: rgb_a(0xffffff, 0.38),
            surface: rgb_a(0xffffff, 0.36),
            surface_hover: rgb_a(0xececef, 0.48),
            surface_active: rgb_a(0xe2e2e7, 0.60),
            border: rgb_a(0x000000, 0.14),
            border_strong: rgb_a(0x000000, 0.24),
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

/// Mirrors the active theme into the framework's global theme, which the text inputs resolve their
/// caret, selection and placeholder colours from.
///
/// The input widget takes those colours from the framework's own tokens rather than from the
/// `TextStyle` around it, and those tokens begin as the framework's *light* palette — which is why
/// the insertion point stayed near-black, and so invisible, on the dark theme. Keeping those tokens
/// in step with ours makes the caret and selection follow the app's appearance.
fn sync_base_theme(cx: &mut App) {
    let theme = Theme::of(cx);
    let colors = &mut gpui::base::Theme::global_mut(cx).tokens.colors;
    colors.foreground = theme.text.into();
    colors.muted_foreground = theme.text_muted.into();
    colors.accent = theme.accent.into();
    colors.surface = theme.surface.into();
    colors.border = theme.border.into();
}

/// Writes the active appearance to both the GPUI global and the process-local mirror.
pub fn set_appearance(cx: &mut App, appearance: Appearance) {
    cx.set_global(appearance);
    // Relaxed is sufficient: a process-local mirror of the global, read on this same thread only.
    ACTIVE.store(appearance as u8, Ordering::Relaxed);
    sync_base_theme(cx);
}

/// The currently active appearance.
pub fn current() -> Appearance {
    // Relaxed is sufficient: as in `set_appearance`, the mirror orders nothing else.
    if ACTIVE.load(Ordering::Relaxed) == Appearance::Light as u8 {
        Appearance::Light
    } else {
        Appearance::Dark
    }
}

/// Writes the active surface treatment to both the GPUI global and the process-local mirror.
pub fn set_treatment(cx: &mut App, treatment: SurfaceTreatment) {
    cx.set_global(treatment);
    // Relaxed is sufficient: a process-local mirror of the global, read on this same thread only.
    TREATMENT.store(treatment as u8, Ordering::Relaxed);
    sync_base_theme(cx);
}

/// The currently active surface treatment.
pub fn current_treatment() -> SurfaceTreatment {
    // Relaxed is sufficient: as in `set_treatment`, the mirror orders nothing else.
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
    px(8.0)
}
pub fn radius_md() -> Pixels {
    px(10.0)
}
pub fn radius_lg() -> Pixels {
    px(16.0)
}

// --- Type scale ------------------------------------------------------------------------------

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
pub fn text_micro() -> Pixels {
    px(11.0)
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

// --- Fixed sizes ------------------------------------------------------------------------------
//
// Dimensions the chrome repeats: a hairline, the standard marks and dots, and the shell's own
// measurements. Anything that only one element needs stays a constant beside that element.

/// A single-pixel hairline: separators and borders, below the 4px spacing grid.
pub fn hairline() -> Pixels {
    px(1.0)
}

/// A status or activity dot.
pub fn size_dot() -> Pixels {
    px(8.0)
}

/// A navigation glyph's box, so labels align regardless of the glyph's own width.
pub fn size_icon() -> Pixels {
    px(16.0)
}

/// The application mark drawn in the title bar.
pub fn size_mark() -> Pixels {
    px(18.0)
}

/// The title bar's height, and the height of the window controls inside it.
pub fn size_title_bar() -> Pixels {
    px(44.0)
}

/// One window control button's width.
pub fn size_control() -> Pixels {
    px(46.0)
}

/// The navigation rail's width.
pub fn size_sidebar() -> Pixels {
    px(190.0)
}

/// The widest a text field's column grows, so text stays readable instead of stretching.
pub fn size_field_max() -> Pixels {
    px(440.0)
}

/// An extra-small radius for miniature surfaces (the theme previews).
pub fn radius_xs() -> Pixels {
    px(2.0)
}

/// The tightest step: insets and gaps that need to be smaller than the 4px grid.
pub fn space_half() -> Pixels {
    px(2.0)
}

// --- Motion -----------------------------------------------------------------------------------

/// The spring used wherever a state change should read as motion rather than an instant flip:
/// quick and near-critically damped, so there is no visible overshoot.
pub fn spring_snappy() -> SpringConfig {
    SpringConfig::new(180.0, 26.0, 1.0)
}

/// Hover alpha for a control whose fill is animated (the toggle's spring), where a flat hover fill
/// would fight the animation. Controls with flat fills use the state fills instead.
pub fn hover_alpha() -> f32 {
    0.92
}

/// Press alpha, for the same animated-fill controls.
pub fn press_alpha() -> f32 {
    0.85
}
