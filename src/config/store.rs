//! Settings persistence: atomic load/save for the typed settings and their secret.
//!
//! Writes are atomic (temp file + rename), a malformed file is preserved as `settings.json.invalid`
//! before defaults replace it, and a refused write is explained through a bounded diagnostics log.
//! The webhook's own persistence lives in the [`webhook`] child module — a child so the store's
//! fields stay private — and the Windows token query the diagnostics use lives in
//! [`crate::platform::integrity`].

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde_json::Value;

use crate::notifications::discord::WebhookSecret;
use crate::platform::integrity::process_integrity_level;

use super::secret::{default_secret_codec, SecretCodec, SecretError};
use super::settings::{merge_settings, normalize_settings, AppSettings};

mod webhook;

use webhook::stored_secret;
pub use webhook::webhook_error_message;

const SETTINGS_FILE_NAME: &str = "settings.json";

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Json(serde_json::Error),
    /// A webhook URL failed Discord validation before it could be stored.
    InvalidWebhook,
    /// The webhook secret could not be protected for storage, so nothing was written.
    Secret(SecretError),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(error) => write!(f, "settings I/O error: {error}"),
            StoreError::Json(error) => write!(f, "settings JSON error: {error}"),
            StoreError::InvalidWebhook => write!(f, "the Discord webhook URL is not valid"),
            StoreError::Secret(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<SecretError> for StoreError {
    fn from(error: SecretError) -> Self {
        StoreError::Secret(error)
    }
}

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
        self.replace_webhook(WebhookSecret::new(webhook));
        self.settings = normalize_settings(&raw, &AppSettings::default());
        Ok(())
    }

    fn save(&self) -> Result<(), StoreError> {
        self.save_with(&self.read_webhook())
    }

    /// Writes the settings with `secret` as the stored webhook, without adopting it in memory.
    ///
    /// Making the secret a parameter is what lets a webhook change be written first and applied
    /// second: `save` passes the secret already in the handle, and the setters pass the candidate.
    fn save_with(&self, secret: &WebhookSecret) -> Result<(), StoreError> {
        let mut stored = serde_json::to_value(&self.settings)?;
        // A value the OS will not protect is refused here, before anything is written, so the
        // persisted secret and the in-memory handle both keep the value they already had.
        let encoded = self.codec.encode(secret.expose())?;
        if let Some(object) = stored.as_object_mut() {
            object.insert("discordWebhookUrl".into(), serde_json::to_value(encoded)?);
        }
        let text = format!("{}\n", serde_json::to_string_pretty(&stored)?);

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        if let Err(error) = std::fs::write(&temporary, &text) {
            // The refusal can happen here, before the replacement is ever attempted, in which case
            // nothing below this point runs — so it is recorded and reported from here.
            record_write_failure(&self.path, &error);
            let step =
                contextual_io_error("writing the temporary settings file", &temporary, &error);
            return Err(step.into());
        }

        // A momentary lock — a scanner reading the file, another handle not yet closed — is worth
        // waiting out, because the alternative is a setting the user watched disappear.
        let mut refusal = None;
        for _ in 0..REPLACE_ATTEMPTS {
            match std::fs::rename(&temporary, &self.path) {
                Ok(()) => {
                    // A backup written by an earlier in-place save is stale once the atomic
                    // replacement works again, and nothing ever reads it back.
                    let _ = std::fs::remove_file(self.path.with_extension("json.bak"));
                    return Ok(());
                }
                Err(error) => {
                    refusal = Some(error);
                    std::thread::sleep(REPLACE_RETRY_DELAY);
                }
            }
        }
        let refusal = refusal.unwrap_or_else(|| std::io::Error::other("not replaced"));

        // Being able to open the file itself proves the refusal was about *replacing* it (something
        // holds it without delete sharing) rather than about writing to it, so the same text is
        // written in place. A copy is kept first: an interrupted in-place write is not atomic.
        if settings_file_accepts_a_write(&self.path) {
            let backup = self.path.with_extension("json.bak");
            if std::fs::copy(&self.path, &backup).is_ok()
                && std::fs::write(&self.path, &text).is_ok()
            {
                let _ = std::fs::remove_file(&temporary);
                return Ok(());
            }
        }

        // A refused replace must not leave a stale temporary file behind.
        let _ = std::fs::remove_file(&temporary);
        record_write_failure(&self.path, &refusal);
        let step = contextual_io_error("replacing the settings file", &self.path, &refusal);
        Err(step.into())
    }
}

