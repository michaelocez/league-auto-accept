//! Discord notification service — strictly downstream of Auto Accept.
//!
//! Ported from the reference implementation (`Electron V1/src/main/discord-notification-service.ts`).
//! Delivery is fire-and-forget: `notification_for` returns a future the caller spawns and never
//! awaits on the accept path, so a notification can never delay, retry, or change acceptance.

use std::sync::Arc;

use crate::config::settings::AppSettings;
use crate::league::ready_check::Lifecycle;

use super::discord::{
    render_notification_message, DiscordSender, WebhookFuture, WebhookRequest,
};

/// Reads the current settings snapshot.
pub type SettingsSnapshot = Arc<dyn Fn() -> AppSettings + Send + Sync>;

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
        let settings = (self.settings)();
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
        Some(self.dispatch(&settings, &template))
    }

    /// Builds the Test Webhook request using the saved settings and configured mentions.
    pub fn test_webhook(&self) -> WebhookFuture {
        let settings = (self.settings)();
        let has_mentions = !settings.discord_mentions.is_empty();
        let prefix = if has_mentions { "{mentions} " } else { "" };
        let template = format!("{prefix}League Auto Accept webhook test.");
        self.dispatch(&settings, &template)
    }

    fn dispatch(&self, settings: &AppSettings, template: &str) -> WebhookFuture {
        let user_ids: Vec<String> = settings
            .discord_mentions
            .iter()
            .map(|mention| mention.id.clone())
            .collect();
        let content = render_notification_message(template, &user_ids);
        self.sender.send(WebhookRequest {
            url: settings.discord_webhook_url.clone(),
            content,
            user_ids,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::discord::{WebhookResult, MAX_CONTENT_LENGTH};
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

    fn settings() -> AppSettings {
        AppSettings {
            discord_notifications_enabled: true,
            discord_webhook_url: format!(
                "https://discord.com/api/webhooks/{}/{}",
                "1".repeat(17),
                "t".repeat(40)
            ),
            discord_mentions: vec![
                crate::config::settings::DiscordMention {
                    id: "12345678901234567".into(),
                    nickname: "Michael".into(),
                },
            ],
            notification_events: crate::config::settings::NotificationEvents {
                queue_popped: true,
                auto_accepted: true,
                game_started: true,
            },
            ..AppSettings::default()
        }
    }

    fn service(current: AppSettings, sender: Arc<FakeSender>) -> NotificationService {
        let snapshot = Arc::new(move || current.clone());
        NotificationService::new(snapshot, sender)
    }

    #[tokio::test]
    async fn sends_enabled_events_with_template_and_allowlist() {
        let mut current = settings();
        current.notification_messages.queue_popped = "{mentions} custom queue message".into();
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
        global_off.discord_notifications_enabled = false;
        let sender = Arc::new(FakeSender::default());
        assert!(service(global_off, sender.clone())
            .notification_for(Lifecycle::QueuePopped)
            .is_none());

        let mut event_off = settings();
        event_off.notification_events.queue_popped = false;
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
        assert!(result.message.chars().count() <= MAX_CONTENT_LENGTH + 64);
    }
}
