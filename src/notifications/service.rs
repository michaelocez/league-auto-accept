//! Discord notification service — strictly downstream of Auto Accept.
//!
//! Delivery is fire-and-forget: `notification_for` returns a future the caller spawns and never
//! awaits on the accept path, so a notification can never delay, retry, or change acceptance.

use std::sync::Arc;

use crate::config::settings::{allowed_mention_ids, AppSettings};
use crate::league::ready_check::Lifecycle;

use super::discord::{
    render_notification_message, template_error, DiscordSender, WebhookFuture, WebhookRequest,
    WebhookResult, WebhookSecret,
};

/// A settings snapshot plus the webhook secret the sender needs. The secret is read here only, and
/// is deliberately absent from [`AppSettings`] so it never reaches application state or the UI.
#[derive(Clone, Debug, Default)]
pub struct NotificationSettings {
    pub settings: AppSettings,
    pub webhook: WebhookSecret,
}

/// Reads the current settings snapshot.
pub type SettingsSnapshot = Arc<dyn Fn() -> NotificationSettings + Send + Sync>;

pub struct NotificationService {
    settings: SettingsSnapshot,
    sender: Arc<dyn DiscordSender>,
}

impl NotificationService {
    pub fn new(settings: SettingsSnapshot, sender: Arc<dyn DiscordSender>) -> Self {
        Self { settings, sender }
    }

    /// Returns a fire-and-forget delivery future for a lifecycle event, or `None` when the event is
    /// disabled globally or individually. The caller must spawn (not await) the future.
    pub fn notification_for(&self, event: Lifecycle) -> Option<WebhookFuture> {
        let current = (self.settings)();
        let settings = &current.settings;
        if !settings.discord_notifications_enabled {
            return None;
        }
        let (enabled, template) = match event {
            Lifecycle::QueuePopped => (
                settings.notification_events.queue_popped,
                settings.notification_messages.queue_popped.clone(),
            ),
            Lifecycle::AutoAccepted => (
                settings.notification_events.auto_accepted,
                settings.notification_messages.auto_accepted.clone(),
            ),
            Lifecycle::GameStarted => (
                settings.notification_events.game_started,
                settings.notification_messages.game_started.clone(),
            ),
        };
        if !enabled {
            return None;
        }
        Some(self.dispatch(&current, &template))
    }

    /// Builds the Test Webhook request using the saved settings and configured mentions.
    pub fn test_webhook(&self) -> WebhookFuture {
        let current = (self.settings)();
        let has_mentions = !current.settings.discord_mentions.is_empty();
        let prefix = if has_mentions { "{mentions} " } else { "" };
        let template = format!("{prefix}League Auto Accept webhook test.");
        self.dispatch(&current, &template)
    }