/// How many times the atomic replacement is retried before the write is called refused. Short,
/// because the save runs on the UI thread.
const REPLACE_ATTEMPTS: u32 = 3;
/// The pause between those attempts.
const REPLACE_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(20);
/// The file a refused save records its reason in.
const WRITE_ERROR_LOG: &str = "settings-error.log";
/// The write-failure log is a diagnostic aid rather than a history: past this size the live file is
/// rotated, so a refusal that persists cannot grow it without bound.
const WRITE_ERROR_LOG_MAX_BYTES: u64 = 64 * 1024;

/// True when the settings file itself accepts a write, which separates "this file cannot be
/// replaced" from "this file cannot be written at all".
fn settings_file_accepts_a_write(path: &Path) -> bool {
    std::fs::OpenOptions::new().write(true).open(path).is_ok()
}

/// Keeps the write-failure log bounded by rotating an oversized one before the next append.
///
/// Only refusals ever reach this log, so a file past the cap is old news: one rotation is kept as
/// `settings-error.log.1`, replacing any earlier one, and the live file starts again from empty.
fn rotate_diagnostics_log(path: &Path) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if metadata.len() <= WRITE_ERROR_LOG_MAX_BYTES {
        return;
    }
    let previous = path.with_extension("log.1");
    let _ = std::fs::remove_file(&previous);
    let _ = std::fs::rename(path, &previous);
}

/// Records why a save was refused, beside the settings file.
///
/// A refusal is nearly always specific to the machine rather than to the app — a security product, a
/// handle held by another program, a restricted token — and a one-line banner cannot tell those
/// apart, so the facts are written down where they can be read afterwards.
fn record_write_failure(path: &Path, error: &std::io::Error) {
    use std::io::Write as _;

    let directory = path.parent().unwrap_or_else(|| Path::new("."));

    // A file that can be created in the directory but does not accept a write is the shape of a lock
    // or a scanner; one that cannot be created at all points at the directory or the process.
    let probe_path = directory.join("settings-write-probe.tmp");
    let can_create_in_directory = std::fs::write(&probe_path, b"probe").is_ok();
    let _ = std::fs::remove_file(&probe_path);
    let can_write_in_place = settings_file_accepts_a_write(path);
    let existing = std::fs::metadata(path)
        .map(|metadata| {
            format!(
                "{} bytes, readonly={}",
                metadata.len(),
                metadata.permissions().readonly()
            )
        })
        .unwrap_or_else(|failure| format!("unreadable ({failure})"));

    let integrity = process_integrity_level()
        .map(|level| format!("{level:#x}"))
        .unwrap_or_else(|| "unknown".into());
    let line = format!(
        "settings save refused: {error} | path={} | integrity={integrity} | can create file in directory={can_create_in_directory} | can open existing file for write={can_write_in_place} | {existing}",
        path.display()
    );
    log::error!("{line}");

    // When the settings directory is the thing being refused, a log written there cannot exist —
    // which is precisely the case worth recording — so the app's own directory and the temporary
    // directory are tried as well. An unwritable `AppData` is exactly when this matters.
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|parent| parent.join(WRITE_ERROR_LOG)))
        .unwrap_or_default();
    let candidates = [
        directory.join(WRITE_ERROR_LOG),
        beside_exe,
        std::env::temp_dir().join("league-auto-accept-settings-error.log"),
    ];
    for candidate in candidates {
        if candidate.as_os_str().is_empty() {
            continue;
        }
        rotate_diagnostics_log(&candidate);
        let opened = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&candidate);
        if opened
            .map(|mut log| writeln!(log, "{line}").is_ok())
            .unwrap_or(false)
        {
            return;
        }
    }
}

