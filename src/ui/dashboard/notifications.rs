//! The Notifications screen: Discord delivery, event switches, mentions and message templates.

use gpui::base::input::{Input, InputState};
use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, Entity, IntoElement};
use serde_json::json;

use crate::app::service::NotificationCommand;
use crate::config::settings::{is_valid_discord_id, DiscordMention, MAX_DISCORD_USER_IDS};
use crate::notifications::discord::{
    content_error, parse_discord_webhook_url, preview_notification, template_error,
    MAX_TEMPLATE_LENGTH,
};
use crate::ui::components::{Button, ButtonVariant, Row, SettingsGroup};
use crate::ui::decisions::{self, ToggleSetting, WebhookTestStart};
use crate::ui::theme::{self, Theme};

use super::Dashboard;

impl Dashboard {
    pub(super) fn render_notifications(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
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
                    .max_w(theme::size_field_max())
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
                    // A request that cannot be queued means no worker is left to answer it, so the
                    // button must not sit on "Sending…" waiting for a result that cannot come.
                    match decisions::webhook_test_start(
                        this.notifications
                            .try_send(NotificationCommand::TestWebhook),
                    ) {
                        WebhookTestStart::Queued => this.state.webhook_testing = true,
                        WebhookTestStart::Unavailable(message) => {
                            this.state.settings_error = Some(message);
                        }
                    }
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
                    .pt(theme::space_4())
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
    ///
    /// The field also carries the stored cap as its own validator (see `Dashboard::template_input`),
    /// and the character count is shown, so the length the store keeps is visible rather than
    /// discovered when a saved template comes back shorter.
    fn message_row(
        &self,
        label: &'static str,
        template: &Entity<InputState>,
        mentions: &[DiscordMention],
        cx: &mut Context<Self>,
    ) -> Row {
        let t = Theme::of(cx);
        let value = template.read(cx).value();
        let characters = value.chars().count();
        let rendered = preview_notification(&value, mentions);
        // Being over the stored cap is the more specific problem, so it is the one reported.
        let error = template_error(&value).or_else(|| content_error(&rendered).map(str::to_string));
        let counter = div()
            .text_size(theme::text_caption())
            .text_color(if characters > MAX_TEMPLATE_LENGTH {
                t.danger
            } else {
                t.text_muted
            })
            .child(format!("{characters} / {MAX_TEMPLATE_LENGTH} characters"));

        Row::new(label)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::space_1())
                    .max_w(theme::size_field_max())
                    .child(Self::input_frame(template, cx))
                    .child(counter),
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
