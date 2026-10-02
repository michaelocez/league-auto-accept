//! Win32/DWM polish for application windows: rounded corners (main window and tray popup) and the
//! popup's drop shadow.
//!
//! A `WindowKind::PopUp` has no frame, so it gets square corners and no drop shadow. On Windows 11
//! we ask DWM to round the corners and to draw a shadow (by extending a 1px frame into the client
//! area). Both are best-effort: on unsupported builds the calls simply fail and the window stays
//! square, which is still usable.

use std::mem::size_of;

use gpui::Window;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND,
};
use windows_sys::Win32::UI::Controls::MARGINS;

use crate::platform::window::raw_hwnd;

/// Applies rounded corners and a drop shadow to a popup window. No-op if the handle is unavailable.
pub fn apply_rounded_shadow(window: &Window) {
    let Some(hwnd) = raw_hwnd(window) else {
        return;
    };
    apply_to(hwnd);
}

/// Rounds a framed window's corners (Windows 11). Unlike [`apply_rounded_shadow`], it does not
/// extend the frame into the client area — the main window already has DWM chrome and a shadow, so
/// extending a frame there would bleed a hairline over the custom title bar.
pub fn apply_rounded_corners(window: &Window) {
    let Some(hwnd) = raw_hwnd(window) else {
        return;
    };
    round_corners(hwnd);
}

fn apply_to(hwnd: HWND) {
    unsafe {
        // Round the corners (Windows 11 22000+).
        round_corners(hwnd);

        // Extending a 1px frame gives the borderless window a DWM drop shadow.
        let margins = MARGINS {
            cxLeftWidth: 1,
            cxRightWidth: 1,
            cyTopHeight: 1,
            cyBottomHeight: 1,
        };
        DwmExtendFrameIntoClientArea(hwnd, &margins);
    }
}

fn round_corners(hwnd: HWND) {
    unsafe {
        let preference: i32 = DWMWCP_ROUND;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &preference as *const i32 as *const core::ffi::c_void,
            size_of::<i32>() as u32,
        );
    }
}
