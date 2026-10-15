use crate::config::settings::AppSettings;
use crate::league::ready_check::{Lifecycle, Status as LeagueStatus};
use crate::league::service::ServiceEvent;
use crate::notifications::discord::{WebhookResult, WebhookStatus};

/// A recent activity entry shown on the dashboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityItem {
    pub label: String,
    pub kind: ActivityKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityKind {
    Info,
    Accepted,
}

const MAX_ACTIVITY: usize = 8;

/// LCU connection status shown to the user.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
}

impl ConnectionStatus {
    pub fn label(self) -> &'static str {
        match self {
            ConnectionStatus::Disconnected => "League disconnected",
            ConnectionStatus::Connecting => "League connecting",
            ConnectionStatus::Connected => "League connected",
        }
    }
}

/// Ready-check status shown to the user.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReadyCheckStatus {
    Idle,
    Searching,
    Ready,
    Accepting,
    Accepted,
    Error,
}

impl ReadyCheckStatus {
    pub fn from_league(status: LeagueStatus) -> Self {
        match status {
            LeagueStatus::Idle => ReadyCheckStatus::Idle,
            LeagueStatus::Searching => ReadyCheckStatus::Searching,
            LeagueStatus::Ready => ReadyCheckStatus::Ready,
            LeagueStatus::Accepting => ReadyCheckStatus::Accepting,
            LeagueStatus::Accepted => ReadyCheckStatus::Accepted,
            LeagueStatus::Error => ReadyCheckStatus::Error,
        }
    }
}

/// The single authoritative application state. Both the window and the tray derive from this.
///
/// It holds no secret: the Discord webhook URL lives in the settings store and the notification
/// sender, and only its [`WebhookStatus`] reaches this state.
#[derive(Clone, Debug)]
pub struct AppState {
    pub settings: AppSettings,
    pub connection: ConnectionStatus,
    pub ready_check: ReadyCheckStatus,
    pub ready_check_message: String,
    /// Recent lifecycle activity, newest first (bounded).
    pub activity: Vec<ActivityItem>,
    /// Non-secret status of the saved Discord webhook.
    pub webhook_status: WebhookStatus,
    /// Last Test Webhook result, if any.
    pub webhook_test: Option<WebhookResult>,
    /// Whether a Test Webhook request is in flight.
    pub webhook_testing: bool,
    /// The message shown in the window's error banner: the last settings write that was refused
    /// (the change is still applied in memory), or a background worker that has stopped.
    pub settings_error: Option<String>,
}

impl AppState {
    pub fn new(settings: AppSettings, webhook_status: WebhookStatus) -> Self {
        Self {
            settings,
            connection: ConnectionStatus::Connecting,
            ready_check: ReadyCheckStatus::Idle,
            ready_check_message: "Waiting for a ready check.".to_string(),
            activity: Vec::new(),
            webhook_status,
            webhook_test: None,
            webhook_testing: false,
            settings_error: None,
        }
    }

    /// Applies a headless-service event to application state.
    pub fn apply_service_event(&mut self, event: &ServiceEvent) {
        match event {
            ServiceEvent::Connection { connected } => {
                self.connection = if *connected {
                    ConnectionStatus::Connected
                } else {
                    ConnectionStatus::Disconnected
                };
            }
            ServiceEvent::ReadyCheck { status, message } => {
                self.ready_check = ReadyCheckStatus::from_league(*status);
                self.ready_check_message = message.clone();
            }
            ServiceEvent::Lifecycle(lifecycle) => {
                let (label, kind) = match lifecycle {
                    Lifecycle::QueuePopped => ("Match found", ActivityKind::Info),
                    Lifecycle::AutoAccepted => ("Match accepted", ActivityKind::Accepted),
                    Lifecycle::GameStarted => ("Game started", ActivityKind::Info),
                };
                self.push_activity(label, kind);
            }
            // The worker that feeds this channel has stopped, so nothing else will arrive: the UI
            // must stop reporting a state it can no longer observe and say what happened instead of
            // waiting on a channel that will stay silent.
            ServiceEvent::Stopped { message } => {
                self.connection = ConnectionStatus::Disconnected;
                self.ready_check = ReadyCheckStatus::Error;
                self.ready_check_message = message.clone();
                self.webhook_testing = false;
                self.settings_error = Some(message.clone());
            }
        }
    }

