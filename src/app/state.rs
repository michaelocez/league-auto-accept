use crate::config::settings::AppSettings;
use crate::league::ready_check::{Lifecycle, Status as LeagueStatus};
use crate::league::service::ServiceEvent;

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
    Warning,
}

const MAX_ACTIVITY: usize = 8;

/// LCU connection status. Mirrors the Electron `ConnectionStatus` contract.
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

/// Ready-check status. Mirrors the Electron `ReadyCheckStatus` contract.
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

    pub fn label(self) -> &'static str {
        match self {
            ReadyCheckStatus::Idle => "idle",
            ReadyCheckStatus::Searching => "searching",
            ReadyCheckStatus::Ready => "ready",
            ReadyCheckStatus::Accepting => "accepting",
            ReadyCheckStatus::Accepted => "accepted",
            ReadyCheckStatus::Error => "error",
        }
    }
}

/// The single authoritative application state. Both the window and the tray derive from this.
///
/// Phase 1 keeps this deliberately small; the full model (activity log, notification state,
/// reconnect state, ...) arrives with the League service in Phase 2.
#[derive(Clone, Debug)]
pub struct AppState {
    pub settings: AppSettings,
    pub connection: ConnectionStatus,
    pub connection_message: String,
    pub ready_check: ReadyCheckStatus,
    pub ready_check_message: String,
    /// Recent lifecycle activity, newest first (bounded).
    pub activity: Vec<ActivityItem>,
}

impl AppState {
    pub fn new(settings: AppSettings) -> Self {
        Self {
            settings,
            connection: ConnectionStatus::Connecting,
            connection_message: "Starting League Client connection…".to_string(),
            ready_check: ReadyCheckStatus::Idle,
            ready_check_message: "Waiting for a ready check.".to_string(),
            activity: Vec::new(),
        }
    }

    /// Applies a headless-service event to application state.
    pub fn apply_service_event(&mut self, event: &ServiceEvent) {
        match event {
            ServiceEvent::Connection { connected, message } => {
                self.connection = if *connected {
                    ConnectionStatus::Connected
                } else {
                    ConnectionStatus::Disconnected
                };
                self.connection_message = message.clone();
            }
            ServiceEvent::ReadyCheck { status, message } => {
                self.ready_check = ReadyCheckStatus::from_league(*status);
                self.ready_check_message = message.clone();
            }
            ServiceEvent::Lifecycle(lifecycle) => {
                let (label, kind) = match lifecycle {
                    Lifecycle::QueuePopped => ("Queue popped", ActivityKind::Info),
                    Lifecycle::AutoAccepted => {
                        ("Ready check auto-accepted", ActivityKind::Accepted)
                    }
                    Lifecycle::GameStarted => ("Game started", ActivityKind::Info),
                };
                self.push_activity(label, kind);
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

    /// Whether auto-accept is enabled *and* the client is connected — the condition under which
    /// the tray may say "Active". Never infer activity from the process merely running.
    pub fn monitoring(&self) -> bool {
        self.settings.auto_accept_enabled && self.connection == ConnectionStatus::Connected
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
        let mut state = AppState::new(AppSettings::default());

        state.connection = ConnectionStatus::Connected;
        assert_eq!(state.summary(), "Disabled");
        assert!(!state.monitoring());

        state.settings.auto_accept_enabled = true;
        state.connection = ConnectionStatus::Disconnected;
        assert_eq!(state.summary(), "League offline");
        assert!(!state.monitoring());

        state.connection = ConnectionStatus::Connecting;
        assert_eq!(state.summary(), "Connecting");
        assert!(!state.monitoring());

        state.connection = ConnectionStatus::Connected;
        assert_eq!(state.summary(), "Active");
        assert!(state.monitoring());
    }

    #[test]
    fn maps_service_events_and_bounds_activity() {
        let mut state = AppState::new(AppSettings::default());

        state.apply_service_event(&ServiceEvent::Connection {
            connected: true,
            message: "League Client connected.".into(),
        });
        assert_eq!(state.connection, ConnectionStatus::Connected);

        state.apply_service_event(&ServiceEvent::ReadyCheck {
            status: LeagueStatus::Ready,
            message: "Ready check detected; accepting…".into(),
        });
        assert_eq!(state.ready_check, ReadyCheckStatus::Ready);

        state.apply_service_event(&ServiceEvent::Lifecycle(Lifecycle::QueuePopped));
        assert_eq!(state.activity[0].label, "Queue popped");

        for _ in 0..20 {
            state.apply_service_event(&ServiceEvent::Lifecycle(Lifecycle::GameStarted));
        }
        assert_eq!(state.activity.len(), MAX_ACTIVITY);
    }
}
