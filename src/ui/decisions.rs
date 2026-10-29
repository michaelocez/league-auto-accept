//! GPUI-free decision logic behind the view's event handlers.
//!
//! No GPUI, no entities and no borrows: each function maps the state and the outcome it is given to
//! the values the view should adopt. That is what makes the handler decisions testable without a
//! window — including the webhook path whose original shape held a `RefCell` borrow across a `match`.

use std::time::{Duration, Instant};

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

/// How soon after the tray popup hid a tray click still counts as the *same* click.
///
/// A click on the tray icon deactivates the popup, and the popup dismisses itself the moment it loses
/// OS focus (`ui::tray_popup`). So by the time the click's own `OpenPopup` intent is handled, the
/// popup has usually hidden itself already and a naive handler would re-open what the user meant to
/// close — the reported defect. Nothing on the intent itself says which click produced it, so the
/// only evidence is how recently the popup hid: a tray click reaches the handler within a few
/// milliseconds of the deactivation it caused, while a second, deliberate click on the tray icon
/// cannot beat the cooldown, which is far longer than any hand can move. The window is therefore
/// small on purpose: it is the race's length, not a user-facing interval.
pub const TRAY_POPUP_REOPEN_GUARD: Duration = Duration::from_millis(250);

/// What a tray icon activation does to the popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayPopupAction {
    /// No popup is showing: open it.
    Show,
    /// The popup is showing, or the click that arrived is the one that just dismissed it: leave it
    /// hidden, so the second click on the icon closes the popup instead of re-opening it.
    KeepHidden,
}

/// Whether a tray activation opens the popup or leaves it hidden.
///
/// `hidden_at` is when the popup last hid, and the popup reports every hide the same way — its own
/// ✕, Escape, a click elsewhere, or the deactivation of a click on the icon itself — because from
/// here those are the same event. `is_open` is checked first: a popup that is still up at this point
/// was closed neither by this click's deactivation nor by anything else, so this click closes it.
///
/// Pure on purpose: the whole decision is here so it can be tested against literal instants instead
/// of a real tray click and a real window.
pub fn tray_popup_action(
    is_open: bool,
    hidden_at: Option<Instant>,
    now: Instant,
    guard: Duration,
) -> TrayPopupAction {
    if is_open {
        return TrayPopupAction::KeepHidden;
    }
    match hidden_at {
        // Never hidden: the popup is simply not up yet, so this is a first click.
        None => TrayPopupAction::Show,
        Some(hidden_at) => {
            // `now` cannot precede a recorded hide; saturating keeps a clock that steps backwards
            // from turning the race's own click into an "old" hide that re-opens the popup.
            let since_hidden = now.saturating_duration_since(hidden_at);
            if since_hidden < guard {
                TrayPopupAction::KeepHidden
            } else {
                TrayPopupAction::Show
            }
        }
    }
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

    /// A tray activation with no popup up is the first click: it opens.
    #[test]
    fn a_tray_click_with_no_popup_showing_opens_it() {
        let now = Instant::now();
        assert_eq!(
            tray_popup_action(false, None, now, TRAY_POPUP_REOPEN_GUARD),
            TrayPopupAction::Show
        );
    }

    /// A popup that is still up when the activation is handled is the second click: it closes.
    #[test]
    fn a_tray_click_while_the_popup_is_open_closes_it() {
        let now = Instant::now();
        // The popup has been up for a while; the hide stamp from an earlier dismissal is old and
        // irrelevant, which is the point: the open popup wins over any stamp.
        assert_eq!(
            tray_popup_action(
                true,
                Some(now - TRAY_POPUP_REOPEN_GUARD * 10),
                now,
                TRAY_POPUP_REOPEN_GUARD
            ),
            TrayPopupAction::KeepHidden
        );
        assert_eq!(
            tray_popup_action(true, None, now, TRAY_POPUP_REOPEN_GUARD),
            TrayPopupAction::KeepHidden
        );
    }

    /// The defect itself: the click deactivates the popup, the popup hides on focus loss, and the
    /// click's own activation arrives a moment later. It must leave the popup closed.
    #[test]
    fn a_click_arriving_just_after_a_focus_loss_hide_does_not_reopen_the_popup() {
        let now = Instant::now();
        for elapsed in [
            Duration::ZERO,
            Duration::from_millis(1),
            Duration::from_millis(249),
        ] {
            assert_eq!(
                tray_popup_action(false, Some(now - elapsed), now, TRAY_POPUP_REOPEN_GUARD),
                TrayPopupAction::KeepHidden,
                "a hide {elapsed:?} old still belongs to the click being handled"
            );
        }
    }

    /// A hide older than the guard is somebody else's: the popup was dismissed by an earlier click,
    /// so this click is genuinely the next one and opens it again.
    #[test]
    fn a_later_tray_click_after_a_hide_opens_the_popup_again() {
        let now = Instant::now();
        for elapsed in [
            TRAY_POPUP_REOPEN_GUARD,
            TRAY_POPUP_REOPEN_GUARD + Duration::from_millis(1),
            Duration::from_secs(2),
        ] {
            assert_eq!(
                tray_popup_action(false, Some(now - elapsed), now, TRAY_POPUP_REOPEN_GUARD),
                TrayPopupAction::Show,
                "a hide {elapsed:?} old is no longer this click's"
            );
        }
    }

    /// The boundary in both directions, so the comparison cannot drift from `<` to `<=` unnoticed.
    #[test]
    fn the_reopen_guard_boundary_is_exclusive() {
        let now = Instant::now();
        let just_inside = TRAY_POPUP_REOPEN_GUARD - Duration::from_millis(1);
        let just_outside = TRAY_POPUP_REOPEN_GUARD + Duration::from_millis(1);

        assert_eq!(
            tray_popup_action(false, Some(now - just_inside), now, TRAY_POPUP_REOPEN_GUARD),
            TrayPopupAction::KeepHidden
        );
        assert_eq!(
            tray_popup_action(
                false,
                Some(now - TRAY_POPUP_REOPEN_GUARD),
                now,
                TRAY_POPUP_REOPEN_GUARD
            ),
            TrayPopupAction::Show,
            "exactly one guard later the popup opens"
        );
        assert_eq!(
            tray_popup_action(
                false,
                Some(now - just_outside),
                now,
                TRAY_POPUP_REOPEN_GUARD
            ),
            TrayPopupAction::Show
        );
    }

    /// A stamp from the future (a clock that stepped backwards, or a hide recorded on another
    /// timeline) must not be read as an ancient hide that re-opens the popup under the user's click.
    #[test]
    fn a_hide_stamp_from_the_future_still_keeps_the_popup_closed() {
        let now = Instant::now();
        assert_eq!(
            tray_popup_action(
                false,
                Some(now + Duration::from_secs(1)),
                now,
                TRAY_POPUP_REOPEN_GUARD
            ),
            TrayPopupAction::KeepHidden
        );
    }

    fn refusal(action: &str, error: StoreError) -> String {
        match webhook_change(action, Err(error)) {
            WebhookChange::Refused(message) => message,
            WebhookChange::Applied(_) => panic!("a refused change must not produce a status"),
        }
    }
}
