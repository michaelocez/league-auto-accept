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
use crate::ui::theme::SurfaceTreatment;

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

/// How much of the most recent error the Diagnostics summary shows.
pub const DIAGNOSTICS_PREVIEW_CHARACTERS: usize = 180;

/// Whether the Settings page shows a Diagnostics section at all.
///
/// Only a real failure earns the section: an error-level record, or the banner that reports one
/// before the buffer saw it. "Nothing known to be wrong" is not a reason to show it, so an app that
/// has had a quiet session has no such section to ignore.
pub fn diagnostics_visible(error_records: usize, has_settings_banner: bool) -> bool {
    error_records > 0 || has_settings_banner
}

/// The one line that says how much went wrong, in the plainest terms available.
pub fn diagnostics_summary(error_records: usize) -> String {
    match error_records {
        0 => "A problem was reported this session; no error record was kept.".to_string(),
        1 => "1 error was recorded this session.".to_string(),
        count => format!("{count} errors were recorded this session."),
    }
}

/// The most recent error, cut to something that fits the card.
///
/// The record itself is bounded already; this is about the summary being readable rather than
/// complete, so the full text stays available in the copy button's report.
pub fn diagnostics_preview(text: &str) -> String {
    let mut preview: String = text.chars().take(DIAGNOSTICS_PREVIEW_CHARACTERS).collect();
    if text.chars().count() > DIAGNOSTICS_PREVIEW_CHARACTERS {
        preview.push('…');
    }
    preview
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

/// The treatment the app should actually use: the requested one where Windows accepted the backdrop,
/// and `Opaque` where it did not.
///
/// `platform::glass` reports only whether a backdrop took; this turns that report into the treatment
/// the theme should be showing, so the surfaces a window draws can never stay translucent over a
/// window the OS has made opaque. Every place that applies a treatment settles the theme through it
/// (`ui::treatment::apply`).
pub fn effective_treatment(requested: SurfaceTreatment, applied: bool) -> SurfaceTreatment {
    if applied {
        requested
    } else {
        SurfaceTreatment::Opaque
    }
}

/// What Settings says when the treatment in effect is not the one the user chose, and `None` when the
/// two agree.
///
/// The saved choice is deliberately not what this changes: Windows refused the material on this
/// machine, so this is what says so, while the user's choice stays on disk for a machine that
/// accepts it later.
pub fn backdrop_notice(saved: SurfaceTreatment, effective: SurfaceTreatment) -> Option<String> {
    if saved == effective {
        return None;
    }
    Some(format!(
        "Windows did not accept the {} window backdrop on this machine, so {} is being used.",
        saved.label(),
        effective.label()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::merge_settings;
    use crate::ui::theme::{window_background, Appearance};

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

    /// The section is the feature's whole condition, so it is pinned here rather than in the render
    /// code: no error and no banner means no section at all.
    #[test]
    fn the_diagnostics_section_appears_only_after_a_real_failure() {
        assert!(
            !diagnostics_visible(0, false),
            "a quiet session has nothing to show and must show nothing"
        );
        assert!(diagnostics_visible(1, false), "an error record is enough");
        assert!(
            diagnostics_visible(0, true),
            "a banner the buffer did not see is still a failure"
        );
        assert!(diagnostics_visible(3, true));
    }

    #[test]
    fn the_diagnostics_summary_states_how_many_errors_there_were() {
        assert!(diagnostics_summary(0).contains("no error record"));
        assert_eq!(diagnostics_summary(1), "1 error was recorded this session.");
        assert_eq!(
            diagnostics_summary(2),
            "2 errors were recorded this session."
        );
        assert_eq!(
            diagnostics_summary(17),
            "17 errors were recorded this session."
        );
    }

    /// The preview is a summary, so it is cut; the untruncated text is what the copy button carries,
    /// and a text that already fits is left exactly as it is.
    #[test]
    fn the_diagnostics_preview_is_cut_only_when_it_is_longer_than_the_card() {
        let short = "settings save refused: Access is denied.";
        assert_eq!(diagnostics_preview(short), short);

        let at_the_limit = "x".repeat(DIAGNOSTICS_PREVIEW_CHARACTERS);
        assert_eq!(diagnostics_preview(&at_the_limit), at_the_limit);

        let long = "y".repeat(DIAGNOSTICS_PREVIEW_CHARACTERS + 1);
        let preview = diagnostics_preview(&long);
        assert_eq!(preview.chars().count(), DIAGNOSTICS_PREVIEW_CHARACTERS + 1);
        assert!(preview.ends_with('…'), "{preview}");

        // A multi-byte character split by the cut is still a character, not a byte.
        let wide = "é".repeat(DIAGNOSTICS_PREVIEW_CHARACTERS + 5);
        let preview = diagnostics_preview(&wide);
        assert_eq!(preview.chars().count(), DIAGNOSTICS_PREVIEW_CHARACTERS + 1);
        assert!(preview.ends_with('…'));
    }

    /// The whole fallback decision: what Windows accepted is what the theme shows, and a refusal is
    /// always `Opaque` — never the requested treatment.
    #[test]
    fn a_requested_treatment_is_effective_only_when_the_backdrop_was_applied() {
        // Opaque needs no backdrop, so it is effective either way.
        assert_eq!(
            effective_treatment(SurfaceTreatment::Opaque, true),
            SurfaceTreatment::Opaque
        );
        assert_eq!(
            effective_treatment(SurfaceTreatment::Opaque, false),
            SurfaceTreatment::Opaque
        );

        for requested in [SurfaceTreatment::Mica, SurfaceTreatment::Acrylic] {
            assert_eq!(
                effective_treatment(requested, true),
                requested,
                "a {requested:?} backdrop that was applied must stay as requested"
            );
            assert_eq!(
                effective_treatment(requested, false),
                SurfaceTreatment::Opaque,
                "a refused {requested:?} backdrop must fall back to Opaque"
            );
        }
    }

    /// The notice exists only where the two really differ, and it names the material that was
    /// refused *and* the one in use.
    #[test]
    fn the_backdrop_notice_speaks_only_when_the_saved_choice_is_not_in_effect() {
        for saved in [SurfaceTreatment::Mica, SurfaceTreatment::Acrylic] {
            let notice = backdrop_notice(saved, effective_treatment(saved, false))
                .unwrap_or_else(|| panic!("a refused {saved:?} backdrop must be explained"));
            assert!(
                notice.contains(saved.label()),
                "the notice must name what Windows refused: {notice}"
            );
            assert!(
                notice.contains(SurfaceTreatment::Opaque.label()),
                "the notice must name what is used instead: {notice}"
            );
            assert!(notice.contains("did not accept"), "{notice}");
        }

        // A saved Opaque choice never needs an explanation — nothing was refused.
        assert_eq!(
            backdrop_notice(SurfaceTreatment::Opaque, SurfaceTreatment::Opaque),
            None
        );
        // And neither does a choice that is in effect, whether or not it needed a backdrop.
        for applied in [
            SurfaceTreatment::Opaque,
            SurfaceTreatment::Mica,
            SurfaceTreatment::Acrylic,
        ] {
            assert_eq!(
                backdrop_notice(applied, applied),
                None,
                "{applied:?} is in effect, so there is nothing to explain"
            );
        }
    }

    /// The mismatch the defect was about, as close to the window as a test can get without one: the
    /// decision picks the treatment, the theme turns that treatment into the window's background, and
    /// after a refusal the two must agree on opaque.
    #[test]
    fn a_refused_backdrop_leaves_a_window_that_is_opaque_on_both_sides() {
        let opaque = window_background(Appearance::Dark, SurfaceTreatment::Opaque);

        for requested in [SurfaceTreatment::Mica, SurfaceTreatment::Acrylic] {
            assert_ne!(
                window_background(Appearance::Dark, requested),
                opaque,
                "a {requested:?} backdrop that applies is translucent"
            );
            assert_eq!(
                window_background(Appearance::Dark, effective_treatment(requested, false)),
                opaque,
                "a refused {requested:?} backdrop must draw opaque"
            );
        }
    }

    fn refusal(action: &str, error: StoreError) -> String {
        match webhook_change(action, Err(error)) {
            WebhookChange::Refused(message) => message,
            WebhookChange::Applied(_) => panic!("a refused change must not produce a status"),
        }
    }
}
