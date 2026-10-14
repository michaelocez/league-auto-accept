//! Main window: dashboard + settings for Auto Accept and Discord notifications.
//!
//! The view renders `AppState` and emits user intents (toggles, Test Webhook). It contains no
//! networking, credentials or secret handling — those live in the app/notifications services.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use gpui::base::input::{Input, InputEvent, InputState};
use gpui::prelude::*;
use gpui::{
    div, px, ClickEvent, Context, Entity, FocusHandle, IntoElement, Render, Rgba, Subscription,
    Task, Window,
};
use serde_json::json;

use crate::app::events::{AppEvent, EventOutcome};
use crate::app::service::NotificationCommand;
use crate::app::state::{ActivityKind, AppState};
use crate::config::settings::{
    is_valid_discord_id, AppSettings, BackdropMode, DiscordMention, ThemeMode, MAX_DISCORD_USER_IDS,
};
use crate::config::store::{save_error_message, webhook_error_message, SettingsStore};
use crate::notifications::discord::{
    content_error, parse_discord_webhook_url, preview_notification,
};
use crate::platform::glass;
use crate::platform::tray::TrayCommand;
use crate::platform::window as platform_window;
use crate::ui::components::{
    Button, ButtonVariant, Divider, Elevation, Row, SettingsGroup, Surface, Toggle,
};
use crate::ui::shell::{PageHeader, Screen, Sidebar, TitleBar};
use crate::ui::theme::{self, Appearance, SurfaceTreatment, Theme};

/// How long a text edit settles before the settings file is written. A write re-encodes the whole
/// file, so a burst of keystrokes is coalesced into one, as in the reference implementation.
const SETTINGS_WRITE_DEBOUNCE: Duration = Duration::from_millis(300);

/// Which boolean setting a toggle row switch controls.
#[derive(Clone, Copy)]
enum ToggleSetting {
    DiscordNotifications,
    QueuePopped,
    AutoAccepted,
    GameStarted,
    MinimizeToTray,
}

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
    webhook: Entity<InputState>,
    webhook_draft: String,
    mention_nickname: Entity<InputState>,
    mention_id: Entity<InputState>,
    mention_error: Option<String>,
    /// A pending debounced settings write; dropping it cancels the write.
    pending_write: Option<Task<()>>,
    templates: MessageTemplates,
    #[allow(dead_code)]
    subscriptions: Vec<Subscription>,
    screen: Screen,
    focus_handle: FocusHandle,
}

