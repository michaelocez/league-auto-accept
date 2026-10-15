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
    /// The user closed the main window while minimize-to-tray was off. The window procedure routes
    /// this back through the application so the pending settings write happens before the exit.
    CloseRequested,
    /// An event from the headless League service (connection / ready check / lifecycle).
    Service(ServiceEvent),
    /// The result of a user-triggered Test Webhook.
    WebhookTest(WebhookResult),
}

impl AppEvent {
    /// Whether this event asks the application to exit.
    ///
    /// Every exit path has to write the pending debounced settings edit first, so exit intents are
    /// recognised in one place instead of being repeated per variant — where a new one could
    /// silently skip the flush.
    pub fn requests_exit(&self) -> bool {
        matches!(self, Self::CloseRequested | Self::Tray(TrayCommand::Quit))
    }
}

/// What the UI should do after applying an event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventOutcome {
    Continue,
    ShowWindow,
    Quit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_quit_or_a_window_close_asks_the_application_to_exit() {
        assert!(AppEvent::CloseRequested.requests_exit());
        assert!(AppEvent::Tray(TrayCommand::Quit).requests_exit());

        for event in [
            AppEvent::Tray(TrayCommand::ShowWindow),
            AppEvent::Tray(TrayCommand::OpenPopup),
            AppEvent::Tray(TrayCommand::ToggleAutoAccept),
            AppEvent::Tray(TrayCommand::ToggleDiscordNotifications),
        ] {
            assert!(
                !event.requests_exit(),
                "{event:?} must not be treated as an exit"
            );
        }
    }
}
