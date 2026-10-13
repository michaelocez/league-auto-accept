//! Settings persistence: atomic load/save with an encrypted webhook secret.
//!
//! Writes are atomic (temp file + rename), a malformed file is preserved as `settings.json.invalid`
//! before defaults replace it, and the webhook secret is stored as `{protection, value}` separate
//! from the plain settings. The secret is owned here (and in the shared handle the notification
//! sender reads); it is never part of [`AppSettings`], so it cannot reach application state, the
//! tray or the UI.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde_json::Value;

use crate::notifications::discord::{parse_discord_webhook_url, WebhookSecret, WebhookStatus};

use super::secret::{default_secret_codec, SecretCodec, StoredSecret};
use super::settings::{merge_settings, normalize_settings, AppSettings};

const SETTINGS_FILE_NAME: &str = "settings.json";

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Json(serde_json::Error),
    /// A webhook URL failed Discord validation before it could be stored.
    InvalidWebhook,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(error) => write!(f, "settings I/O error: {error}"),
            StoreError::Json(error) => write!(f, "settings JSON error: {error}"),
            StoreError::InvalidWebhook => write!(f, "the Discord webhook URL is not valid"),
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
    webhook: Arc<RwLock<WebhookSecret>>,
}

impl SettingsStore {
    /// Creates a store backed by `<directory>/settings.json` using the platform secret codec.
    pub fn with_directory(directory: &Path) -> Self {
        Self::with_webhook(
            directory,
            default_secret_codec(),
            Arc::new(RwLock::new(WebhookSecret::default())),
        )
    }

    /// Creates a store backed by `<directory>/settings.json`, sharing `webhook` with the
    /// notification sender. The store is the only writer of that handle.
    pub fn with_directory_and_webhook(
        directory: &Path,
        webhook: Arc<RwLock<WebhookSecret>>,
    ) -> Self {
        Self::with_webhook(directory, default_secret_codec(), webhook)
    }

    /// Creates a store with an explicit codec (used by tests).
    pub fn new(directory: &Path, codec: Box<dyn SecretCodec>) -> Self {
        Self::with_webhook(
            directory,
            codec,
            Arc::new(RwLock::new(WebhookSecret::default())),
        )
    }

