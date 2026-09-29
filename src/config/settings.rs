//! Minimal settings used by the Phase 1 spikes.
//!
//! The full schema, validation, migration and persistence are Phase 3 (AGENTS.md sec.12). This
//! exists only so the tray/close behaviour can be driven by a real setting value.

use serde::{Deserialize, Serialize};

pub const SETTINGS_FILE_NAME: &str = "settings.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// When true, closing/minimizing the window hides it to the tray instead of exiting.
    /// The tray is always present regardless of this setting.
    pub minimize_to_tray: bool,
    /// Auto Accept master switch (no accept logic yet in Phase 1).
    pub auto_accept_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            minimize_to_tray: true,
            auto_accept_enabled: false,
        }
    }
}
