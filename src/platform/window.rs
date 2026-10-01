//! Window show/hide for Windows.
//!
//! GPUI 0.2.2's `App::hide()` and `App::activate()` are **no-ops on Windows**, and there is no
//! public `Window::hide`/`show`. To implement hide-to-tray correctly we operate on the real
//! window handle (`gpui::Window` implements `raw_window_handle::HasWindowHandle`).
//!
//! All operations are pure Win32 and deliberately do not call back into GPUI: doing so from
//! inside a GPUI update re-enters the window's interior borrows (`RefCell already borrowed`).
//! Hiding/showing raises `WM_SHOWWINDOW`, which GPUI already handles to suspend/resume painting.

use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IsIconic, SetForegroundWindow, ShowWindow, SW_HIDE, SW_RESTORE, SW_SHOW,
};

/// Returns the native window handle for a GPUI window.
///
/// `Window::window_handle()` resolves to GPUI's own `AnyWindowHandle`; the
/// `raw_window_handle::HasWindowHandle` trait method is called explicitly to get the raw handle.
pub fn raw_hwnd(window: &Window) -> Option<HWND> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
        _ => None,
    }
}

/// Hides the window without closing or destroying it.
pub fn hide(window: &Window) {
    match raw_hwnd(window) {
        Some(hwnd) => unsafe {
            ShowWindow(hwnd, SW_HIDE);
        },
        None => log::warn!("could not obtain the window handle to hide"),
    }
}

/// Restores and focuses a previously hidden or minimized window.
///
/// Takes the handle as an `isize` (the form in which it is stored between calls) rather than a
/// raw pointer.
pub fn show_hwnd(hwnd: isize) {
    let hwnd = hwnd as HWND;
    if hwnd.is_null() {
        log::warn!("cannot show window: null handle");
        return;
    }
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        } else {
            ShowWindow(hwnd, SW_SHOW);
        }
        SetForegroundWindow(hwnd);
    }
}
