use crate::league::service::ServiceEvent;
use crate::notifications::discord::WebhookResult;
use crate::platform::tray::TrayCommand;

/// Events delivered from background threads (tray, League service, notifications) to the UI.
///
/// This replaces the Electron `broadcastState()` IPC hop: the backend never touches GPUI types,
/// it only sends plain owned data over an `async_channel`.
#[derive(Clone, Debug)]
pub enum AppEvent {
    Tray(TrayCommand),
    /// An event from the headless League service (connection / ready check / lifecycle).
    Service(ServiceEvent),
    /// The result of a user-triggered Test Webhook.
    WebhookTest(WebhookResult),
}

/// What the UI should do after applying an event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventOutcome {
    Continue,
    ShowWindow,
    Quit,
}
