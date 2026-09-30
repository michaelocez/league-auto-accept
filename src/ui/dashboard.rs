//! Main window: dashboard + settings for Auto Accept and Discord notifications.
//!
//! The view renders `AppState` and emits user intents (toggles, Test Webhook). It contains no
//! networking, credentials or secret handling — those live in the app/notifications services.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use gpui::base::input::{Input, InputEvent, InputState};
use gpui::prelude::*;
use gpui::{
    div, ClickEvent, Context, Entity, FocusHandle, IntoElement, Render, Rgba, Subscription, Window,
};
use serde_json::json;

use crate::app::events::{AppEvent, EventOutcome};
use crate::app::service::NotificationCommand;
use crate::app::state::{ActivityKind, AppState};
use crate::config::settings::AppSettings;
use crate::config::store::SettingsStore;
use crate::notifications::discord::parse_discord_webhook_url;
use crate::platform::tray::TrayCommand;
use crate::ui::components::{
    Button, ButtonVariant, Divider, Elevation, LabeledField, StatusPill, Surface, Toggle,
};
use crate::ui::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Dashboard,
    Settings,
}

/// Which setting a toggle row switch controls.
#[derive(Clone, Copy)]
enum ToggleAction {
    AutoAccept,
    MinimizeToTray,
    Discord,
}

