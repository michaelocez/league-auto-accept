//! Main window: the shell and the behaviour behind its screens.
//!
//! The view renders `AppState` and emits user intents (toggles, Test Webhook). It contains no
//! networking, credentials or secret handling — those live in the app/notifications services. Each
//! screen's rendering lives in its own child module (auto_accept, notifications, settings), which is
//! also what keeps this file to the shell and the non-rendering behaviour.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use gpui::base::input::{InputEvent, InputState};
use gpui::prelude::*;
use gpui::{
    div, px, ClickEvent, Context, Entity, FocusHandle, IntoElement, Render, Rgba, Subscription,
    Task, Window,
};
use serde_json::json;

use crate::app::events::{AppEvent, EventOutcome};
use crate::app::service::{NotificationCommand, UpdateCommand};
use crate::app::state::AppState;
use crate::app::update::CheckGate;
use crate::config::settings::{AppSettings, BackdropMode};
use crate::config::store::{save_error_message, SettingsStore};
use crate::notifications::discord::MAX_TEMPLATE_LENGTH;
use crate::platform::glass;
use crate::platform::tray::TrayCommand;
use crate::platform::window as platform_window;
use crate::ui::components::{Row, Toggle};
use crate::ui::decisions::{self, ToggleSetting, WebhookChange};
use crate::ui::shell::{PageHeader, Screen, Sidebar, TitleBar};
use crate::ui::theme::{self, SurfaceTreatment, Theme};

mod auto_accept;
mod notifications;
mod settings;

/// How long a text edit settles before the settings file is written. A write re-encodes the whole
/// file, so a burst of keystrokes is coalesced into one, as in the reference implementation.
const SETTINGS_WRITE_DEBOUNCE: Duration = Duration::from_millis(300);

/// The three editable notification message templates, newest state in their input entities.
struct MessageTemplates {
    queue_popped: Entity<InputState>,
    auto_accepted: Entity<InputState>,
    game_started: Entity<InputState>,
}

pub struct Dashboard {
    pub state: AppState,
    store: Rc<RefCell<SettingsStore>>,
    enabled_flag: Arc<AtomicBool>,
    settings_shared: Arc<RwLock<AppSettings>>,
    notifications: async_channel::Sender<NotificationCommand>,
    updates: async_channel::Sender<UpdateCommand>,
    /// Makes the release check happen at most once per session, however often Settings is opened.
    update_check: CheckGate,
    webhook: Entity<InputState>,
    webhook_draft: String,
    mention_nickname: Entity<InputState>,
    mention_id: Entity<InputState>,
    mention_error: Option<String>,
    /// A pending debounced settings write; dropping it cancels the write.
    pending_write: Option<Task<()>>,
    /// Whether the diagnostics report has been copied this session, so the button can say so.
    diagnostics_copied: bool,
    templates: MessageTemplates,
    /// GPUI subscriptions are RAII — dropping one unsubscribes it — so the vec is held for the
    /// view's lifetime to keep the input handlers attached, even though nothing reads it.
    #[allow(dead_code)]
    subscriptions: Vec<Subscription>,
    screen: Screen,
    focus_handle: FocusHandle,
}