    fn with_webhook(
        directory: &Path,
        codec: Box<dyn SecretCodec>,
        webhook: Arc<RwLock<WebhookSecret>>,
    ) -> Self {
        Self {
            path: directory.join(SETTINGS_FILE_NAME),
            codec,
            settings: AppSettings::default(),
            webhook,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads settings, creating defaults (and persisting them) when the file is absent or malformed.
    ///
    /// Recovering to defaults must never destroy what the user had: a file that cannot be parsed is
    /// moved to `settings.json.invalid` first, so a webhook, mention list or preference lost to one
    /// malformed byte stays recoverable. A file that merely cannot be *read* right now — a lock, an
    /// antivirus scan, a permission the user will fix — is left untouched, and the caller keeps
    /// running on defaults in memory until the next successful load.
    pub fn load(&mut self) -> Result<AppSettings, StoreError> {
        match self.try_load() {
            Ok(()) => Ok(self.snapshot()),
            Err(StoreError::Json(error)) => {
                log::warn!(
                    "settings file {} is not valid JSON: {error}",
                    self.path.display()
                );
                self.preserve_unreadable_file()?;
                self.settings = AppSettings::default();
                self.replace_webhook(WebhookSecret::default());
                self.save()?;
                Ok(self.snapshot())
            }
            // Nothing to replace means nothing to preserve: this is a first run.
            Err(StoreError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                self.settings = AppSettings::default();
                self.replace_webhook(WebhookSecret::default());
                self.save()?;
                Ok(self.snapshot())
            }
            Err(error) => Err(error),
        }
    }

    /// Moves an unreadable settings file aside to `settings.json.invalid`.
    ///
    /// A missing file is not corruption — the defaults are simply created — so there is nothing to
    /// preserve and no error. Any other refusal (for example a directory that cannot be written to)
    /// propagates, so the file is left alone rather than overwritten.
    fn preserve_unreadable_file(&self) -> Result<(), StoreError> {
        let preserved = self.path.with_extension("json.invalid");
        match std::fs::rename(&self.path, &preserved) {
            Ok(()) => {
                log::warn!(
                    "settings file {} could not be read; preserved as {} before restoring defaults",
                    self.path.display(),
                    preserved.display()
                );
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Applies a partial patch, persists, and returns the new settings.
    ///
    /// The patch is merged into the in-memory settings *before* the file is written, so when the
    /// write is refused — a read-only or sandboxed settings directory, for example —
    /// [`SettingsStore::snapshot`] still holds the user's intent and the caller can apply it for the
    /// session instead of dropping it silently. The webhook is not part of [`AppSettings`], so a
    /// patch can never write it.
    pub fn update(&mut self, patch: &Value) -> Result<AppSettings, StoreError> {
        self.settings = merge_settings(&self.settings, patch);
        self.save()?;
        Ok(self.snapshot())
    }

    /// Replaces the Discord webhook. The only path by which a webhook enters the store, so the URL
    /// is validated here rather than trusted from the UI.
    pub fn set_webhook(&mut self, url: &str) -> Result<(), StoreError> {
        let url = url.trim();
        if !url.is_empty() && parse_discord_webhook_url(url).is_none() {
            return Err(StoreError::InvalidWebhook);
        }
        self.replace_webhook(WebhookSecret::new(url));
        self.save()
    }

    /// Removes the stored Discord webhook.
    pub fn clear_webhook(&mut self) -> Result<(), StoreError> {
        self.replace_webhook(WebhookSecret::default());
        self.save()
    }

    /// Secret-free webhook status, suitable for application state and the UI.
    pub fn webhook_status(&self) -> WebhookStatus {
        self.read_webhook().status()
    }

    /// Returns a clone of the current settings (no internal state is leaked).
    pub fn snapshot(&self) -> AppSettings {
        self.settings.clone()
    }

    fn read_webhook(&self) -> WebhookSecret {
        match self.webhook.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn replace_webhook(&self, secret: WebhookSecret) {
        match self.webhook.write() {
            Ok(mut guard) => *guard = secret,
            Err(poisoned) => *poisoned.into_inner() = secret,
        }
    }

    fn try_load(&mut self) -> Result<(), StoreError> {
        let contents = std::fs::read_to_string(&self.path)?;
        let raw: Value = serde_json::from_str(&contents)?;
        let webhook = raw
            .get("discordWebhookUrl")
            .and_then(stored_secret)
            .map(|secret| self.codec.decode(&secret))
            .unwrap_or_default();
        self.replace_webhook(WebhookSecret::new(webhook));
        self.settings = normalize_settings(&raw, &AppSettings::default());
        Ok(())
    }

    fn save(&self) -> Result<(), StoreError> {
        let mut stored = serde_json::to_value(&self.settings)?;
        let secret = self.codec.encode(self.read_webhook().expose());
        if let Some(object) = stored.as_object_mut() {
            object.insert("discordWebhookUrl".into(), serde_json::to_value(secret)?);
        }
        let text = format!("{}\n", serde_json::to_string_pretty(&stored)?);

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, text)?;
        if let Err(error) = std::fs::rename(&temporary, &self.path) {
            // A refused atomic replace must not leave a stale temporary file behind.
            let _ = std::fs::remove_file(&temporary);
            return Err(error.into());
        }
        Ok(())
    }
}

/// The user-facing explanation when settings could not be written.
pub fn save_error_message(path: &Path, error: &StoreError) -> String {
    format!(
        "Settings could not be saved to {} — {error}. Your changes apply until the app closes.",
        path.display()
    )
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

    /// A valid Discord webhook URL: 17-digit id + 40-character token.
    const SAMPLE_URL: &str =
        "https://discord.com/api/webhooks/12345678901234567/tttttttttttttttttttttttttttttttttttttttt";

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

        store.update(&json!({ "autoAcceptEnabled": true })).unwrap();

        let reloaded = SettingsStore::new(root.path(), Box::new(FakeCodec))
            .load()
            .unwrap();
        assert!(reloaded.auto_accept_enabled);

        let stored = std::fs::read_to_string(root.path().join("settings.json")).unwrap();
        assert!(stored.contains("\"protection\": \"os\""));
    }

    #[test]
    fn a_settings_patch_cannot_write_the_webhook_secret() {
        let root = tempdir().unwrap();
        let webhook = Arc::new(RwLock::new(WebhookSecret::default()));
        let mut store =
            SettingsStore::with_webhook(root.path(), Box::new(FakeCodec), webhook.clone());
        store.load().unwrap();

        store
            .update(&json!({ "discordWebhookUrl": SAMPLE_URL }))
            .unwrap();

        assert!(!store.webhook_status().configured);
        assert!(!webhook.read().unwrap().is_configured());
        let stored = std::fs::read_to_string(root.path().join("settings.json")).unwrap();
        assert!(!stored.contains("discord.com"));
    }

    #[test]
    fn the_typed_setter_validates_persists_and_reloads_the_webhook() {
        let root = tempdir().unwrap();
        let webhook = Arc::new(RwLock::new(WebhookSecret::default()));
        let mut store =
            SettingsStore::with_webhook(root.path(), Box::new(FakeCodec), webhook.clone());
        store.load().unwrap();
        assert_eq!(store.webhook_status().label(), "Not configured");

        for invalid in [
            "not a url",
            "http://discord.com/api/webhooks/1/t",
            "https://evil.test/api/webhooks/1/t",
        ] {
            assert!(
                matches!(store.set_webhook(invalid), Err(StoreError::InvalidWebhook)),
                "{invalid:?} should be rejected"
            );
        }
        assert!(!store.webhook_status().configured);

        store.set_webhook(SAMPLE_URL).unwrap();
        assert!(store.webhook_status().configured);
        assert_eq!(store.webhook_status().host.as_deref(), Some("discord.com"));
        assert_eq!(webhook.read().unwrap().expose(), SAMPLE_URL);

        // A fresh store picks the secret up from disk, and the plaintext never appears there.
        let reloaded_handle = Arc::new(RwLock::new(WebhookSecret::default()));
        let reloaded =
            SettingsStore::with_webhook(root.path(), Box::new(FakeCodec), reloaded_handle.clone())
                .load()
                .unwrap();
        assert_eq!(reloaded_handle.read().unwrap().expose(), SAMPLE_URL);
        assert!(!format!("{reloaded:?}").contains("discord.com"));

        let stored = std::fs::read_to_string(root.path().join("settings.json")).unwrap();
        assert!(!stored.contains("discord.com"));
        assert!(stored.contains("\"protection\": \"os\""));

        let mut store = SettingsStore::with_webhook(root.path(), Box::new(FakeCodec), webhook);
        store.load().unwrap();
        store.clear_webhook().unwrap();
        assert!(!store.webhook_status().configured);
    }

    #[cfg(windows)]
    #[test]
    fn the_real_codec_stores_the_webhook_protected_on_windows() {
        let root = tempdir().unwrap();
        let mut store = SettingsStore::with_directory(root.path());
        store.load().unwrap();
        store.set_webhook(SAMPLE_URL).unwrap();
        assert_eq!(store.webhook_status().host.as_deref(), Some("discord.com"));

        let stored = std::fs::read_to_string(root.path().join("settings.json")).unwrap();
        assert!(!stored.contains(SAMPLE_URL));
        assert!(stored.contains("\"protection\": \"os\""));

        let mut reloaded = SettingsStore::with_directory(root.path());
        reloaded.load().unwrap();
        assert_eq!(
            reloaded.webhook_status().host.as_deref(),
            Some("discord.com")
        );
    }

    #[test]
    fn recovers_from_a_corrupt_settings_file() {
        let root = tempdir().unwrap();
        let path = root.path().join("settings.json");
        std::fs::write(&path, "{not-json").unwrap();

        let settings = SettingsStore::new(root.path(), Box::new(FakeCodec))
            .load()
            .unwrap();
        assert!(!settings.auto_accept_enabled);
        assert!(settings.notification_events.auto_accepted);

        // The malformed file is kept instead of being overwritten...
        let preserved = root.path().join("settings.json.invalid");
        assert_eq!(std::fs::read_to_string(&preserved).unwrap(), "{not-json");
        // ...and the file in its place parses cleanly again.
        let restored = std::fs::read_to_string(&path).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&restored).is_ok());

        // Reloading a healthy store must not disturb the preserved original.
        SettingsStore::new(root.path(), Box::new(FakeCodec))
            .load()
            .unwrap();
        assert_eq!(std::fs::read_to_string(&preserved).unwrap(), "{not-json");
    }

    /// A read failure is not corruption: the file may be locked by another process or an antivirus
    /// scan, so it stays exactly as it is and the caller runs on defaults in memory.
    #[test]
    fn leaves_an_unreadable_file_untouched() {
        let root = tempdir().unwrap();
        let path = root.path().join("settings.json");
        std::fs::create_dir(&path).unwrap();

        let error = SettingsStore::new(root.path(), Box::new(FakeCodec))
            .load()
            .expect_err("a directory cannot be read as settings");
        assert!(matches!(error, StoreError::Io(_)));
        assert!(path.is_dir());
        assert!(!root.path().join("settings.json.invalid").exists());
    }

    #[test]
    fn creates_the_directory_and_defaults_when_nothing_exists_yet() {
        let root = tempdir().unwrap();
        let directory = root.path().join("League Auto Accept");
        assert!(!directory.exists());

        let mut store = SettingsStore::new(&directory, Box::new(FakeCodec));
        let defaults = store.load().unwrap();

        assert!(defaults.minimize_to_tray);
        assert!(directory.join("settings.json").exists());
        assert!(!directory.join("settings.json.invalid").exists());
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

    #[test]
    fn persists_theme_and_backdrop_choices() {
        use crate::config::settings::{BackdropMode, ThemeMode};

        let root = tempdir().unwrap();
        let mut store = SettingsStore::new(root.path(), Box::new(FakeCodec));
        store.load().unwrap();
        store
            .update(&json!({ "themeMode": "light", "windowBackdrop": "mica" }))
            .unwrap();

        let reloaded = SettingsStore::new(root.path(), Box::new(FakeCodec))
            .load()
            .unwrap();
        assert_eq!(reloaded.theme_mode, ThemeMode::Light);
        assert_eq!(reloaded.window_backdrop, BackdropMode::Mica);
    }

    /// Windows refuses to replace a read-only file, which is the same refusal a sandboxed or
    /// locked-down settings directory produces.
    #[cfg(windows)]
    #[test]
    fn keeps_the_change_in_memory_when_the_write_is_refused() {
        use crate::config::settings::ThemeMode;

        let root = tempdir().unwrap();
        let mut store = SettingsStore::new(root.path(), Box::new(FakeCodec));
        store.load().unwrap();

        let path = root.path().join(SETTINGS_FILE_NAME);
        let original = std::fs::metadata(&path).unwrap().permissions();
        let mut read_only = original.clone();
        read_only.set_readonly(true);
        std::fs::set_permissions(&path, read_only).unwrap();

        let error = store
            .update(&json!({ "autoAcceptEnabled": true }))
            .expect_err("a read-only settings file cannot be replaced");
        assert!(matches!(error, StoreError::Io(_)));
        // The change survives in memory even though it could not be written.
        assert!(store.snapshot().auto_accept_enabled);
        // A refused write leaves no temporary file behind.
        assert!(!root.path().join("settings.json.tmp").exists());

        std::fs::set_permissions(&path, original).unwrap();

        store
            .update(&json!({ "themeMode": "light" }))
            .expect("the file can be written again");
        let reloaded = SettingsStore::new(root.path(), Box::new(FakeCodec))
            .load()
            .unwrap();
        assert!(reloaded.auto_accept_enabled);
        assert_eq!(reloaded.theme_mode, ThemeMode::Light);
    }
}
