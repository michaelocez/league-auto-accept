//! GPUI-free decision logic behind the view's event handlers.
//!
//! No GPUI, no entities and no borrows: each function maps the state and the outcome it is given to
//! the values the view should adopt. That is what makes the handler decisions testable without a
//! window — including the webhook path whose original shape held a `RefCell` borrow across a `match`.

use serde_json::{json, Value};

use crate::app::service::NotificationCommand;
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

/// What the Test Webhook button does with the result of queueing its request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebhookTestStart {
    /// The request is queued; the button may show a test in flight.
    Queued,
    /// Nothing is listening any more, so the button must not show a test in flight that can never
    /// finish.
    Unavailable(String),
}

/// The decision behind the Test Webhook button.
///
/// The sender reports whether a worker is still listening, so a dead notification channel becomes a
/// message the user can act on instead of a button stuck on "Sending…".
pub fn webhook_test_start(
    result: Result<(), async_channel::TrySendError<NotificationCommand>>,
) -> WebhookTestStart {
    match result {
        Ok(()) => WebhookTestStart::Queued,
        Err(_) => WebhookTestStart::Unavailable(
            "The background service is not running, so a test webhook cannot be sent. Restart \
             League Auto Accept to reconnect."
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::merge_settings;

    /// Every toggle row must read the setting it names. The event flags start with mixed values on
    /// purpose: with all three `false`, as they are by default, an arm reading the wrong field would
    /// still produce the right patch and the test would pass.
    #[test]
    fn each_toggle_patch_flips_only_the_field_it_names() {
        let before = AppSettings {
            notification_events: crate::config::settings::NotificationEvents {
                queue_popped: true,
                auto_accepted: false,
                game_started: true,
            },
            ..AppSettings::default()
        };

        assert_eq!(
            toggle_patch(ToggleSetting::DiscordNotifications, &before),
            json!({ "discordNotificationsEnabled": true })
        );
        assert_eq!(
            toggle_patch(ToggleSetting::QueuePopped, &before),
            json!({ "notificationEvents": { "queuePopped": false } })
        );
        assert_eq!(
            toggle_patch(ToggleSetting::AutoAccepted, &before),
            json!({ "notificationEvents": { "autoAccepted": true } })
        );
        assert_eq!(
            toggle_patch(ToggleSetting::GameStarted, &before),
            json!({ "notificationEvents": { "gameStarted": false } })
        );
        assert_eq!(
            toggle_patch(ToggleSetting::MinimizeToTray, &before),
            json!({ "minimizeToTray": false })
        );

        // The patch has to reach the setting, not just look right: a nested row must flip its own
        // field and leave its siblings as they were.
        let after = merge_settings(&before, &toggle_patch(ToggleSetting::AutoAccepted, &before));
        assert!(after.notification_events.auto_accepted);
        assert_eq!(
            after.notification_events.queue_popped,
            before.notification_events.queue_popped
        );
        assert_eq!(
            after.notification_events.game_started,
            before.notification_events.game_started
        );
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

    /// A refused change must never hand the view a status to adopt: showing or keeping a value the
    /// store refused is what the crashing shape used to do.
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

    /// The refusal text comes from the shared store helper, with the action the view was performing;
    /// the wording itself is asserted where it lives, in `config::store`.
    #[test]
    fn the_refusal_text_is_the_shared_helpers_text_for_the_action() {
        let invalid = refusal("saved", StoreError::InvalidWebhook);
        assert!(!invalid.contains("settings-error.log"), "{invalid}");

        let refused = refusal("removed", StoreError::Io(std::io::Error::other("denied")));
        assert!(refused.contains("could not be removed"), "{refused}");
    }

    /// A dead notification channel must not leave the Test Webhook button on "Sending…" forever.
    #[test]
    fn a_test_webhook_with_no_worker_behind_it_is_reported_instead_of_queued() {
        let (tx, rx) = async_channel::unbounded::<NotificationCommand>();
        drop(rx);

        match webhook_test_start(tx.try_send(NotificationCommand::TestWebhook)) {
            WebhookTestStart::Unavailable(message) => {
                assert!(message.contains("not running"), "{message}");
            }
            WebhookTestStart::Queued => panic!("nothing is listening, so nothing can be queued"),
        }
    }

    #[test]
    fn a_test_webhook_with_a_live_channel_is_queued() {
        let (tx, rx) = async_channel::unbounded::<NotificationCommand>();

        assert_eq!(
            webhook_test_start(tx.try_send(NotificationCommand::TestWebhook)),
            WebhookTestStart::Queued
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(NotificationCommand::TestWebhook)
        ));
    }

    fn refusal(action: &str, error: StoreError) -> String {
        match webhook_change(action, Err(error)) {
            WebhookChange::Refused(message) => message,
            WebhookChange::Applied(_) => panic!("a refused change must not produce a status"),
        }
    }
}