impl Dashboard {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        settings: AppSettings,
        store: Rc<RefCell<SettingsStore>>,
        enabled_flag: Arc<AtomicBool>,
        settings_shared: Arc<RwLock<AppSettings>>,
        notifications: async_channel::Sender<NotificationCommand>,
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
            state: AppState::new(settings, webhook_status),
            store,
            enabled_flag,
            settings_shared,
            notifications,
            webhook,
            webhook_draft,
            mention_nickname,
            mention_id,
            mention_error: None,
            pending_write: None,
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
    fn template_input(
        value: &str,
        placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let value = value.to_string();
        cx.new(move |cx| {
            let mut state = InputState::new(window, cx).placeholder(placeholder);
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

    pub fn state(&self) -> &AppState {
        &self.state
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
        match event {
            AppEvent::Tray(TrayCommand::ShowWindow) => return EventOutcome::ShowWindow,
            AppEvent::Tray(TrayCommand::Quit) => {
                self.flush_settings_write(cx);
                return EventOutcome::Quit;
            }
            // Handled by the shell (opens the tray popup window); nothing to apply here.
            AppEvent::Tray(TrayCommand::OpenPopup) => {}
            AppEvent::Tray(TrayCommand::ToggleAutoAccept) => {
                let next = !self.state.settings.auto_accept_enabled;
                self.set_auto_accept(next);
            }
            AppEvent::Tray(TrayCommand::ToggleDiscordNotifications) => {
                let next = !self.state.settings.discord_notifications_enabled;
                self.persist(json!({ "discordNotificationsEnabled": next }));
            }
            AppEvent::Service(service_event) => self.state.apply_service_event(&service_event),
            AppEvent::WebhookTest(result) => {
                self.state.webhook_test = Some(result);
                self.state.webhook_testing = false;
            }
        }
        EventOutcome::Continue
    }

    fn set_auto_accept(&mut self, enabled: bool) {
        self.state.settings.auto_accept_enabled = enabled;
        self.enabled_flag.store(enabled, Ordering::Relaxed);
        self.persist(json!({ "autoAcceptEnabled": enabled }));
    }

    /// Stores the typed webhook through the store's validating setter. The view never reads the
    /// stored value back — only the non-secret status changes.
    ///
    /// A refused save leaves the previously stored webhook in place (the store writes before it
    /// adopts), so the status shown stays the one that is really in use, the typed value is kept so
    /// it can be retried, and the failure is reported in the same banner as other settings failures.
    fn save_webhook(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let outcome = self.store.borrow_mut().set_webhook(&self.webhook_draft);
        match outcome {
            Ok(()) => {
                self.state.webhook_status = self.store.borrow().webhook_status();
                self.state.webhook_test = None;
                self.state.settings_error = None;
                self.webhook_draft.clear();
                let input = self.webhook.clone();
                input.update(cx, |state, cx| {
                    state.set_value(String::new(), window, cx);
                });
            }
            Err(error) => {
                self.state.webhook_status = self.store.borrow().webhook_status();
                log::error!("failed to save the Discord webhook: {error}");
                self.state.settings_error = Some(webhook_error_message("saved", &error));
            }
        }
        cx.notify();
    }

    /// Removes the stored webhook. The view cannot do this through a settings patch.
    fn clear_webhook(&mut self, cx: &mut Context<Self>) {
        match self.store.borrow_mut().clear_webhook() {
            Ok(()) => {
                self.state.webhook_status = self.store.borrow().webhook_status();
                self.state.webhook_test = None;
                self.state.settings_error = None;
            }
            Err(error) => {
                self.state.webhook_status = self.store.borrow().webhook_status();
                log::error!("failed to clear the Discord webhook: {error}");
                self.state.settings_error = Some(webhook_error_message("removed", &error));
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
        _ => t.text_muted,
    }
}

fn activity_color(kind: ActivityKind, t: &Theme) -> Rgba {
    match kind {
        ActivityKind::Info => t.text_muted,
        ActivityKind::Accepted => t.success,
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
                match setting {
                    ToggleSetting::DiscordNotifications => {
                        let next = !this.state.settings.discord_notifications_enabled;
                        this.persist(json!({ "discordNotificationsEnabled": next }));
                    }
                    ToggleSetting::QueuePopped => {
                        let next = !this.state.settings.notification_events.queue_popped;
                        this.persist(json!({ "notificationEvents": { "queuePopped": next } }));
                    }
                    ToggleSetting::AutoAccepted => {
                        let next = !this.state.settings.notification_events.auto_accepted;
                        this.persist(json!({ "notificationEvents": { "autoAccepted": next } }));
                    }
                    ToggleSetting::GameStarted => {
                        let next = !this.state.settings.notification_events.game_started;
                        this.persist(json!({ "notificationEvents": { "gameStarted": next } }));
                    }
                    ToggleSetting::MinimizeToTray => {
                        let next = !this.state.settings.minimize_to_tray;
                        this.persist(json!({ "minimizeToTray": next }));
                    }
                }
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

    fn render_dashboard(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::of(cx);
        let auto_accept = self.state.settings.auto_accept_enabled;
        let ready_message = self.state.ready_check_message.clone();
        let summary = self.state.summary();

        let state_color = match summary {
            "Active" => t.success,
            "Connecting" => t.warning,
            "League offline" => t.danger,
            _ => t.text_faint,
        };
        let hero_detail = if auto_accept {
            ready_message
        } else {
            "Off — turn on Auto Accept to monitor ready checks.".to_string()
        };

        let hero = Surface::new().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(theme::space_6())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::space_2())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme::space_3())
                                .child(div().size(px(9.0)).rounded_full().bg(state_color))
                                .child(
                                    div()
                                        .text_size(theme::text_title())
                                        .font_weight(theme::weight_semibold())
                                        .text_color(t.text)
                                        .child(summary),
                                ),
                        )
                        .child(
                            div()
                                .text_size(theme::text_small())
                                .text_color(t.text_muted)
                                .child(hero_detail),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme::space_3())
                        .child(
                            div()
                                .text_size(theme::text_small())
                                .text_color(t.text_muted)
                                .child("Auto Accept"),
                        )
                        .child(
                            Toggle::new("toggle-auto-accept")
                                .checked(auto_accept)
                                .on_change(cx.listener(
                                    move |this, _event: &ClickEvent, _window, cx| {
                                        let next = !this.state.settings.auto_accept_enabled;
                                        this.set_auto_accept(next);
                                        cx.notify();
                                    },
                                )),
                        ),
                ),
        );

        let mut body = div().flex().flex_col().gap(theme::space_5()).child(hero);
        // Only show the timeline once there is something real to show — an empty placeholder adds
        // noise without information.
        if !self.state.activity.is_empty() {
            body = body.child(self.render_activity_timeline(&t));
        }
        body.into_any_element()
    }

    fn render_activity_timeline(&self, t: &Theme) -> gpui::AnyElement {
        let clear = Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        };
        // `last` only tags the final row, so an empty list must render nothing rather than underflow.
        let last = self.state.activity.len().saturating_sub(1);
        let items: Vec<gpui::AnyElement> = self
            .state
            .activity
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let top = if index == 0 { clear } else { t.border };
                let bottom = if index == last { clear } else { t.border };
                div()
                    .flex()
                    .gap(theme::space_3())
                    .min_h(px(32.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .w(px(10.0))
                            .flex_shrink_0()
                            .child(div().w(px(1.0)).h(px(8.0)).bg(top))
                            .child(
                                div()
                                    .size(px(8.0))
                                    .rounded_full()
                                    .bg(activity_color(item.kind, t)),
                            )
                            .child(div().w(px(1.0)).flex_1().bg(bottom)),
                    )
                    .child(
                        div()
                            .pb(theme::space_3())
                            .text_size(theme::text_body())
                            .text_color(t.text)
                            .child(item.label.clone()),
                    )
                    .into_any_element()
            })
            .collect();

