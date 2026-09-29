//! Single-instance behaviour for Windows.
//!
//! Uses a named mutex to detect a second launch and a named auto-reset event so the second
//! launch can ask the first to show its window. No manual Win32 window is required, so this
//! does not conflict with GPUI's own message loop.
//!
//! Observable behaviour mirrors the Electron app: a second launch focuses the first and exits.

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject, INFINITE,
};

const MUTEX_NAME: &str = "Local\\LeagueAutoAccept.RustRewrite.Instance";
const EVENT_NAME: &str = "Local\\LeagueAutoAccept.RustRewrite.ShowWindow";

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Held for the lifetime of the primary instance; dropping it releases the mutex.
pub struct InstanceGuard {
    mutex: HANDLE,
}

// The handle is only used via thread-safe Win32 calls.
unsafe impl Send for InstanceGuard {}
unsafe impl Sync for InstanceGuard {}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if !self.mutex.is_null() {
            unsafe { CloseHandle(self.mutex) };
        }
    }
}

/// Returns `Some(guard)` for the first instance, or `None` if another instance already runs.
pub fn acquire() -> Option<InstanceGuard> {
    let name = wide(MUTEX_NAME);
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if mutex.is_null() {
        log::error!("could not create single-instance mutex");
        return None;
    }
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_exists {
        unsafe { CloseHandle(mutex) };
        return None;
    }
    Some(InstanceGuard { mutex })
}

/// Called by a secondary launch to signal the primary instance to show its window.
pub fn signal_primary() {
    let name = wide(EVENT_NAME);
    let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
    if event.is_null() {
        return;
    }
    unsafe {
        SetEvent(event);
        CloseHandle(event);
    }
}

/// Primary instance: watch for signals from secondary launches and invoke `on_signal`.
pub fn spawn_primary_watcher(on_signal: impl Fn() + Send + 'static) {
    let spawned = std::thread::Builder::new()
        .name("laa-instance-watch".to_string())
        .spawn(move || {
            let name = wide(EVENT_NAME);
            let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
            if event.is_null() {
                log::error!("could not create instance event");
                return;
            }
            loop {
                let result = unsafe { WaitForSingleObject(event, INFINITE) };
                if result == WAIT_OBJECT_0 {
                    on_signal();
                } else {
                    break;
                }
            }
            unsafe { CloseHandle(event) };
        });
    if let Err(error) = spawned {
        log::error!("could not spawn instance watcher: {error}");
    }
}
