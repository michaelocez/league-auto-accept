use crate::platform::tray::TrayCommand;

/// Events delivered from background threads (tray, async service, future LCU service) to the UI.
///
/// This replaces the Electron `broadcastState()` IPC hop: the backend never touches GPUI types,
/// it only sends plain owned data over an `async_channel`.
#[derive(Clone, Debug)]
pub enum AppEvent {
    Tray(TrayCommand),
    /// Heartbeat from the background async service (Spike C).
    BackendTick {
        uptime_secs: u64,
    },
    /// Connection state change (stub in Phase 1).
    Connection {
        connected: bool,
        message: String,
    },
}

/// What the UI should do after applying an event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventOutcome {
    Continue,
    ShowWindow,
    Quit,
}
