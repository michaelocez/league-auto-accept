//! Settings persistence: atomic load/save with an encrypted webhook secret.
//!
//! Ported from the reference implementation (`Electron V1/src/main/settings-store.ts`). Writes are
//! atomic (temp file + rename), corrupt files recover to defaults, and the webhook secret is stored
//! as `{protection, value}` separate from the plain settings.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::secret::{default_secret_codec, SecretCodec, StoredSecret};
use super::settings::{merge_settings, normalize_settings, AppSettings};

const SETTINGS_FILE_NAME: &str = "settings.json";

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(error) => write!(f, "settings I/O error: {error}"),
            StoreError::Json(error) => write!(f, "settings JSON error: {error}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        StoreError::Io(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        StoreError::Json(error)
    }
}

/// Per-user settings store.
pub struct SettingsStore {
    path: PathBuf,
    codec: Box<dyn SecretCodec>,
    settings: AppSettings,
}

impl SettingsStore {
    /// Creates a store backed by `<directory>/settings.json` using the platform secret codec.
    pub fn with_directory(directory: &Path) -> Self {
        Self::new(directory, default_secret_codec())
    }

    /// Creates a store with an explicit codec (used by tests).
    pub fn new(directory: &Path, codec: Box<dyn SecretCodec>) -> Self {
        Self {
            path: directory.join(SETTINGS_FILE_NAME),
            codec,
            settings: AppSettings::default(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads settings, creating defaults (and persisting them) when the file is absent or corrupt.
    pub fn load(&mut self) -> Result<AppSettings, StoreError> {
        if self.try_load().is_err() {
            self.settings = AppSettings::default();
            self.save()?;
        }
        Ok(self.snapshot())
    }

    /// Applies a partial patch, persists, and returns the new settings.
    pub fn update(&mut self, patch: &Value) -> Result<AppSettings, StoreError> {
        self.settings = merge_settings(&self.settings, patch);
        self.save()?;
        Ok(self.snapshot())
    }

    /// Returns a clone of the current settings (no internal state is leaked).
    pub fn snapshot(&self) -> AppSettings {
        self.settings.clone()
    }

    fn try_load(&mut self) -> Result<(), StoreError> {
        let contents = std::fs::read_to_string(&self.path)?;
        let raw: Value = serde_json::from_str(&contents)?;
        let webhook = raw
            .get("discordWebhookUrl")
            .and_then(stored_secret)
            .map(|secret| self.codec.decode(&secret))
            .unwrap_or_default();
        let mut candidate = raw;
        if let Some(object) = candidate.as_object_mut() {
            object.insert("discordWebhookUrl".into(), Value::String(webhook));
        }
        self.settings = normalize_settings(&candidate, &AppSettings::default());
        Ok(())
    }

    fn save(&self) -> Result<(), StoreError> {
        let mut stored = serde_json::to_value(&self.settings)?;
        let secret = self.codec.encode(&self.settings.discord_webhook_url);
        if let Some(object) = stored.as_object_mut() {
            object.insert("discordWebhookUrl".into(), serde_json::to_value(secret)?);
        }
        let text = format!("{}\n", serde_json::to_string_pretty(&stored)?);

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, &self.path)?;
        Ok(())
    }
}

/// Default per-user settings directory (`%APPDATA%\League Auto Accept`).
pub fn default_settings_directory() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("League Auto Accept")
}

fn stored_secret(value: &Value) -> Option<StoredSecret> {
    let object = value.as_object()?;
    let protection = object.get("protection")?.as_str()?;
    if protection != "os" && protection != "plain" {
        return None;
    }
    let value = object.get("value")?.as_str()?;
    Some(StoredSecret {
        protection: protection.to_string(),
        value: value.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::secret::StoredSecret;
    use base64::Engine as _;
    use serde_json::json;
    use tempfile::tempdir;

    /// Deterministic codec that base64-wraps the secret so the plaintext never appears in the file.
    struct FakeCodec;

    impl SecretCodec for FakeCodec {
        fn encode(&self, value: &str) -> StoredSecret {
            StoredSecret {
                protection: "os".into(),
                value: base64::engine::general_purpose::STANDARD
                    .encode(format!("protected:{value}")),
            }
        }

        fn decode(&self, secret: &StoredSecret) -> String {
            base64::engine::general_purpose::STANDARD
                .decode(secret.value.as_bytes())
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default()
                .trim_start_matches("protected:")
                .to_string()
        }
    }

    #[test]
    fn creates_defaults_and_persists_updates() {
        let root = tempdir().unwrap();
        let mut store = SettingsStore::new(root.path(), Box::new(FakeCodec));
        let defaults = store.load().unwrap();
        assert!(defaults.minimize_to_tray);

        store
            .update(&json!({
                "autoAcceptEnabled": true,
                "discordWebhookUrl": "https://discord.com/api/webhooks/example"
            }))
            .unwrap();

        let reloaded = SettingsStore::new(root.path(), Box::new(FakeCodec))
            .load()
            .unwrap();
        assert!(reloaded.auto_accept_enabled);
        assert_eq!(
            reloaded.discord_webhook_url,
            "https://discord.com/api/webhooks/example"
        );

        let stored = std::fs::read_to_string(root.path().join("settings.json")).unwrap();
        assert!(!stored.contains("https://discord.com"));
        assert!(stored.contains("\"protection\": \"os\""));
    }

    #[test]
    fn recovers_from_a_corrupt_settings_file() {
        let root = tempdir().unwrap();
        std::fs::write(root.path().join("settings.json"), "{not-json").unwrap();
        let settings = SettingsStore::new(root.path(), Box::new(FakeCodec))
            .load()
            .unwrap();
        assert!(!settings.auto_accept_enabled);
        assert!(settings.notification_events.auto_accepted);
    }

    #[test]
    fn migrates_and_persists_legacy_discord_user_ids() {
        let root = tempdir().unwrap();
        std::fs::write(
            root.path().join("settings.json"),
            serde_json::to_string(&json!({
                "schemaVersion": 1,
                "discordUserIds": ["12345678901234567"],
                "discordWebhookUrl": { "protection": "plain", "value": "" }
            }))
            .unwrap(),
        )
        .unwrap();

        let mut store = SettingsStore::new(root.path(), Box::new(FakeCodec));
        let settings = store.load().unwrap();
        assert_eq!(settings.discord_mentions.len(), 1);
        assert_eq!(settings.discord_mentions[0].id, "12345678901234567");

        store
            .update(&json!({
                "discordMentions": [{ "id": "12345678901234567", "nickname": "Michael" }]
            }))
            .unwrap();
        let reloaded = SettingsStore::new(root.path(), Box::new(FakeCodec))
            .load()
            .unwrap();
        assert_eq!(reloaded.discord_mentions[0].nickname, "Michael");
    }
}