/// Names the step that failed, so the banner says more than "Access is denied".
fn contextual_io_error(step: &str, path: &Path, error: &std::io::Error) -> std::io::Error {
    std::io::Error::new(
        error.kind(),
        format!("{step} ({}) failed: {error}", path.display()),
    )
}

/// A sentence naming a lowered integrity level, which is a cause of a refused write that a log file
/// may itself be unable to record.
///
/// A process started by a sandboxed or otherwise lowered parent inherits that level, and so does one
/// started from an executable that carries a low-integrity label — which is what happens when an
/// executable is built inside a sandboxed folder. Windows then refuses every write the user could
/// normally make — anywhere, not just to the settings — while still letting the process read and run,
/// so the banner is the reliable place to say it.
fn restricted_launch_note() -> Option<&'static str> {
    match process_integrity_level() {
        Some(level) if level < 0x2000 => Some(
            " This process is also running at a lower integrity level than your normal programs, so \
             Windows refuses these writes everywhere in your profile: either it was started by \
             something that hands out restricted rights, or the executable itself inherited a \
             low-integrity label from the folder it was built in. Copy the executable into an \
             ordinary folder, or rebuild it in one, and start it normally.",
        ),
        _ => None,
    }
}

/// The user-facing explanation when settings could not be written.
pub fn save_error_message(path: &Path, error: &StoreError) -> String {
    // A refused protection fails before any write is attempted, so this branch must not send the
    // user to a log that was never written. The webhook is the part that could not be stored.
    if matches!(error, StoreError::Secret(_)) {
        return format!(
            "Settings could not be saved to {} — {error}. Your changes apply until the app closes, \
             and the Discord webhook already stored is still the one in use.",
            path.display()
        );
    }
    let note = restricted_launch_note().unwrap_or("");
    format!(
        "Settings could not be saved to {} — {error}. Your changes apply until the app closes, and \
         it tried to record why in settings-error.log: beside the settings file, or beside the app.{note}",
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

/// Fixtures shared by this module's tests and the webhook module's.
#[cfg(test)]
pub(super) mod test_support {
    use base64::Engine as _;

    use crate::config::secret::{SecretCodec, SecretError, StoredSecret};

    /// A valid Discord webhook URL: 17-digit id + 40-character token.
    pub const SAMPLE_URL: &str =
        "https://discord.com/api/webhooks/12345678901234567/tttttttttttttttttttttttttttttttttttttttt";

    /// A second valid webhook, used to prove a refused change leaves the *first* one in place.
    pub const OTHER_URL: &str =
        "https://discord.com/api/webhooks/22345678901234567/uuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuu";

    /// Deterministic codec that base64-wraps the secret so the plaintext never appears in the file.
    pub struct FakeCodec;

    impl SecretCodec for FakeCodec {
        fn encode(&self, value: &str) -> Result<StoredSecret, SecretError> {
            Ok(StoredSecret {
                protection: "os".into(),
                value: base64::engine::general_purpose::STANDARD
                    .encode(format!("protected:{value}")),
            })
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

    /// A codec that refuses to protect a real value, standing in for a failed `CryptProtectData`.
    /// An empty webhook is not a secret, so it still persists — matching the real codec.
    pub struct RefusingCodec;

    impl SecretCodec for RefusingCodec {
        fn encode(&self, value: &str) -> Result<StoredSecret, SecretError> {
            if value.is_empty() {
                return Ok(StoredSecret {
                    protection: "os".into(),
                    value: String::new(),
                });
            }
            Err(SecretError)
        }

        fn decode(&self, secret: &StoredSecret) -> String {
            secret.value.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::FakeCodec;
    use super::*;
    use crate::config::secret::SecretError;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn a_refused_replace_is_reported_and_leaves_no_temporary_behind() {
        let root = tempdir().unwrap();
        // A directory where the settings file belongs cannot be replaced by a file.
        std::fs::create_dir(root.path().join(SETTINGS_FILE_NAME)).unwrap();
        let store = SettingsStore::new(root.path(), Box::new(FakeCodec));

        let error = store.save().expect_err("the replacement cannot succeed");

        assert!(matches!(error, StoreError::Io(_)));
        assert!(!root.path().join("settings.json.tmp").exists());
        assert!(!root.path().join("settings.json.bak").exists());
    }

    #[test]
    fn a_refused_replace_records_what_was_attempted() {
        let root = tempdir().unwrap();
        std::fs::create_dir(root.path().join(SETTINGS_FILE_NAME)).unwrap();
        let store = SettingsStore::new(root.path(), Box::new(FakeCodec));

        let _ = store.save().expect_err("the replacement cannot succeed");

        let log = std::fs::read_to_string(root.path().join("settings-error.log")).unwrap();
        assert!(log.contains("settings save refused"));
        assert!(log.contains("path="));
        assert!(log.contains("can create file in directory=true"));
        assert!(log.contains("can open existing file for write=false"));
        assert!(!root.path().join("settings-write-probe.tmp").exists());
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

    /// The write-failure log is a diagnostic aid, not a history: it must not grow without bound on a
    /// machine where every save is refused.
    #[test]
    fn an_oversized_write_failure_log_is_rotated_instead_of_growing_without_bound() {
        let root = tempdir().unwrap();
        // The settings path is a directory, so the save cannot succeed and lands in the log.
        std::fs::create_dir(root.path().join(SETTINGS_FILE_NAME)).unwrap();
        let store = SettingsStore::new(root.path(), Box::new(FakeCodec));

        let log = root.path().join(WRITE_ERROR_LOG);
        let old = vec![b'x'; WRITE_ERROR_LOG_MAX_BYTES as usize + 1];
        std::fs::write(&log, &old).unwrap();

        let _ = store.save().expect_err("a directory cannot be replaced");

        let rotated = root.path().join("settings-error.log.1");
        assert_eq!(
            std::fs::read(&rotated).unwrap(),
            old,
            "the old log is kept as one rotation"
        );
        let live = std::fs::read_to_string(&log).unwrap();
        assert!(live.contains("settings save refused"));
        assert!(
            live.len() < 4096,
            "the live log restarts instead of growing"
        );

        // A refusal against a small live log appends rather than rotating again.
        let _ = store.save();
        assert_eq!(
            std::fs::read_to_string(&log)
                .unwrap()
                .matches("settings save refused")
                .count(),
            2
        );
    }

    /// A backup is written only because the in-place write is not atomic, so it is not reused and
    /// must not linger once replacement works again.
    #[test]
    fn a_successful_replacement_clears_a_backup_left_by_an_earlier_in_place_save() {
        let root = tempdir().unwrap();
        let mut store = SettingsStore::new(root.path(), Box::new(FakeCodec));
        store.load().unwrap();

        let backup = root.path().join("settings.json.bak");
        std::fs::write(&backup, "stale copy").unwrap();
        store
            .update(&json!({ "autoAcceptEnabled": true }))
            .expect("the settings file can be written");

        assert!(
            !backup.exists(),
            "the backup is not reused and does not outlive the successful replacement"
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

    /// A refused protection fails before any write is attempted, so the ordinary settings banner must
    /// not promise a diagnostics log that was never written either.
    #[test]
    fn the_settings_error_message_does_not_promise_a_log_when_nothing_was_written() {
        let message =
            save_error_message(Path::new("settings.json"), &StoreError::Secret(SecretError));
        assert!(message.contains("could not be saved"), "{message}");
        assert!(message.contains("still the one in use"), "{message}");
        assert!(!message.contains("settings-error.log"), "{message}");
    }
}