pub struct Dashboard {
    pub state: AppState,
    store: Rc<RefCell<SettingsStore>>,
    enabled_flag: Arc<AtomicBool>,
    settings_shared: Arc<RwLock<AppSettings>>,
    notifications: async_channel::Sender<NotificationCommand>,
    webhook: Entity<InputState>,
    webhook_draft: String,
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
        let webhook_draft = settings.discord_webhook_url.clone();
        let subscription = cx.subscribe_in(
            &webhook,
            window,
            |this, state, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.webhook_draft = state.read(cx).value().to_string();
                    this.persist(json!({ "discordWebhookUrl": this.webhook_draft.clone() }));
                    cx.notify();
                }
            },
        );
        Self {
            state: AppState::new(settings),
            store,
            enabled_flag,
            settings_shared,
            notifications,
            webhook,
            webhook_draft,
            subscriptions: vec![subscription],
            screen: Screen::Dashboard,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    fn persist(&mut self, patch: serde_json::Value) {
        match self.store.borrow_mut().update(&patch) {
            Ok(updated) => {
                self.state.settings = updated.clone();
                if let Ok(mut guard) = self.settings_shared.write() {
                    *guard = updated;
                }
            }
            Err(error) => log::error!("failed to persist settings: {error}"),
        }
    }

    pub fn apply_event(&mut self, event: AppEvent) -> EventOutcome {
        match event {
            AppEvent::Tray(TrayCommand::ShowWindow) => return EventOutcome::ShowWindow,
            AppEvent::Tray(TrayCommand::Quit) => return EventOutcome::Quit,
            AppEvent::Tray(TrayCommand::ToggleAutoAccept) => {
                let next = !self.state.settings.auto_accept_enabled;
                self.set_auto_accept(next);
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
}

fn status_color(summary: &str) -> Rgba {
    match summary {
        "Active" => theme::success(),
        "Connecting" => theme::warning(),
        "League offline" => theme::danger(),
        _ => theme::text_secondary(),
    }
}

fn activity_color(kind: ActivityKind) -> Rgba {
    match kind {
        ActivityKind::Info => theme::text_secondary(),
        ActivityKind::Accepted => theme::success(),
        ActivityKind::Warning => theme::danger(),
    }
}

impl Dashboard {
    /// A titled row with a switch on the right — the shared shape of an automation setting.
    fn toggle_row(
        id: &'static str,
        title: &'static str,
        subtitle: &'static str,
        checked: bool,
        action: ToggleAction,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        Surface::new().elevation(Elevation::Raised).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(theme::space_4())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::space_1())
                        .text_size(theme::text_body())
                        .child(div().text_color(theme::text_primary()).child(title))
                        .child(
                            div()
                                .text_size(theme::text_small())
                                .text_color(theme::text_secondary())
                                .child(subtitle),
                        ),
                )
                .child(Toggle::new(id).checked(checked).on_change(cx.listener(
                    move |this, _event: &ClickEvent, _window, cx| {
                        match action {
                            ToggleAction::AutoAccept => {
                                let next = !this.state.settings.auto_accept_enabled;
                                this.set_auto_accept(next);
                            }
                            ToggleAction::MinimizeToTray => {
                                let next = !this.state.settings.minimize_to_tray;
                                this.persist(json!({ "minimizeToTray": next }));
                            }
                            ToggleAction::Discord => {
                                let next = !this.state.settings.discord_notifications_enabled;
                                this.persist(json!({ "discordNotificationsEnabled": next }));
                            }
                        }
                        cx.notify();
                    },
                ))),
        )
    }

    fn nav_pill(
        id: &'static str,
        label: &'static str,
        active: bool,
        cx: &mut Context<Self>,
        screen: Screen,
    ) -> impl IntoElement {
        let (background, foreground) = if active {
            (theme::surface_raised(), theme::text_primary())
        } else {
            (theme::surface(), theme::text_secondary())
        };
        div()
            .id(id)
            .px_3()
            .py_1()
            .bg(background)
            .border_1()
            .border_color(theme::border())
            .rounded_full()
            .cursor_pointer()
            .hover(|style| style.text_color(theme::text_primary()))
            .on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                this.screen = screen;
                cx.notify();
            }))
            .child(div().text_sm().text_color(foreground).child(label))
    }

    fn render_dashboard(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let auto_accept = self.state.settings.auto_accept_enabled;
        let minimize = self.state.settings.minimize_to_tray;
        let connection_message = self.state.connection_message.clone();
        let ready_message = self.state.ready_check_message.clone();
        let monitoring = self.state.monitoring();

        let activity: Vec<gpui::AnyElement> = if self.state.activity.is_empty() {
            vec![div()
                .px_4()
                .py_3()
                .text_sm()
                .text_color(theme::text_secondary())
                .child("No activity yet.")
                .into_any_element()]
        } else {
            self.state
                .activity
                .iter()
                .map(|item| {
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_4()
                        .py_2()
                        .child(
                            div()
                                .size(gpui::px(6.0))
                                .rounded_full()
                                .bg(activity_color(item.kind)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme::text_primary())
                                .child(item.label.clone()),
                        )
                        .into_any_element()
                })
                .collect()
        };

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(connection_message),
            )
            .child(Self::toggle_row(
                "toggle-auto-accept",
                "Auto Accept",
                if monitoring {
                    "Monitoring ready checks."
                } else {
                    "Accepts one ready check per queue."
                },
                auto_accept,
                ToggleAction::AutoAccept,
                cx,
            ))
            .child(
                Surface::new().elevation(Elevation::Base).child(
                    div()
                        .text_size(theme::text_small())
                        .text_color(theme::text_secondary())
                        .child(ready_message),
                ),
            )
            .child(Self::toggle_row(
                "toggle-minimize",
                "Minimize to tray",
                "On: closing hides to tray. Off: closing exits (tray stays present).",
                minimize,
                ToggleAction::MinimizeToTray,
                cx,
            ))
            .child(
                Surface::new()
                    .elevation(Elevation::Base)
                    .padded(false)
                    .child(
                        div()
                            .px_4()
                            .pt_3()
                            .pb_1()
                            .text_sm()
                            .text_color(theme::text_secondary())
                            .child("Recent activity"),
                    )
                    .child(Divider::horizontal())
                    .children(activity),
            )
            .child(
                div().flex().justify_end().child(
                    Button::new("quit", "Quit")
                        .variant(ButtonVariant::Primary)
                        .on_click(cx.listener(|_this, _event: &ClickEvent, _window, cx| {
                            cx.quit();
                        })),
                ),
            )
            .into_any_element()
    }

    fn render_settings(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let discord_enabled = self.state.settings.discord_notifications_enabled;
        let mentions = self.state.settings.discord_mentions.len();
        let testing = self.state.webhook_testing;
        let webhook_error = if self.webhook_draft.trim().is_empty() {
            String::new()
        } else if parse_discord_webhook_url(&self.webhook_draft).is_none() {
            "Enter a valid Discord webhook URL.".to_string()
        } else {
            String::new()
        };
        let (test_message, test_color) = if testing {
            ("Sending test webhook…".to_string(), theme::text_secondary())
        } else {
            match &self.state.webhook_test {
                Some(result) if result.ok => (result.message.clone(), theme::success()),
                Some(result) => (result.message.clone(), theme::danger()),
                None => (
                    "Uses the saved webhook URL and configured mentions.".to_string(),
                    theme::text_secondary(),
                ),
            }
        };

        let webhook_field = if webhook_error.is_empty() {
            LabeledField::new()
                .label("Discord webhook URL")
                .helper(format!("People to mention: {mentions}"))
                .child(Input::new(&self.webhook))
        } else {
            LabeledField::new()
                .label("Discord webhook URL")
                .error(webhook_error)
                .child(Input::new(&self.webhook))
        };

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_color(theme::text_primary())
                            .child("Discord notifications"),
                    )
                    .child(div().text_sm().text_color(theme::text_secondary()).child(
                        "Messages are downstream of Auto Accept and never delay or control it.",
                    )),
            )
            .child(Self::toggle_row(
                "toggle-discord",
                "Enable Discord notifications",
                "Queue popped, auto-accepted and game-started alerts.",
                discord_enabled,
                ToggleAction::Discord,
                cx,
            ))
            .child(
                Surface::new()
                    .elevation(Elevation::Base)
                    .child(webhook_field),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        Button::new(
                            "test-webhook",
                            if testing {
                                "Sending…"
                            } else {
                                "Test Webhook"
                            },
                        )
                        .variant(ButtonVariant::Primary)
                        .disabled(testing)
                        .on_click(cx.listener(
                            |this, _event: &ClickEvent, _window, cx| {
                                if this.state.webhook_testing {
                                    return;
                                }
                                this.state.webhook_test = None;
                                this.state.webhook_testing = true;
                                let _ = this
                                    .notifications
                                    .try_send(NotificationCommand::TestWebhook);
                                cx.notify();
                            },
                        )),
                    )
                    .child(div().text_sm().text_color(test_color).child(test_message)),
            )
            .into_any_element()
    }
}

impl Render for Dashboard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.state.summary();
        let pill_color = status_color(summary);
        let screen = self.screen;
        let body = match screen {
            Screen::Dashboard => self.render_dashboard(cx),
            Screen::Settings => self.render_settings(cx),
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::background())
            .text_color(theme::text_primary())
            .p_6()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_xl().child("League Auto Accept"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::text_secondary())
                                    .child("Ready-check automation"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Self::nav_pill(
                                "nav-dashboard",
                                "Dashboard",
                                screen == Screen::Dashboard,
                                cx,
                                Screen::Dashboard,
                            ))
                            .child(Self::nav_pill(
                                "nav-settings",
                                "Settings",
                                screen == Screen::Settings,
                                cx,
                                Screen::Settings,
                            ))
                            .child(StatusPill::new(summary, pill_color)),
                    ),
            )
            .child(body)
            .track_focus(&self.focus_handle)
    }
}
