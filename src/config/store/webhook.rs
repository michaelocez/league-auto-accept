//! The Discord webhook's own persistence: the store's only secret.
//!
//! A child module of the store on purpose — the store's fields stay private while the webhook code
//! owns them — so the public paths (`SettingsStore::set_webhook`, `webhook_error_message`) do not
//! move.

use serde_json::Value;

use crate::config::secret::StoredSecret;
use crate::notifications::discord::{parse_discord_webhook_url, WebhookSecret, WebhookStatus};

use super::{restricted_launch_note, SettingsStore, StoreError};

impl SettingsStore {
    /// Replaces the Discord webhook. The only path by which a webhook enters the store, so the URL
    /// is validated here rather than trusted from the UI.
    ///
    /// The file is written before the shared handle changes, so a refused save leaves the
    /// notification sender and the UI on the secret that is actually stored instead of on one that
    /// exists only in memory and disappears at the next launch.
    ///
    /// On success the status that is really stored is returned, so a caller that shares the store
    /// can refresh what it shows from one call instead of re-entering the store while its own
    /// mutable borrow is still alive.
    pub fn set_webhook(&mut self, url: &str) -> Result<WebhookStatus, StoreError> {
        let url = url.trim();
        if !url.is_empty() && parse_discord_webhook_url(url).is_none() {
            return Err(StoreError::InvalidWebhook);
        }
        self.commit_webhook(WebhookSecret::new(url))
    }

    /// Removes the stored Discord webhook, under the same write-first rule as
    /// [`SettingsStore::set_webhook`].
    pub fn clear_webhook(&mut self) -> Result<WebhookStatus, StoreError> {
        self.commit_webhook(WebhookSecret::default())
    }

    /// Persists `secret`, adopts it only once the file has accepted it, and reports the stored
    /// status.
    ///
    /// Answering both halves of a change — what was written and what should now be shown — is what
    /// lets a caller hold a single borrow: a `RefCell`-shared store panics with
    /// `already mutably borrowed` if it is read again before the change's borrow has ended, and that
    /// panic takes the whole process down.
    fn commit_webhook(&mut self, secret: WebhookSecret) -> Result<WebhookStatus, StoreError> {
        self.save_with(&secret)?;
        self.replace_webhook(secret);
        Ok(self.webhook_status())
    }

    /// Secret-free webhook status, suitable for application state and the UI.
    pub fn webhook_status(&self) -> WebhookStatus {
        self.read_webhook().status()
    }

