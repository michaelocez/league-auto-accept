//! Pure decision logic behind the view's event handlers.
//!
//! No GPUI, no entities and no borrows: each function maps the state and the outcome it is given to
//! the values the view should adopt. That is what makes the handler decisions testable without a
//! window — including the webhook path whose original shape held a `RefCell` borrow across a `match`
//! and closed the whole application.

use serde_json::{json, Value};

use crate::config::settings::AppSettings;
use crate::config::store::{webhook_error_message, StoreError};
use crate::notifications::discord::WebhookStatus;

/// Which boolean setting a toggle row switch controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToggleSetting {
    DiscordNotifications,
    QueuePopped,
    AutoAccepted,
    GameStarted,
    MinimizeToTray,
}

/// The settings patch that flips `setting`, derived from the value the UI is showing, so the write
/// can never disagree with what the user clicked.
pub fn toggle_patch(setting: ToggleSetting, settings: &AppSettings) -> Value {
    match setting {
        ToggleSetting::DiscordNotifications => {
            json!({ "discordNotificationsEnabled": !settings.discord_notifications_enabled })
        }
        ToggleSetting::QueuePopped => {
            json!({ "notificationEvents": { "queuePopped": !settings.notification_events.queue_popped } })
        }
        ToggleSetting::AutoAccepted => {
            json!({ "notificationEvents": { "autoAccepted": !settings.notification_events.auto_accepted } })
        }
        ToggleSetting::GameStarted => {
            json!({ "notificationEvents": { "gameStarted": !settings.notification_events.game_started } })
        }
        ToggleSetting::MinimizeToTray => json!({ "minimizeToTray": !settings.minimize_to_tray }),
    }
}

/// One Auto Accept transition: the value the running service reads and the patch that persists it.
///
/// Both come from here because they must agree — deriving them separately at each call site is how
/// the tray and the window could end up describing different states.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoAcceptChange {
    pub enabled: bool,
    pub patch: Value,
}

pub fn auto_accept_change(current: bool) -> AutoAcceptChange {
    let enabled = !current;
    AutoAcceptChange {
        enabled,
        patch: json!({ "autoAcceptEnabled": enabled }),
    }
}

/// What a webhook save or removal did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebhookChange {
    /// The store accepted the change and answers with the status that is now really stored.
    Applied(WebhookStatus),
    /// The store refused; what is shown stays the webhook in use, and this is what to tell the user.
    Refused(String),
}

/// The outcome of a save or removal, as the view should act on it.
///
/// The refusal arm carries no status on purpose: a refused change must never be adopted, which is
/// what keeps the shown status, the notification sender and the settings file in agreement.
pub fn webhook_change(action: &str, result: Result<WebhookStatus, StoreError>) -> WebhookChange {
    match result {
        Ok(status) => WebhookChange::Applied(status),
        Err(error) => WebhookChange::Refused(webhook_error_message(action, &error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::merge_settings;

    /// Every toggle row must move exactly the one setting it names, or a click on one switch would
    /// silently change another.
    #[test]
    fn each_toggle_patch_flips_exactly_its_own_setting() {
        let before = AppSettings::default();

        let cases = [
            (
                ToggleSetting::DiscordNotifications,
                "discordNotificationsEnabled",
                toggle_patch(ToggleSetting::DiscordNotifications, &before),
            ),
            (
                ToggleSetting::QueuePopped,
                "queuePopped",
                toggle_patch(ToggleSetting::QueuePopped, &before),
            ),
            (
                ToggleSetting::AutoAccepted,
                "autoAccepted",
                toggle_patch(ToggleSetting::AutoAccepted, &before),
            ),
            (
                ToggleSetting::GameStarted,
                "gameStarted",
                toggle_patch(ToggleSetting::GameStarted, &before),
            ),
            (
                ToggleSetting::MinimizeToTray,
                "minimizeToTray",
                toggle_patch(ToggleSetting::MinimizeToTray, &before),
            ),
        ];

        for (setting, key, patch) in cases {
            assert!(
                patch.to_string().contains(key),
                "{setting:?} must address {key}: {patch}"
            );
            let after = merge_settings(&before, &patch);
            let changed = [
                after.schema_version != before.schema_version,
                after.auto_accept_enabled != before.auto_accept_enabled,
                after.discord_notifications_enabled != before.discord_notifications_enabled,
                after.discord_mentions != before.discord_mentions,
                after.notification_events != before.notification_events,
                after.notification_messages != before.notification_messages,
                after.minimize_to_tray != before.minimize_to_tray,
                after.theme_mode != before.theme_mode,
                after.window_backdrop != before.window_backdrop,
            ]
            .into_iter()
            .filter(|changed| *changed)
            .count();
            assert_eq!(changed, 1, "{setting:?} must flip one setting: {patch}");
        }
    }

    /// The stored value and the patch are the same decision, so they cannot drift apart.
    #[test]
    fn the_auto_accept_flag_and_its_patch_always_agree() {
        for current in [false, true] {
            let change = auto_accept_change(current);
            assert_eq!(change.enabled, !current);
            assert_eq!(change.patch, json!({ "autoAcceptEnabled": change.enabled }));
        }
    }

    #[test]
    fn an_accepted_webhook_change_adopts_exactly_the_status_it_was_given() {
        let stored = WebhookStatus {
            configured: true,
            host: Some("discord.com".to_string()),
        };
        assert_eq!(
            webhook_change("saved", Ok(stored.clone())),
            WebhookChange::Applied(stored)
        );

        // A removal is the same contract with the empty status.
        assert_eq!(
            webhook_change("removed", Ok(WebhookStatus::default())),
            WebhookChange::Applied(WebhookStatus::default())
        );
    }

    /// The shape behind the removal crash: a refused change must never hand the view a status to
    /// adopt, or the window could show — and the store could keep — a value that was never stored.
    #[test]
    fn a_refused_webhook_change_carries_no_status() {
        for error in [
            StoreError::InvalidWebhook,
            StoreError::Io(std::io::Error::other("denied")),
        ] {
            match webhook_change("saved", Err(error)) {
                WebhookChange::Applied(status) => panic!("a refusal must not adopt {status:?}"),
                WebhookChange::Refused(message) => {
                    // Both refusal shapes have to say the stored webhook was left alone.
                    assert!(
                        message.contains("left as it was")
                            || message.contains("still the one in use"),
                        "{message}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_refusal_message_matches_the_kind_of_refusal() {
        let invalid = refusal("saved", StoreError::InvalidWebhook);
        assert!(
            invalid.contains("not a valid Discord webhook URL"),
            "{invalid}"
        );
        assert!(!invalid.contains("settings-error.log"), "{invalid}");

        let refused = refusal("removed", StoreError::Io(std::io::Error::other("denied")));
        assert!(refused.contains("could not be removed"), "{refused}");
        assert!(refused.contains("settings-error.log"), "{refused}");
    }

    fn refusal(action: &str, error: StoreError) -> String {
        match webhook_change(action, Err(error)) {
            WebhookChange::Refused(message) => message,
            WebhookChange::Applied(_) => panic!("a refused change must not produce a status"),
        }
    }
}