impl Dashboard {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        settings: AppSettings,
        settings_error: Option<String>,
        store: Rc<RefCell<SettingsStore>>,
        enabled_flag: Arc<AtomicBool>,
        settings_shared: Arc<RwLock<AppSettings>>,
        notifications: async_channel::Sender<NotificationCommand>,
        updates: async_channel::Sender<UpdateCommand>,
        webhook: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The saved secret is never read into the view: the field starts empty, and only a newly
        // typed URL (or a removal) can change what is stored, through the store's typed setter.
        let webhook_draft = String::new();
        let webhook_status = store.borrow().webhook_status();
        let subscription = cx.subscribe_in(
            &webhook,
            window,
            |this, state, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.webhook_draft = state.read(cx).value().to_string();
                    cx.notify();
                }
            },
        );
        let mention_nickname =
            cx.new(|cx| InputState::new(window, cx).placeholder("Nickname (optional)"));
        let mention_id = cx.new(|cx| InputState::new(window, cx).placeholder("Discord user ID"));

        let queue_popped = Self::template_input(
            &settings.notification_messages.queue_popped,
            "Sent when your queue pops",
            window,
            cx,
        );
        let auto_accepted = Self::template_input(
            &settings.notification_messages.auto_accepted,
            "Sent when a ready check is accepted",
            window,
            cx,
        );
        let game_started = Self::template_input(
            &settings.notification_messages.game_started,
            "Sent when the game begins",
            window,
            cx,
        );
        let subscriptions = vec![
            subscription,
            Self::template_subscription(&queue_popped, window, cx),
            Self::template_subscription(&auto_accepted, window, cx),
            Self::template_subscription(&game_started, window, cx),
        ];

        Self {
            state: AppState::new(settings, webhook_status).with_settings_error(settings_error),
            store,
            enabled_flag,
            settings_shared,
            notifications,
            updates,
            update_check: CheckGate::default(),
            webhook,
            webhook_draft,
            mention_nickname,
            mention_id,
            mention_error: None,
            pending_write: None,
            diagnostics_copied: false,
            templates: MessageTemplates {
                queue_popped,
                auto_accepted,
                game_started,
            },
            subscriptions,
            screen: Screen::AutoAccept,
            focus_handle: cx.focus_handle(),
        }
    }

    /// An input pre-filled with a saved message template.
    ///
    /// The stored cap is applied as a validator too, so the field cannot be made longer than the
    /// length the store keeps: an edit that would exceed [`MAX_TEMPLATE_LENGTH`] is refused rather
    /// than accepted and then quietly shortened on the next write.
    fn template_input(
        value: &str,
        placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let value = value.to_string();
        cx.new(move |cx| {
            let mut state = InputState::new(window, cx)
                .placeholder(placeholder)
                .validate(|candidate, _| candidate.chars().count() <= MAX_TEMPLATE_LENGTH);
            state.set_value(value, window, cx);
            state
        })
    }

    /// Schedules a template write as the user types; the debounce collapses a burst of keystrokes
    /// into one write. `{mentions}` is substituted at send time.
    fn template_subscription(
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe_in(
            input,
            window,
            move |this, _state, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.schedule_settings_write(cx);
                    cx.notify();
                }
            },
        )
    }

    fn persist(&mut self, patch: serde_json::Value) {
        let outcome = self.store.borrow_mut().update(&patch);
        // `update` merges the patch before writing, so the store's snapshot is the user's intent
        // even when the file could not be written. Apply it either way: a refused write must not
        // make the controls look dead, it must only be reported.
        let updated = self.store.borrow().snapshot();
        self.state.settings = updated.clone();
        if let Ok(mut guard) = self.settings_shared.write() {
            *guard = updated;
        }
        match outcome {
            Ok(_) => self.state.settings_error = None,
            Err(error) => {
                let message = {
                    let store = self.store.borrow();
                    save_error_message(store.path(), &error)
                };
                log::error!("{message}");
                self.state.settings_error = Some(message);
            }
        }
        // Closing the window follows the setting the UI is showing, not the last successful write.
        platform_window::set_hide_on_close(self.state.settings.minimize_to_tray);
    }

    /// Schedules one settings write [`SETTINGS_WRITE_DEBOUNCE`] after the last text edit. Each call
    /// replaces the previous task, and dropping a task cancels it, so a burst of keystrokes writes
    /// the file once.
    fn schedule_settings_write(&mut self, cx: &mut Context<Self>) {
        self.pending_write = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(SETTINGS_WRITE_DEBOUNCE)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.flush_settings_write(cx);
                cx.notify();
            });
        }));
    }

    /// Writes the edited message templates now instead of waiting for the debounce, so the last
    /// keystrokes before quitting are not lost. The values are read from the inputs, which always
    /// hold the newest text.
    pub fn flush_settings_write(&mut self, cx: &mut Context<Self>) {
        let patch = json!({
            "notificationMessages": {
                "queuePopped": self.templates.queue_popped.read(cx).value().to_string(),
                "autoAccepted": self.templates.auto_accepted.read(cx).value().to_string(),
                "gameStarted": self.templates.game_started.read(cx).value().to_string(),
            }
        });
        self.persist(patch);
    }

    pub fn apply_event(&mut self, event: AppEvent, cx: &mut Context<Self>) -> EventOutcome {
        // A tray Quit and a window close are the same intent, and both have to write the pending
        // template edit before the process goes away. Recognising the exit here rather than inside
        // each branch is what makes a close safe to route through the application.
        if event.requests_exit() {
            self.flush_settings_write(cx);
            return EventOutcome::Quit;
        }
        match event {
            AppEvent::Tray(TrayCommand::ShowWindow) => return EventOutcome::ShowWindow,
            // Both are handled above, so the flush cannot drift apart from the exit.
            AppEvent::Tray(TrayCommand::Quit) | AppEvent::CloseRequested => {}
            // Handled by the shell (opens the tray popup window); nothing to apply here.
            AppEvent::Tray(TrayCommand::OpenPopup) => {}
            AppEvent::Tray(TrayCommand::ToggleAutoAccept) => {
                self.toggle_auto_accept();
            }
            AppEvent::Tray(TrayCommand::ToggleDiscordNotifications) => {
                self.persist(decisions::toggle_patch(
                    ToggleSetting::DiscordNotifications,
                    &self.state.settings,
                ));
            }
            AppEvent::Service(service_event) => self.state.apply_service_event(&service_event),
            AppEvent::WebhookTest(result) => {
                self.state.webhook_test = Some(result);
                self.state.webhook_testing = false;
            }
            AppEvent::UpdateCheck(notice) => self.state.apply_update_check(notice),
        }
        EventOutcome::Continue
    }

    /// Asks for the session's single release check, the first time Settings is opened.
    ///
    /// Deliberately the only caller: nothing at startup, no timer, nothing on another screen, and
    /// the gate means a later visit does not ask again. A check that cannot even be queued (the
    /// background worker is gone) is not retried and not reported — the version line simply stays
    /// as it is, which is also what every failed check looks like.
    fn request_update_check(&mut self) {
        if self.update_check.begin() {
            let _ = self.updates.try_send(UpdateCommand::Check);
        }
    }

    /// Flips Auto Accept from the value the UI is showing. The flag the running service reads and the
    /// value that is persisted come from the same decision, so they cannot drift apart.
    fn toggle_auto_accept(&mut self) {
        let change = decisions::auto_accept_change(self.state.settings.auto_accept_enabled);
        // Relaxed is sufficient: a lone boolean the service re-reads, with nothing published with it.
        self.enabled_flag.store(change.enabled, Ordering::Relaxed);
        self.persist(change.patch);
    }

    /// Stores the typed webhook through the store's validating setter. The view never reads the
    /// stored value back — only the non-secret status changes.
    ///
    /// A refused save leaves the previously stored webhook in place (the store writes before it
    /// adopts), so the status shown stays the one that is really in use, the typed value is kept so
    /// it can be retried, and the failure is reported in the same banner as other settings failures.
    fn save_webhook(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The result is bound before it is matched: a `match` on the store's own borrow would keep
        // that borrow alive through every arm, and the arm's second read of the `RefCell`-shared
        // store would panic with `already mutably borrowed`, closing the process and the tray with
        // it. Binding ends the borrow with this statement, and the store answers with the status it
        // really stored, so no second read is needed.
        let outcome = self.store.borrow_mut().set_webhook(&self.webhook_draft);
        match decisions::webhook_change("saved", outcome) {
            WebhookChange::Applied(status) => {
                self.state.webhook_status = status;
                self.state.webhook_test = None;
                self.state.settings_error = None;
                self.webhook_draft.clear();
                let input = self.webhook.clone();
                input.update(cx, |state, cx| {
                    state.set_value(String::new(), window, cx);
                });
            }
            WebhookChange::Refused(message) => {
                // The store writes before it adopts, so a refusal leaves the stored webhook — and
                // the status already shown for it — untouched.
                log::error!("{message}");
                self.state.settings_error = Some(message);
            }
        }
        cx.notify();
    }

    /// Removes the stored webhook. The view cannot do this through a settings patch.
    fn clear_webhook(&mut self, cx: &mut Context<Self>) {
        // Bound for the same reason as `save_webhook`: the store must not be read while the borrow
        // performing the removal is still alive.
        let outcome = self.store.borrow_mut().clear_webhook();
        match decisions::webhook_change("removed", outcome) {
            WebhookChange::Applied(status) => {
                self.state.webhook_status = status;
                self.state.webhook_test = None;
                self.state.settings_error = None;
            }
            WebhookChange::Refused(message) => {
                // A refused removal leaves the stored webhook in place, so the status shown stays
                // the one that is really in use.
                log::error!("{message}");
                self.state.settings_error = Some(message);
            }
        }
        cx.notify();
    }

    /// Applies a translucent backdrop treatment, falling back to opaque when the OS has no DWM
    /// backdrop (so the window is never left transparent and unreadable).
    fn set_treatment(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        treatment: SurfaceTreatment,
    ) {
        theme::set_treatment(cx, treatment);
        // Fall back to opaque where the OS has no DWM backdrop, and persist what actually applied.
        let effective = if glass::apply(window, theme::current(), treatment) {
            treatment
        } else {
            theme::set_treatment(cx, SurfaceTreatment::Opaque);
            let _ = glass::apply(window, theme::current(), SurfaceTreatment::Opaque);
            SurfaceTreatment::Opaque
        };
        self.persist(json!({ "windowBackdrop": BackdropMode::from(effective) }));
        cx.notify();
    }
}

