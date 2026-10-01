//! Win32/DWM polish for the borderless tray popup window.
//!
//! A `WindowKind::PopUp` has no frame, so it gets square corners and no drop shadow. On Windows 11
//! we ask DWM to round the corners and to draw a shadow (by extending a 1px frame into the client
//! area). Both are best-effort: on unsupported builds the calls simply fail and the popup stays
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

fn apply_to(hwnd: HWND) {
    unsafe {
        // Round the corners (Windows 11 22000+).
        let preference: i32 = DWMWCP_ROUND;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &preference as *const i32 as *const core::ffi::c_void,
            size_of::<i32>() as u32,
        );

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
