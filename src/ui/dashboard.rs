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
    div, px, ClickEvent, Context, Entity, FocusHandle, IntoElement, Render, Rgba, Subscription,
    Window,
};
use serde_json::json;

use crate::app::events::{AppEvent, EventOutcome};
use crate::app::service::NotificationCommand;
use crate::app::state::{ActivityKind, AppState};
use crate::config::settings::{
    is_valid_discord_id, AppSettings, BackdropMode, DiscordMention, ThemeMode, MAX_DISCORD_USER_IDS,
};
use crate::config::store::SettingsStore;
use crate::notifications::discord::parse_discord_webhook_url;
use crate::platform::glass;
use crate::platform::tray::TrayCommand;
use crate::ui::components::{
    Button, ButtonVariant, Divider, Elevation, Row, SettingsGroup, Surface, Toggle,
};
use crate::ui::shell::{PageHeader, Screen, Sidebar, TitleBar};
use crate::ui::theme::{self, Appearance, SurfaceTreatment, Theme};

/// Which boolean setting a toggle row switch controls.
#[derive(Clone, Copy)]
enum ToggleSetting {
    DiscordNotifications,
    MinimizeToTray,
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
        let mention_nickname =
            cx.new(|cx| InputState::new(window, cx).placeholder("Nickname (optional)"));
        let mention_id = cx.new(|cx| InputState::new(window, cx).placeholder("Discord user ID"));
        Self {
            state: AppState::new(settings),
            store,
            enabled_flag,
            settings_shared,
            notifications,
            webhook,
            webhook_draft,
            mention_nickname,
            mention_id,
            mention_error: None,
            subscriptions: vec![subscription],
            screen: Screen::AutoAccept,
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
        ActivityKind::Warning => t.danger,
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
        let last = self.state.activity.len() - 1;
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
        let webhook_error = if self.webhook_draft.trim().is_empty() {
            String::new()
        } else if parse_discord_webhook_url(&self.webhook_draft).is_none() {
            "Enter a valid Discord webhook URL.".to_string()
        } else {
            String::new()
        };
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
            .description("Stored in the privileged backend; never shown to other apps.")
            .child(div().max_w(px(440.0)).child(Input::new(&self.webhook)));
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

        SettingsGroup::new()
            .child(Self::toggle_row(
                "toggle-discord",
                "Enable Discord notifications",
                "Queue popped, auto-accepted and game-started alerts.",
                discord_enabled,
                ToggleSetting::DiscordNotifications,
                cx,
            ))
            .child(webhook_row)
            .child(self.render_mentions(&t, cx))
            .child(test_row)
            .into_any_element()
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
                    .child(Input::new(&self.mention_nickname)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(Input::new(&self.mention_id)),
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
                                .child(div().flex().flex_col().flex_shrink_0().child(body)),
                        ),
                ),
            )
            .track_focus(&self.focus_handle)
    }
}
