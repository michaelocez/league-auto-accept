//! Window show/hide and close interception for Windows.
//!
//! GPUI 0.2.2's `App::hide()` and `App::activate()` are **no-ops on Windows**, and there is no
//! public `Window::hide`/`show`. To implement hide-to-tray correctly we operate on the real
//! window handle (`gpui::Window` implements `raw_window_handle::HasWindowHandle`).
//!
//! All operations are pure Win32 and deliberately do not call back into GPUI: doing so from
//! inside a GPUI update re-enters the window's interior borrows (`RefCell already borrowed`).
//! Hiding/showing raises `WM_SHOWWINDOW`, which GPUI already handles to suspend/resume painting.
//!
//! Closing is decided here as well. GPUI 0.2.2 offers `Window::on_window_should_close`, but the
//! observer it registers is never invoked on Windows: the window closes and the process exits
//! whatever the callback would have returned. The window procedure is subclassed instead, which
//! also covers every other close path — Alt+F4, the taskbar menu and the system menu — that a
//! custom close button or a hit-test override would miss.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IsIconic, SetForegroundWindow, ShowWindow, SW_HIDE, SW_RESTORE, SW_SHOW, WM_CLOSE,
};

/// Whether closing the main window hides it to the tray instead of exiting, mirroring
/// `minimizeToTray`. There is one main window per process, so the flag is process-wide.
static HIDE_ON_CLOSE: AtomicBool = AtomicBool::new(false);

/// Identifies our subclass within the window; unique per window, any non-zero value works.
const CLOSE_SUBCLASS_ID: usize = 1;

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

/// Makes closing the window hide it to the tray while `hide_on_close` is set.
pub fn intercept_close(window: &Window, hide_on_close: bool) {
    set_hide_on_close(hide_on_close);
    let Some(hwnd) = raw_hwnd(window) else {
        log::warn!("could not obtain the window handle to intercept close");
        return;
    };
    if unsafe { SetWindowSubclass(hwnd, Some(close_subclass), CLOSE_SUBCLASS_ID, 0) } == 0 {
        log::warn!("could not install the close-to-tray subclass");
    }
}

/// Keeps the close behaviour in step with the `minimizeToTray` setting.
pub fn set_hide_on_close(hide_on_close: bool) {
    HIDE_ON_CLOSE.store(hide_on_close, Ordering::Relaxed);
}

/// Hides the window instead of closing it while minimize-to-tray is on, and lets every other
/// message reach GPUI's own window procedure untouched.
unsafe extern "system" fn close_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _reference_data: usize,
) -> LRESULT {
    if msg == WM_CLOSE {
        if HIDE_ON_CLOSE.load(Ordering::Relaxed) {
            log::info!("close intercepted: hiding to tray");
            ShowWindow(hwnd, SW_HIDE);
            return 0;
        }
        log::info!("close allowed: exiting application");
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
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
