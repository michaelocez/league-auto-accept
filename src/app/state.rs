use crate::config::settings::Settings;

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
    pub settings: Settings,
    pub connection: ConnectionStatus,
    pub connection_message: String,
    pub ready_check: ReadyCheckStatus,
    pub ready_check_message: String,
    /// Proof that the background async service is alive and bridging events (Spike C).
    pub backend_uptime_secs: u64,
}

impl AppState {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            connection: ConnectionStatus::Connecting,
            connection_message: "Starting League Client connection…".to_string(),
            ready_check: ReadyCheckStatus::Idle,
            ready_check_message: "Waiting for a ready check.".to_string(),
            backend_uptime_secs: 0,
        }
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
        let mut state = AppState::new(Settings::default());

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
}