pub(crate) fn status_color(summary: &str, t: &Theme) -> Rgba {
    match summary {
        "Active" => t.success,
        "Connecting" => t.warning,
        "League offline" => t.danger,
        // An inactive or unknown state gets the faintest dot: "Disabled" must not read as a status.
        _ => t.text_faint,
    }
}

impl Dashboard {
    /// A titled row with a switch on the right — the shared shape of a boolean setting. Rendered
    /// without its own border so it can sit inside a grouped section surface.
    fn toggle_row(
        id: &'static str,
        title: &'static str,
        subtitle: &'static str,
        checked: bool,
        setting: ToggleSetting,
        cx: &mut Context<Self>,
    ) -> Row {
        let toggle = Toggle::new(id).checked(checked).on_change(cx.listener(
            move |this, _event: &ClickEvent, _window, cx| {
                this.persist(decisions::toggle_patch(setting, &this.state.settings));
                cx.notify();
            },
        ));

        let row = Row::new(title).control(toggle);
        if subtitle.is_empty() {
            row
        } else {
            row.description(subtitle)
        }
    }
}

impl Render for Dashboard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let summary = self.state.summary();
        let screen = self.screen;

        let body = match screen {
            Screen::AutoAccept => self.render_dashboard(cx),
            Screen::Notifications => self.render_notifications(cx),
            Screen::Settings => self.render_settings(cx),
        };

        let nav_view = cx.entity();

        let sidebar = Sidebar::new(
            screen,
            summary,
            status_color(summary, &t),
            self.state.connection.label(),
            move |target, _event, _window, app| {
                nav_view.update(app, |this, cx| {
                    this.screen = target;
                    if target == Screen::Settings {
                        this.request_update_check();
                    }
                    cx.notify();
                });
            },
        );

        let header = match screen {
            Screen::AutoAccept => PageHeader::new("Auto Accept"),
            Screen::Notifications => PageHeader::new("Notifications"),
            Screen::Settings => PageHeader::new("Settings"),
        };

        let title_bar = TitleBar::new();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(t.bg)
            .text_color(t.text)
            .child(
                div().flex().flex_1().min_h(px(0.0)).child(sidebar).child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(title_bar)
                        .child(header)
                        .child(
                            div()
                                .id("content-scroll")
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_h(px(0.0))
                                .overflow_y_scroll()
                                .px(theme::space_8())
                                .pb(theme::space_8())
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_shrink_0()
                                        .when_some(
                                            self.state.settings_error.clone(),
                                            |this, message| {
                                                this.child(
                                                    div()
                                                        .mb(theme::space_5())
                                                        .px(theme::space_4())
                                                        .py(theme::space_3())
                                                        .bg(t.surface)
                                                        .border_1()
                                                        .border_color(t.danger)
                                                        .rounded(theme::radius_md())
                                                        .text_size(theme::text_small())
                                                        .text_color(t.danger)
                                                        .child(message),
                                                )
                                            },
                                        )
                                        .child(body),
                                ),
                        ),
                ),
            )
            .track_focus(&self.focus_handle)
    }
}