        Surface::new()
            .elevation(Elevation::Base)
            .padded(false)
            .child(
                div()
                    .px(theme::space_4())
                    .py(theme::space_3())
                    .text_size(theme::text_small())
                    .font_weight(theme::weight_medium())
                    .text_color(t.text_muted)
                    .child("Recent activity"),
            )
            .child(Divider::horizontal())
            .child(
                div()
                    .px(theme::space_4())
                    .pt(theme::space_3())
                    .children(items),
            )
            .into_any_element()
    }

    fn render_notifications(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::of(cx);
        let discord_enabled = self.state.settings.discord_notifications_enabled;
        let testing = self.state.webhook_testing;
        let draft = self.webhook_draft.trim().to_string();
        let webhook_error = if draft.is_empty() {
            String::new()
        } else if parse_discord_webhook_url(&draft).is_none() {
            "Enter a valid Discord webhook URL.".to_string()
        } else {
            String::new()
        };
        let can_save = !draft.is_empty() && webhook_error.is_empty();
        let webhook_configured = self.state.webhook_status.configured;
        let (test_message, test_color) = if testing {
            ("Sending test webhook…".to_string(), t.text_muted)
        } else {
            match &self.state.webhook_test {
                Some(result) if result.ok => (result.message.clone(), t.success),
                Some(result) => (result.message.clone(), t.danger),
                None => (
                    "Uses the saved webhook URL and configured mentions.".to_string(),
                    t.text_muted,
                ),
            }
        };

        let mut webhook_row = Row::new("Webhook URL")
            .description("Encrypted at rest and read only by the notification service; this window shows whether one is saved, never the URL itself.")
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::space_2())
                    .max_w(px(440.0))
                    .child(Self::input_frame(&self.webhook, cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::space_3())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_size(theme::text_caption())
                                    .text_color(t.text_muted)
                                    .child(self.state.webhook_status.label()),
                            )
                            .child(
                                Button::new("save-webhook", "Save")
                                    .variant(ButtonVariant::Secondary)
                                    .disabled(!can_save)
                                    .on_click(cx.listener(
                                        |this, _event: &ClickEvent, window, cx| {
                                            this.save_webhook(window, cx);
                                        },
                                    )),
                            )
                            .child(
                                Button::new("clear-webhook", "Remove")
                                    .variant(ButtonVariant::Ghost)
                                    .disabled(!webhook_configured)
                                    .on_click(cx.listener(
                                        |this, _event: &ClickEvent, _window, cx| {
                                            this.clear_webhook(cx);
                                        },
                                    )),
                            ),
                    ),
            );
        if !webhook_error.is_empty() {
            webhook_row = webhook_row.child(
                div()
                    .text_size(theme::text_caption())
                    .text_color(t.danger)
                    .child(webhook_error),
            );
        }

        let test_row = div()
            .flex()
            .items_center()
            .gap(theme::space_3())
            .px(theme::space_4())
            .py(theme::space_3())
            .child(
                Button::new(
                    "test-webhook",
                    if testing {
                        "Sending…"
                    } else {
                        "Test Webhook"
                    },
                )
                .variant(ButtonVariant::Secondary)
                .disabled(testing)
                .on_click(cx.listener(|this, _event: &ClickEvent, _window, cx| {
                    if this.state.webhook_testing {
                        return;
                    }
                    this.state.webhook_test = None;
                    this.state.webhook_testing = true;
                    let _ = this
                        .notifications
                        .try_send(NotificationCommand::TestWebhook);
                    cx.notify();
                })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(theme::text_small())
                    .text_color(test_color)
                    .child(test_message),
            );

        let events = self.state.settings.notification_events.clone();
        let delivery = SettingsGroup::new()
            .child(Self::toggle_row(
                "toggle-discord",
                "Enable Discord notifications",
                "Match-found, auto-accepted and game-started alerts.",
                discord_enabled,
                ToggleSetting::DiscordNotifications,
                cx,
            ))
            .child(webhook_row)
            .child(self.render_mentions(&t, cx))
            .child(test_row);

        let event_group = SettingsGroup::new()
            .title("Events")
            .child(Self::toggle_row(
                "toggle-queue-popped",
                "Match found",
                "",
                events.queue_popped,
                ToggleSetting::QueuePopped,
                cx,
            ))
            .child(Self::toggle_row(
                "toggle-auto-accepted",
                "Match accepted",
                "",
                events.auto_accepted,
                ToggleSetting::AutoAccepted,
                cx,
            ))
            .child(Self::toggle_row(
                "toggle-game-started",
                "Game started",
                "",
                events.game_started,
                ToggleSetting::GameStarted,
                cx,
            ));

        let mentions = self.state.settings.discord_mentions.clone();
        let message_group = SettingsGroup::new()
            .title("Messages")
            .child(
                div()
                    .px(theme::space_4())
                    .pb(theme::space_3())
                    .text_size(theme::text_caption())
                    .text_color(t.text_muted)
                    .child("Use {mentions} where the mentioned people should appear."),
            )
            .child(self.message_row("Match found", &self.templates.queue_popped, &mentions, cx))
            .child(self.message_row(
                "Match accepted",
                &self.templates.auto_accepted,
                &mentions,
                cx,
            ))
            .child(self.message_row("Game started", &self.templates.game_started, &mentions, cx));

        div()
            .flex()
            .flex_col()
            .gap(theme::space_6())
            .child(delivery)
            .child(event_group)
            .child(message_group)
            .into_any_element()
    }

    /// Wraps a text input so it reads as an editable field. The input widget paints no frame of its
    /// own, so the recessed background and hairline border are what mark it as a place to type.
    fn input_frame(state: &Entity<InputState>, cx: &gpui::App) -> impl IntoElement {
        let t = Theme::of(cx);
        div()
            .bg(t.bg)
            .border_1()
            .border_color(t.border)
            .rounded(theme::radius_md())
            .px(theme::space_3())
            .py(theme::space_2())
            .child(Input::new(state))
    }

    /// A message template. The text is validated as it will be sent — with `{mentions}` expanded
    /// and the saved mentions substituted — so what is checked is the message Discord receives.
    fn message_row(
        &self,
        label: &'static str,
        template: &Entity<InputState>,
        mentions: &[DiscordMention],
        cx: &mut Context<Self>,
    ) -> Row {
        let t = Theme::of(cx);
        let rendered = preview_notification(&template.read(cx).value(), mentions);
        let error = content_error(&rendered);

        Row::new(label)
            .child(
                div()
                    .max_w(px(440.0))
                    .child(Self::input_frame(template, cx)),
            )
            .when_some(error, |this, message| {
                this.child(
                    div()
                        .text_size(theme::text_caption())
                        .text_color(t.danger)
                        .child(message),
                )
            })
    }

    fn render_settings(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::of(cx);
        let appearance = theme::current();
        let treatment = theme::current_treatment();
        let minimize = self.state.settings.minimize_to_tray;

        let window = SettingsGroup::new().title("Window").child(Self::toggle_row(
            "toggle-minimize",
            "Close to tray",
            "Keep running in the system tray when the window is closed.",
            minimize,
            ToggleSetting::MinimizeToTray,
            cx,
        ));

        let theme_control = div()
            .flex()
            .gap(theme::space_3())
            .child(self.appearance_option(
                "theme-dark",
                Appearance::Dark,
                appearance == Appearance::Dark,
                cx,
            ))
            .child(self.appearance_option(
                "theme-light",
                Appearance::Light,
                appearance == Appearance::Light,
                cx,
            ));

        let backdrop_control = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .p(px(2.0))
            .bg(t.surface_active)
            .border_1()
            .border_color(t.border)
            .rounded(theme::radius_md())
            .child(self.backdrop_option(
                "backdrop-opaque",
                SurfaceTreatment::Opaque,
                treatment == SurfaceTreatment::Opaque,
                cx,
            ))
            .child(self.backdrop_option(
                "backdrop-mica",
                SurfaceTreatment::Mica,
                treatment == SurfaceTreatment::Mica,
                cx,
            ))
            .child(self.backdrop_option(
                "backdrop-acrylic",
                SurfaceTreatment::Acrylic,
                treatment == SurfaceTreatment::Acrylic,
                cx,
            ));

        let appearance_group = SettingsGroup::new()
            .title("Appearance")
            .child(Row::new("Theme").control(theme_control))
            .child(
                Row::new("Window backdrop")
                    .description(
                        "Mica tints from the desktop wallpaper; Acrylic blurs what is behind the \
                         window in real time. Falls back to opaque where unsupported.",
                    )
                    .control(backdrop_control),
            );

        div()
            .flex()
            .flex_col()
            .gap(theme::space_6())
            .child(window)
            .child(appearance_group)
            .into_any_element()
    }

    /// A backdrop choice rendered as one segment of a segmented control.
    fn backdrop_option(
        &self,
        id: &'static str,
        value: SurfaceTreatment,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = Theme::of(cx);
        let label = match value {
            SurfaceTreatment::Opaque => "Opaque",
            SurfaceTreatment::Mica => "Mica",
            SurfaceTreatment::Acrylic => "Acrylic",
        };
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .px(theme::space_3())
            .py(theme::space_1())
            .rounded(theme::radius_sm())
            .cursor_pointer()
            .when(selected, |this| this.bg(t.accent).text_color(t.accent_fg))
            .when(!selected, |this| this.text_color(t.text_muted))
            .hover(move |style| {
                if selected {
                    style
                } else {
                    style.text_color(t.text)
                }
            })
            .on_click(cx.listener(move |this, _event: &ClickEvent, window, cx| {
                this.set_treatment(window, cx, value);
            }))
            .child(
                div()
                    .text_size(theme::text_small())
                    .font_weight(theme::weight_medium())
                    .child(label),
            )
            .into_any_element()
    }

    /// A theme choice rendered as a mini window preview in that theme, so the effect is visible
    /// before it is applied.
    fn appearance_option(
        &self,
        id: &'static str,
        value: Appearance,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let current = Theme::of(cx);
        let target = value.theme();
        let label = match value {
            Appearance::Dark => "Dark",
            Appearance::Light => "Light",
        };
        let ring = if selected {
            current.accent
        } else {
            current.border
        };

        div()
            .id(id)
            .flex()
            .flex_col()
            .items_center()
            .gap(theme::space_2())
            .cursor_pointer()
            .on_click(cx.listener(move |this, _event: &ClickEvent, window, cx| {
                theme::set_appearance(cx, value);
                this.persist(json!({ "themeMode": ThemeMode::from(value) }));
                // Mica/Acrylic map to a different backdrop variant per appearance, so re-apply.
                let treatment = theme::current_treatment();
                if treatment != SurfaceTreatment::Opaque {
                    let _ = glass::apply(window, value, treatment);
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(96.0))
                    .h(px(64.0))
                    .rounded(theme::radius_md())
                    .overflow_hidden()
                    .border_2()
                    .border_color(ring)
                    .bg(target.window_bg)
                    .child(
                        div()
                            .w_full()
                            .h(px(10.0))
                            .flex_shrink_0()
                            .bg(target.sidebar_bg)
                            .border_b_1()
                            .border_color(target.border),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h(px(0.0))
                            .child(div().w(px(18.0)).h_full().bg(target.sidebar_bg))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .gap(px(4.0))
                                    .p(px(7.0))
                                    .child(
                                        div()
                                            .w_full()
                                            .h(px(7.0))
                                            .rounded(px(2.0))
                                            .bg(target.surface_hover),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .h(px(7.0))
                                            .rounded(px(2.0))
                                            .bg(target.surface_hover),
                                    )
                                    .child(
                                        div()
                                            .w(px(28.0))
                                            .h(px(7.0))
                                            .rounded(px(2.0))
                                            .bg(target.surface_hover),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(theme::text_small())
                    .font_weight(if selected {
                        theme::weight_medium()
                    } else {
                        theme::weight_regular()
                    })
                    .text_color(if selected {
                        current.text
                    } else {
                        current.text_muted
                    })
                    .child(label),
            )
            .into_any_element()
    }

    fn render_mentions(&self, t: &Theme, cx: &mut Context<Self>) -> Row {
        let mentions = self.state.settings.discord_mentions.clone();

        let rows: Vec<gpui::AnyElement> = mentions
            .iter()
            .map(|mention| {
                let nickname = if mention.nickname.is_empty() {
                    "Unnamed".to_string()
                } else {
                    mention.nickname.clone()
                };
                let remove_id = mention.id.clone();
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme::space_3())
                    .px(theme::space_3())
                    .py(theme::space_2())
                    .bg(t.surface_hover)
                    .rounded(theme::radius_md())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::space_3())
                            .child(
                                div()
                                    .text_size(theme::text_body())
                                    .text_color(t.text)
                                    .child(nickname),
                            )
                            .child(
                                div()
                                    .text_size(theme::text_small())
                                    .text_color(t.text_faint)
                                    .child(mention.id.clone()),
                            ),
                    )
                    .child(
                        Button::new(format!("remove-mention-{remove_id}"), "Remove")
                            .variant(ButtonVariant::Ghost)
                            .size(crate::ui::components::ButtonSize::Sm)
                            .on_click(cx.listener(
                                move |this, _event: &ClickEvent, _window, cx| {
                                    let remaining: Vec<DiscordMention> = this
                                        .state
                                        .settings
                                        .discord_mentions
                                        .iter()
                                        .filter(|item| item.id != remove_id)
                                        .cloned()
                                        .collect();
                                    this.persist(json!({ "discordMentions": remaining }));
                                    cx.notify();
                                },
                            )),
                    )
                    .into_any_element()
            })
            .collect();

        let at_capacity = mentions.len() >= MAX_DISCORD_USER_IDS;
        let add_row = div()
            .flex()
            .items_center()
            .gap(theme::space_2())
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(Self::input_frame(&self.mention_nickname, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(Self::input_frame(&self.mention_id, cx)),
            )
            .child(
                Button::new("add-mention", "Add")
                    .variant(ButtonVariant::Secondary)
                    .disabled(at_capacity)
                    .on_click(cx.listener(|this, _event: &ClickEvent, window, cx| {
                        let id = this.mention_id.read(cx).value().trim().to_string();
                        let nickname = this.mention_nickname.read(cx).value().trim().to_string();
                        if !is_valid_discord_id(&id) {
                            this.mention_error =
                                Some("Enter a valid Discord user ID (17–20 digits).".to_string());
                            cx.notify();
                            return;
                        }
                        if this
                            .state
                            .settings
                            .discord_mentions
                            .iter()
                            .any(|item| item.id == id)
                        {
                            this.mention_error = Some("That user is already added.".to_string());
                            cx.notify();
                            return;
                        }
                        if this.state.settings.discord_mentions.len() >= MAX_DISCORD_USER_IDS {
                            this.mention_error =
                                Some("You can mention up to 5 people.".to_string());
                            cx.notify();
                            return;
                        }
                        let mut next = this.state.settings.discord_mentions.clone();
                        next.push(DiscordMention {
                            id: id.clone(),
                            nickname: nickname.chars().take(40).collect(),
                        });
                        this.persist(json!({ "discordMentions": next }));
                        this.mention_error = None;
                        this.mention_id
                            .update(cx, |state, cx| state.set_value("", window, cx));
                        this.mention_nickname
                            .update(cx, |state, cx| state.set_value("", window, cx));
                        cx.notify();
                    })),
            );

        let mut row = Row::new("People to mention")
            .description("Mentioned at the start of each notification. Up to 5.")
            .children(rows)
            .child(add_row);
        if let Some(error) = self.mention_error.clone() {
            row = row.child(
                div()
                    .text_size(theme::text_caption())
                    .text_color(t.danger)
                    .child(error),
            );
        }
        row
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
            .child(title_bar)
            .child(
                div().flex().flex_1().min_h(px(0.0)).child(sidebar).child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.0))
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
