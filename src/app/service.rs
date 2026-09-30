//! Background service bridge.
//!
//! Runs the headless `LeagueService` and the Discord notification sender on a dedicated OS thread
//! with its own tokio runtime, forwarding `ServiceEvent`s into the UI channel. Notifications are
//! triggered downstream of lifecycle events only and are fire-and-forget (never awaited on the
//! accept path). The UI never owns credentials, networking or secrets.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use crate::app::events::AppEvent;
use crate::config::settings::AppSettings;
use crate::league::discovery::ClientLocator;
use crate::league::service::{LeagueService, ServiceEvent};
use crate::notifications::discord::DiscordWebhookClient;
use crate::notifications::service::NotificationService;

/// Commands the UI sends to the notification backend.
#[derive(Clone, Copy, Debug)]
pub enum NotificationCommand {
    TestWebhook,
}

/// Handles for driving the background service from the UI.
pub struct BackgroundService {
    pub enabled_flag: Arc<AtomicBool>,
    pub notifications: async_channel::Sender<NotificationCommand>,
}

/// Starts the League service + notifications and bridges their events to `tx`.
pub fn spawn_league_service(
    tx: async_channel::Sender<AppEvent>,
    settings: Arc<RwLock<AppSettings>>,
    auto_accept_enabled: bool,
) -> BackgroundService {
    let enabled_flag = Arc::new(AtomicBool::new(auto_accept_enabled));
    let enabled_for_service = enabled_flag.clone();
    let (notification_tx, notification_rx) = async_channel::unbounded::<NotificationCommand>();

    let spawned = std::thread::Builder::new()
        .name("laa-league-service".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    log::error!("failed to build League service runtime: {error}");
                    return;
                }
            };

            runtime.block_on(async move {
                let (service_tx, service_rx) = async_channel::unbounded::<ServiceEvent>();

                let snapshot = {
                    let settings = settings.clone();
                    Arc::new(move || settings.read().map(|guard| guard.clone()).unwrap_or_default())
                };
                let notifications = Arc::new(NotificationService::new(
                    snapshot,
                    Arc::new(DiscordWebhookClient::new()),
                ));

                let bridge_notifications = notifications.clone();
                let bridge_tx = tx.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            event = service_rx.recv() => {
                                let Ok(event) = event else { break };
                                if let ServiceEvent::Lifecycle(lifecycle) = &event {
                                    if let Some(future) = bridge_notifications.notification_for(*lifecycle) {
                                        tokio::spawn(future);
                                    }
                                }
                                if bridge_tx.send(AppEvent::Service(event)).await.is_err() {
                                    break;
                                }
                            }
                            command = notification_rx.recv() => {
                                let Ok(NotificationCommand::TestWebhook) = command else { break };
                                let future = bridge_notifications.test_webhook();
                                let tx = bridge_tx.clone();
                                tokio::spawn(async move {
                                    let result = future.await;
                                    let _ = tx.send(AppEvent::WebhookTest(result)).await;
                                });
                            }
                        }
                    }
                });

                let mut service = LeagueService::with_enabled_flag(
                    ClientLocator::default(),
                    enabled_for_service,
                    service_tx,
                );
                service.run().await;
            });
        });

    if let Err(error) = spawned {
        log::error!("could not spawn League service thread: {error}");
    }

    BackgroundService {
        enabled_flag,
        notifications: notification_tx,
    }
}