    fn dispatch(&self, current: &NotificationSettings, template: &str) -> WebhookFuture {
        // The template cap is enforced here as well as in the editor, so the sender cannot deliver
        // a message the store would have shortened — the same defect arriving by another route (a
        // hand-written settings file, or an older build).
        if let Some(message) = template_error(template) {
            return Box::pin(async move { WebhookResult::failure(message) });
        }
        // Re-apply the mention allowlist and cap at send time: settings are normalised when they
        // are written, but the delivered message must never exceed the cap regardless of how the
        // snapshot was produced.
        let user_ids = allowed_mention_ids(&current.settings.discord_mentions);
        let content = render_notification_message(template, &user_ids);
        self.sender.send(WebhookRequest {
            url: current.webhook.clone(),
            content,
            user_ids,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::{DiscordMention, MAX_DISCORD_USER_IDS};
    use crate::notifications::discord::WebhookResult;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeSender {
        requests: Mutex<Vec<WebhookRequest>>,
        reject: bool,
    }

    impl DiscordSender for FakeSender {
        fn send(&self, request: WebhookRequest) -> WebhookFuture {
            self.requests.lock().unwrap().push(request);
            let reject = self.reject;
            Box::pin(async move {
                if reject {
                    WebhookResult::failure("Discord webhook request failed.")
                } else {
                    WebhookResult::ok()
                }
            })
        }
    }

    fn settings() -> NotificationSettings {
        NotificationSettings {
            settings: AppSettings {
                discord_notifications_enabled: true,
                discord_mentions: vec![crate::config::settings::DiscordMention {
                    id: "12345678901234567".into(),
                    nickname: "Michael".into(),
                }],
                notification_events: crate::config::settings::NotificationEvents {
                    queue_popped: true,
                    auto_accepted: true,
                    game_started: true,
                },
                ..AppSettings::default()
            },
            webhook: WebhookSecret::new(format!(
                "https://discord.com/api/webhooks/{}/{}",
                "1".repeat(17),
                "t".repeat(40)
            )),
        }
    }

    fn service(current: NotificationSettings, sender: Arc<FakeSender>) -> NotificationService {
        let snapshot = Arc::new(move || current.clone());
        NotificationService::new(snapshot, sender)
    }

    #[tokio::test]
    async fn sends_enabled_events_with_template_and_allowlist() {
        let mut current = settings();
        current.settings.notification_messages.queue_popped =
            "{mentions} custom queue message".into();
        let sender = Arc::new(FakeSender::default());
        let service = service(current, sender.clone());

        let future = service
            .notification_for(Lifecycle::QueuePopped)
            .expect("enabled event should notify");
        let result = future.await;
        assert!(result.ok);

        let requests = sender.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].content,
            "<@12345678901234567> custom queue message"
        );
        assert_eq!(requests[0].user_ids, vec!["12345678901234567".to_string()]);
    }

    #[tokio::test]
    async fn skips_globally_and_individually_disabled_events() {
        let mut global_off = settings();
        global_off.settings.discord_notifications_enabled = false;
        let sender = Arc::new(FakeSender::default());
        assert!(service(global_off, sender.clone())
            .notification_for(Lifecycle::QueuePopped)
            .is_none());

        let mut event_off = settings();
        event_off.settings.notification_events.queue_popped = false;
        assert!(service(event_off, sender.clone())
            .notification_for(Lifecycle::QueuePopped)
            .is_none());
        assert!(sender.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_webhook_uses_saved_settings_and_mentions() {
        let sender = Arc::new(FakeSender::default());
        let service = service(settings(), sender.clone());
        let result = service.test_webhook().await;
        assert!(result.ok);
        let requests = sender.requests.lock().unwrap();
        assert_eq!(
            requests[0].content,
            "<@12345678901234567> League Auto Accept webhook test."
        );
    }

    #[tokio::test]
    async fn the_webhook_secret_reaches_the_sender_but_not_debug_output() {
        let sender = Arc::new(FakeSender::default());
        let current = settings();
        let rendered = format!("{current:?}");
        assert!(!rendered.contains("discord.com"), "{rendered}");

        let service = service(current, sender.clone());
        assert!(service.test_webhook().await.ok);
        let requests = sender.requests.lock().unwrap();
        assert!(requests[0].url.is_configured());
    }

    #[tokio::test]
    async fn caps_mentions_at_send_time_even_for_an_unnormalised_snapshot() {
        let mut current = settings();
        let mut mentions: Vec<DiscordMention> = (0..7)
            .map(|index| DiscordMention {
                id: format!("{:017}", index + 1),
                nickname: String::new(),
            })
            .collect();
        mentions.push(DiscordMention {
            id: "12345678901234567".into(), // duplicate of the first entry
            nickname: String::new(),
        });
        mentions.push(DiscordMention {
            id: "not-an-id".into(),
            nickname: String::new(),
        });
        current.settings.discord_mentions = mentions;
        current.settings.notification_messages.queue_popped = "{mentions}".into();

        let sender = Arc::new(FakeSender::default());
        let service = service(current, sender.clone());
        assert!(service.notification_for(Lifecycle::QueuePopped).is_some());

        let requests = sender.requests.lock().unwrap();
        assert_eq!(requests[0].user_ids.len(), MAX_DISCORD_USER_IDS);
        assert_eq!(requests[0].user_ids[0], "00000000000000001");
        assert_eq!(
            requests[0].content,
            "<@00000000000000001> <@00000000000000002> <@00000000000000003> <@00000000000000004> <@00000000000000005>"
        );
        assert!(!requests[0].content.contains("not-an-id"));
    }

    /// A template at the stored cap, with the maximum number of mentions configured, is delivered —
    /// not refused for length.
    ///
    /// This is the end of the chain the fix has to hold: the cap says the store keeps the template,
    /// the mention budget says the render fits Discord, and the sender must therefore accept it. It
    /// fails if the budget is ever sized too small for what the editor and store allow.
    #[tokio::test]
    async fn sends_a_template_at_the_cap_with_every_mention() {
        let mut current = settings();
        current.settings.discord_mentions = (0..MAX_DISCORD_USER_IDS)
            .map(|index| DiscordMention {
                id: format!("{:020}", index + 1),
                nickname: String::new(),
            })
            .collect();
        current.settings.notification_messages.auto_accepted = "{mentions}".to_string()
            + &"t".repeat(crate::notifications::discord::MAX_TEMPLATE_LENGTH - "{mentions}".len());

        let sender = Arc::new(FakeSender::default());
        let service = service(current, sender.clone());
        let future = service
            .notification_for(Lifecycle::AutoAccepted)
            .expect("an enabled event should notify");
        assert!(
            future.await.ok,
            "a template the store keeps must still be deliverable"
        );

        let requests = sender.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        // The budget is a ceiling: the longest possible mention is 23 characters while these ids are
        // 20 digits, so the delivered content lands just inside Discord's limit.
        assert!(
            requests[0].content.chars().count()
                <= crate::notifications::discord::MAX_CONTENT_LENGTH,
            "the delivered content must fit Discord's limit, got {}",
            requests[0].content.chars().count()
        );
    }

    /// A template longer than the cap is refused rather than delivered, even when it arrives from
    /// somewhere the editor could not have produced it — a hand-written settings file, or an older
    /// build. Delivering it would re-create the original defect by another route: the app sending a
    /// message the store would have shortened.
    #[tokio::test]
    async fn refuses_to_deliver_a_template_longer_than_the_cap() {
        let mut current = settings();
        current.settings.notification_messages.auto_accepted =
            "t".repeat(crate::notifications::discord::MAX_TEMPLATE_LENGTH + 1);

        let sender = Arc::new(FakeSender::default());
        let service = service(current, sender.clone());
        let future = service
            .notification_for(Lifecycle::AutoAccepted)
            .expect("the event is enabled, so a delivery is attempted");
        let result = future.await;

        assert!(!result.ok, "an over-long template must not be delivered");
        assert!(
            result.message.contains("template"),
            "the failure must name the template length: {}",
            result.message
        );
        assert!(
            sender.requests.lock().unwrap().is_empty(),
            "nothing may reach the network for a refused template"
        );
    }

    #[tokio::test]
    async fn delivery_failure_is_returned_not_panicked() {
        let sender = Arc::new(FakeSender {
            requests: Mutex::new(Vec::new()),
            reject: true,
        });
        let service = service(settings(), sender);
        let future = service
            .notification_for(Lifecycle::AutoAccepted)
            .expect("enabled");
        let result = future.await;
        assert!(!result.ok);
        assert_eq!(result.message, "Discord webhook request failed.");
    }
}
