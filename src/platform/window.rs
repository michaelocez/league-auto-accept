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
use std::sync::OnceLock;

use gpui::Window;

use crate::app::events::AppEvent;
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

/// Where a window close is handed to the application.
///
/// The window procedure cannot write the pending settings edit itself — that is GPUI view state, and
/// calling into GPUI from inside the Win32 message dispatch would re-enter its borrows. So a close
/// with minimize-to-tray off becomes an application event, and the window stays open until the
/// application exits through its own shutdown path.
static CLOSE_REQUESTS: OnceLock<async_channel::Sender<AppEvent>> = OnceLock::new();

/// Routes a window close to the application. Call once, before the main window is opened.
pub fn request_exit_through(tx: async_channel::Sender<AppEvent>) {
    let _ = CLOSE_REQUESTS.set(tx);
}

/// What a `WM_CLOSE` means for the window procedure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseAction {
    /// Minimize-to-tray is on: hide the window and keep running.
    Hide,
    /// Minimize-to-tray is off: ask the application to exit, so pending settings are written first.
    RequestExit,
}

fn close_action(hide_on_close: bool) -> CloseAction {
    if hide_on_close {
        CloseAction::Hide
    } else {
        CloseAction::RequestExit
    }
}

/// Asks the application to exit. `false` means the request could not be delivered, so the window has
/// to be allowed to close directly rather than becoming unclosable.
fn request_exit() -> bool {
    CLOSE_REQUESTS
        .get()
        .is_some_and(|tx| tx.try_send(AppEvent::CloseRequested).is_ok())
}

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
    // Relaxed is sufficient: a lone boolean the window procedure re-reads, published with nothing.
    HIDE_ON_CLOSE.store(hide_on_close, Ordering::Relaxed);
}

/// Hides the window instead of closing it while minimize-to-tray is on, hands a close to the
/// application while it is off, and lets every other message reach GPUI's own window procedure
/// untouched.
unsafe extern "system" fn close_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _reference_data: usize,
) -> LRESULT {
    if msg == WM_CLOSE {
        // Relaxed is sufficient: the subclass re-reads the flag on each close; nothing else rides on it.
        match close_action(HIDE_ON_CLOSE.load(Ordering::Relaxed)) {
            CloseAction::Hide => {
                log::info!("close intercepted: hiding to tray");
                ShowWindow(hwnd, SW_HIDE);
                return 0;
            }
            CloseAction::RequestExit => {
                // Exiting through the application writes the debounced settings edit first; letting
                // WM_CLOSE through here would drop the last keystrokes.
                if request_exit() {
                    log::info!("close requested: exiting through the application");
                    return 0;
                }
                log::warn!("could not deliver the close request; closing the window directly");
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_close_hides_to_the_tray_or_asks_the_application_to_exit() {
        assert_eq!(close_action(true), CloseAction::Hide);
        assert_eq!(close_action(false), CloseAction::RequestExit);
    }

    #[test]
    fn requesting_an_exit_delivers_one_close_request() {
        let (tx, rx) = async_channel::unbounded();
        // The slot is process-wide and can only be filled once, so this is the only test that sets it.
        request_exit_through(tx);

        assert!(request_exit());
        assert!(matches!(rx.try_recv(), Ok(AppEvent::CloseRequested)));
        assert!(rx.try_recv().is_err(), "one close means one request");
    }
}