    /// The secret the shared handle currently holds, for the settings write path.
    pub(super) fn read_webhook(&self) -> WebhookSecret {
        match self.webhook.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Stores `secret` in the shared handle the notification sender reads.
    pub(super) fn replace_webhook(&self, secret: WebhookSecret) {
        match self.webhook.write() {
            Ok(mut guard) => *guard = secret,
            Err(poisoned) => *poisoned.into_inner() = secret,
        }
    }
}

/// Reads the persisted `{protection, value}` pair back out of the settings JSON.
pub(super) fn stored_secret(value: &Value) -> Option<StoredSecret> {
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

/// The user-facing explanation when a webhook change could not be saved.
///
/// `action` is how the attempt reads in a sentence ("saved", "removed"). The message must not imply
/// that the change took effect: when a write is refused the store keeps the secret it already had.
pub fn webhook_error_message(action: &str, error: &StoreError) -> String {
    if matches!(error, StoreError::InvalidWebhook) {
        return "That is not a valid Discord webhook URL, so the webhook was left as it was."
            .to_string();
    }
    // A refused protection fails before any write is attempted, and a degraded store refuses before
    // it even records a diagnostics log, so neither may send the user to a log that was not written.
    if matches!(error, StoreError::Secret(_) | StoreError::LoadFailed { .. }) {
        return format!(
            "The Discord webhook could not be {action} — {error}. The webhook already stored is \
             still the one in use, and nothing was written."
        );
    }
    let note = restricted_launch_note().unwrap_or("");
    format!(
        "The Discord webhook could not be {action} — {error}. The webhook already stored is still \
         the one in use, and the app tried to record why in settings-error.log.{note}"
    )
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{FakeCodec, RefusingCodec, OTHER_URL, SAMPLE_URL};
    use super::super::SETTINGS_FILE_NAME;
    use super::*;
    use crate::config::secret::SecretError;
    use serde_json::json;
    use std::sync::{Arc, RwLock};
    use tempfile::tempdir;

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

    /// A refused webhook save must not be adopted: the sender and the UI have to stay on the secret
    /// that is really stored, or the change appears to work and then vanishes at the next launch.
    #[cfg(windows)]
    #[test]
    fn a_refused_webhook_save_keeps_the_stored_secret() {
        let root = tempdir().unwrap();
        let webhook = Arc::new(RwLock::new(WebhookSecret::default()));
        let mut store =
            SettingsStore::with_webhook(root.path(), Box::new(FakeCodec), webhook.clone());
        store.load().unwrap();

        let path = root.path().join(SETTINGS_FILE_NAME);
        let original = std::fs::metadata(&path).unwrap().permissions();
        let mut read_only = original.clone();
        read_only.set_readonly(true);
        std::fs::set_permissions(&path, read_only).unwrap();

        let error = store
            .set_webhook(SAMPLE_URL)
            .expect_err("a read-only settings file cannot be replaced");
        assert!(matches!(error, StoreError::Io(_)));
        assert!(
            !store.webhook_status().configured,
            "the status must keep describing the stored webhook"
        );
        assert!(
            !webhook.read().unwrap().is_configured(),
            "the notification sender must keep the stored secret"
        );
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("discord.com"));

        std::fs::set_permissions(&path, original).unwrap();

        // Once the file accepts a write again the change is persisted and adopted together.
        store.set_webhook(SAMPLE_URL).unwrap();
        assert!(store.webhook_status().configured);
        assert!(webhook.read().unwrap().is_configured());
    }

    /// Removing a webhook is a write too, so the same rule applies in reverse.
    #[cfg(windows)]
    #[test]
    fn a_refused_webhook_removal_keeps_the_stored_secret() {
        let root = tempdir().unwrap();
        let webhook = Arc::new(RwLock::new(WebhookSecret::default()));
        let mut store =
            SettingsStore::with_webhook(root.path(), Box::new(FakeCodec), webhook.clone());
        store.load().unwrap();
        store.set_webhook(SAMPLE_URL).unwrap();

        let path = root.path().join(SETTINGS_FILE_NAME);
        let original = std::fs::metadata(&path).unwrap().permissions();
        let mut read_only = original.clone();
        read_only.set_readonly(true);
        std::fs::set_permissions(&path, read_only).unwrap();

        let error = store
            .clear_webhook()
            .expect_err("a read-only settings file cannot be replaced");
        assert!(matches!(error, StoreError::Io(_)));
        assert!(
            store.webhook_status().configured,
            "a refused removal must leave the webhook in place"
        );
        assert!(webhook.read().unwrap().is_configured());

        std::fs::set_permissions(&path, original).unwrap();
        store.clear_webhook().unwrap();
        assert!(!store.webhook_status().configured);
        assert!(!webhook.read().unwrap().is_configured());
    }

    /// The defect this guards: when the OS refused to protect the webhook, the URL used to be stored
    /// as `protection: "plain"` in the clear, silently. A refusal must instead be reported, with the
    /// webhook already stored left as the one in use and nothing written.
    #[test]
    fn a_webhook_the_codec_refuses_to_protect_is_not_stored_in_cleartext() {
        let root = tempdir().unwrap();
        let path = root.path().join(SETTINGS_FILE_NAME);
        // Seed a stored webhook directly, because this codec cannot encode one.
        std::fs::write(
            &path,
            serde_json::to_string(&json!({
                "discordWebhookUrl": { "protection": "os", "value": SAMPLE_URL }
            }))
            .unwrap(),
        )
        .unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        let webhook = Arc::new(RwLock::new(WebhookSecret::default()));
        let mut store =
            SettingsStore::with_webhook(root.path(), Box::new(RefusingCodec), webhook.clone());
        store.load().unwrap();
        assert!(
            store.webhook_status().configured,
            "the seeded webhook loads"
        );

        let error = store
            .set_webhook(OTHER_URL)
            .expect_err("a codec that refuses to protect must refuse the change");
        assert!(matches!(error, StoreError::Secret(_)));

        // Nothing was adopted and nothing was written: the stored webhook is still the first one.
        assert_eq!(webhook.read().unwrap().expose(), SAMPLE_URL);
        assert_eq!(store.webhook_status().host.as_deref(), Some("discord.com"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        // The banner explains the refusal without sending the user to a log that was never written.
        let message = webhook_error_message("saved", &error);
        assert!(message.contains("still the one in use"), "{message}");
        assert!(!message.contains("settings-error.log"), "{message}");
    }

    /// The UI refresh after a webhook change relies on the setter answering with the status that is
    /// really stored: it must agree with a fresh read, because the caller must not take a second
    /// borrow of a `RefCell`-shared store to find that out.
    #[test]
    fn a_webhook_change_reports_the_status_that_is_really_stored() {
        let root = tempdir().unwrap();
        let mut store = SettingsStore::new(root.path(), Box::new(FakeCodec));
        store.load().unwrap();

        let after_save = store.set_webhook(SAMPLE_URL).unwrap();
        assert_eq!(after_save, store.webhook_status());
        assert!(after_save.configured);
        assert_eq!(after_save.host.as_deref(), Some("discord.com"));

        let after_clear = store.clear_webhook().unwrap();
        assert_eq!(after_clear, store.webhook_status());
        assert!(!after_clear.configured);
        assert_eq!(after_clear, WebhookStatus::default());

        // A refused change reports an error and stores nothing, so there is no status to report.
        let refused = store.set_webhook("https://example.com/not-a-discord-webhook");
        assert!(matches!(refused, Err(StoreError::InvalidWebhook)));
        assert!(!store.webhook_status().configured);
    }

    /// Regression guard for the removal crash: the view holds the store behind `Rc<RefCell<_>>`, so
    /// every change must be usable with a single bound borrow. Holding the borrow across the arms —
    /// `match store.borrow_mut().clear_webhook() { Ok(()) => store.borrow()… }` — panics with
    /// `already mutably borrowed`, which closed the application and its tray.
    #[test]
    fn a_shared_store_takes_one_borrow_per_webhook_change() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let root = tempdir().unwrap();
        let cell = Rc::new(RefCell::new(SettingsStore::new(
            root.path(),
            Box::new(FakeCodec),
        )));
        cell.borrow_mut().load().unwrap();

        let outcome = cell.borrow_mut().set_webhook(SAMPLE_URL);
        let status = outcome.expect("the store accepts a valid webhook");
        assert!(status.configured);
        // The borrow ended with the statement above, so the store can be read again here — and is.
        assert_eq!(cell.borrow().webhook_status(), status);

        let outcome = cell.borrow_mut().clear_webhook();
        let status = outcome.expect("the store accepts a removal");
        assert!(!status.configured);
        assert_eq!(cell.borrow().webhook_status(), status);
        assert!(
            cell.try_borrow_mut().is_ok(),
            "no borrow may outlive a change"
        );
    }

    #[test]
    fn the_webhook_error_message_promises_nothing_that_did_not_happen() {
        let refused =
            webhook_error_message("saved", &StoreError::Io(std::io::Error::other("denied")));
        assert!(refused.contains("could not be saved"));
        assert!(refused.contains("still the one in use"));
        assert!(refused.contains("settings-error.log"));

        let removed =
            webhook_error_message("removed", &StoreError::Io(std::io::Error::other("denied")));
        assert!(removed.contains("could not be removed"));

        let invalid = webhook_error_message("saved", &StoreError::InvalidWebhook);
        assert!(invalid.contains("not a valid Discord webhook URL"));

        // A refused protection never reaches the file, so its message must not promise a log entry.
        let unprotected = webhook_error_message("saved", &StoreError::Secret(SecretError));
        assert!(unprotected.contains("could not be saved"), "{unprotected}");
        assert!(unprotected.contains("nothing was written"), "{unprotected}");
        assert!(!unprotected.contains("settings-error.log"), "{unprotected}");

        // Nor may a degraded store's refusal: it writes nothing at all, the log included.
        let degraded = webhook_error_message(
            "saved",
            &StoreError::LoadFailed {
                cause: "settings I/O error: denied".into(),
            },
        );
        assert!(degraded.contains("could not be saved"), "{degraded}");
        assert!(degraded.contains("still the one in use"), "{degraded}");
        assert!(!degraded.contains("settings-error.log"), "{degraded}");
    }
}
