//! Windows 11 Mica backdrop support for the "frost" surface treatment.
//!
//! `WindowBackgroundAppearance::MicaBackdrop` makes GPUI clear the window transparently so a DWM
//! backdrop can show through. That is only safe where the backdrop actually applies, so we probe
//! `DWMWA_SYSTEMBACKDROP_TYPE` first and fall back to an opaque background if it is unsupported
//! (Windows 10, or Windows 11 builds before 22621) — otherwise the window would be left fully
//! transparent and unreadable.

use std::ffi::c_void;

use gpui::{Window, WindowBackgroundAppearance};
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE};

use crate::platform::window::raw_hwnd;

const DWMSBT_NONE: i32 = 1;
const DWMSBT_MAINWINDOW: i32 = 2;
const DWMSBT_TABBEDWINDOW: i32 = 4;

/// Applies the requested window background, returning whether a Mica backdrop is actually active.
///
/// `Opaque` (or any non-Mica appearance) always succeeds: it disables the backdrop and clears
/// opaquely. A Mica request returns `false` on unsupported systems, having left the window opaque.
pub fn apply(window: &Window, appearance: WindowBackgroundAppearance) -> bool {
    let Some(hwnd) = raw_hwnd(window) else {
        window.set_background_appearance(WindowBackgroundAppearance::Opaque);
        return false;
    };

    let backdrop = match appearance {
        WindowBackgroundAppearance::MicaBackdrop => Some(DWMSBT_MAINWINDOW),
        WindowBackgroundAppearance::MicaAltBackdrop => Some(DWMSBT_TABBEDWINDOW),
        _ => None,
    };

    let Some(backdrop) = backdrop else {
        unsafe {
            set_backdrop_type(hwnd, DWMSBT_NONE);
        }
        window.set_background_appearance(appearance);
        return matches!(appearance, WindowBackgroundAppearance::Opaque);
    };

    let result = unsafe { set_backdrop_type(hwnd, backdrop) };
    if result >= 0 {
        window.set_background_appearance(appearance);
        true
    } else {
        window.set_background_appearance(WindowBackgroundAppearance::Opaque);
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