    fn push_activity(&mut self, label: &str, kind: ActivityKind) {
        self.activity.insert(
            0,
            ActivityItem {
                label: label.to_string(),
                kind,
            },
        );
        self.activity.truncate(MAX_ACTIVITY);
    }

    pub fn summary(&self) -> &'static str {
        if !self.settings.auto_accept_enabled {
            "Disabled"
        } else {
            match self.connection {
                ConnectionStatus::Connected => "Active",
                ConnectionStatus::Connecting => "Connecting",
                ConnectionStatus::Disconnected => "League offline",
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_reports_active_unless_enabled_and_connected() {
        let mut state = AppState::new(AppSettings::default(), WebhookStatus::default());

        state.connection = ConnectionStatus::Connected;
        assert_eq!(state.summary(), "Disabled");

        state.settings.auto_accept_enabled = true;
        state.connection = ConnectionStatus::Disconnected;
        assert_eq!(state.summary(), "League offline");

        state.connection = ConnectionStatus::Connecting;
        assert_eq!(state.summary(), "Connecting");

        state.connection = ConnectionStatus::Connected;
        assert_eq!(state.summary(), "Active");
    }

    #[test]
    fn maps_service_events_and_bounds_activity() {
        let mut state = AppState::new(AppSettings::default(), WebhookStatus::default());

        state.apply_service_event(&ServiceEvent::Connection { connected: true });
        assert_eq!(state.connection, ConnectionStatus::Connected);

        state.apply_service_event(&ServiceEvent::ReadyCheck {
            status: LeagueStatus::Ready,
            message: "Ready check detected; accepting…".into(),
        });
        assert_eq!(state.ready_check, ReadyCheckStatus::Ready);

        state.apply_service_event(&ServiceEvent::Lifecycle(Lifecycle::QueuePopped));
        assert_eq!(state.activity[0].label, "Match found");

        for _ in 0..20 {
            state.apply_service_event(&ServiceEvent::Lifecycle(Lifecycle::GameStarted));
        }
        assert_eq!(state.activity.len(), MAX_ACTIVITY);
    }

    /// A stopped worker still has to change the state the UI derives from it: the connection is no
    /// longer observable, an in-flight test cannot still be in flight, and the banner says what
    /// happened instead of the window quietly claiming the old state.
    #[test]
    fn a_stopped_worker_ends_the_wait_and_stops_reporting_a_live_service() {
        let mut state = AppState::new(AppSettings::default(), WebhookStatus::default());
        state.settings.auto_accept_enabled = true;
        state.apply_service_event(&ServiceEvent::Connection { connected: true });
        state.webhook_testing = true;

        state.apply_service_event(&ServiceEvent::Stopped {
            message: "The League service stopped unexpectedly (boom).".to_string(),
        });

        assert_eq!(state.connection, ConnectionStatus::Disconnected);
        assert_eq!(
            state.summary(),
            "League offline",
            "no longer claims to be active"
        );
        assert_eq!(state.ready_check, ReadyCheckStatus::Error);
        assert!(!state.webhook_testing, "nothing can still be in flight");
        assert_eq!(
            state.settings_error.as_deref(),
            Some("The League service stopped unexpectedly (boom).")
        );
    }

    #[test]
    fn application_state_never_carries_the_webhook_secret() {
        let secret = crate::notifications::discord::WebhookSecret::new(
            "https://discord.com/api/webhooks/12345678901234567/secret-token-value",
        );
        let state = AppState::new(AppSettings::default(), secret.status());

        assert!(state.webhook_status.configured);
        let rendered = format!("{state:?}");
        assert!(!rendered.contains("secret-token-value"), "{rendered}");
    }
}
