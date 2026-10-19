//! Windows 11 backdrop support for the translucent surface treatments.
//!
//! GPUI must be told to clear the window transparently before a DWM backdrop can show through, and
//! the only safe appearance for that is the Mica one — GPUI's own `Blurred` path applies a legacy
//! white-tinted acrylic that washes the window out. So for both Mica and Acrylic we clear with the
//! Mica appearance and then set `DWMWA_SYSTEMBACKDROP_TYPE` explicitly, which wins:
//!
//! - Mica (`DWMSBT_MAINWINDOW`) samples the desktop wallpaper.
//! - Acrylic (`DWMSBT_TRANSIENTWINDOW`) blurs what is behind the window in real time.
//!
//! The probe is best-effort: on Windows 10 or pre-22621 builds the attribute fails and the window
//! falls back to opaque, rather than being left transparent and unreadable.
//!
//! The material itself has a light and a dark variant, and Windows picks it from the *system* app
//! mode rather than from this app's theme, so the app also sets `DWMWA_USE_IMMERSIVE_DARK_MODE`.
//! Without it a dark theme over a light material composites to roughly `#d1d1d1`, where the
//! near-white text lands near 1.4:1 — which is exactly what a Light-mode Windows would show on a
//! fresh install.

use std::ffi::c_void;

use gpui::{Window, WindowBackgroundAppearance};
use windows_sys::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
};

use crate::platform::window::raw_hwnd;
use crate::ui::theme::{Appearance, SurfaceTreatment};

const DWMSBT_NONE: i32 = 1;
const DWMSBT_MAINWINDOW: i32 = 2;
const DWMSBT_TRANSIENTWINDOW: i32 = 3;
const DWMSBT_TABBEDWINDOW: i32 = 4;

/// Applies a surface treatment, returning whether a DWM backdrop is actually active.
///
/// `Opaque` always succeeds. Mica/Acrylic return `false` where the backdrop is unsupported, having
/// left the window opaque.
pub fn apply(window: &Window, appearance: Appearance, treatment: SurfaceTreatment) -> bool {
    let Some(hwnd) = raw_hwnd(window) else {
        window.set_background_appearance(WindowBackgroundAppearance::Opaque);
        return false;
    };

    // Keep the DWM material on the same side as the theme the app is drawing, whatever the system
    // app mode happens to be.
    set_dark_mode(hwnd, appearance == Appearance::Dark);

    if treatment == SurfaceTreatment::Opaque {
        window.set_background_appearance(WindowBackgroundAppearance::Opaque);
        unsafe {
            set_backdrop_type(hwnd, DWMSBT_NONE);
        }
        return true;
    }

    let gpui_background = match appearance {
        Appearance::Dark => WindowBackgroundAppearance::MicaBackdrop,
        Appearance::Light => WindowBackgroundAppearance::MicaAltBackdrop,
    };
    let backdrop = match treatment {
        SurfaceTreatment::Mica => match appearance {
            Appearance::Dark => DWMSBT_MAINWINDOW,
            Appearance::Light => DWMSBT_TABBEDWINDOW,
        },
        SurfaceTreatment::Acrylic => DWMSBT_TRANSIENTWINDOW,
        SurfaceTreatment::Opaque => unreachable!("handled above"),
    };

    // Clear transparently first (the Mica appearance sets its own DWM type), then override with the
    // backdrop type we actually want so it takes precedence.
    window.set_background_appearance(gpui_background);
    if unsafe { set_backdrop_type(hwnd, backdrop) } >= 0 {
        true
    } else {
        window.set_background_appearance(WindowBackgroundAppearance::Opaque);
        unsafe {
            set_backdrop_type(hwnd, DWMSBT_NONE);
        }
        false
    }
}

unsafe fn set_backdrop_type(hwnd: windows_sys::Win32::Foundation::HWND, backdrop: i32) -> i32 {
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_SYSTEMBACKDROP_TYPE as u32,
        &backdrop as *const i32 as *const c_void,
        std::mem::size_of::<i32>() as u32,
    )
}

/// Selects DWM's light or dark material. Best effort: the attribute is unsupported before Windows 10
/// 1809 and the call simply fails there, which is why the result is ignored.
fn set_dark_mode(hwnd: windows_sys::Win32::Foundation::HWND, dark: bool) {
    let value = i32::from(dark);
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &value as *const i32 as *const c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}
