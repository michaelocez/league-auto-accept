//! Background service bridge.
//!
//! Runs the headless `LeagueService` and the Discord notification sender on a dedicated OS thread
//! with its own tokio runtime, forwarding `ServiceEvent`s into the UI channel. Notifications are
//! triggered downstream of lifecycle events only and are fire-and-forget (never awaited on the
//! accept path). The UI never owns credentials, networking or secrets.
//!
//! The thread is supervised: a panic in the service or in the bridge that forwards its events is
//! caught, logged and reported on the channel the UI is waiting on. A worker that dies silently
//! would leave the UI waiting forever for an event that can no longer arrive.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use crate::app::events::AppEvent;
use crate::app::update::{self, GithubReleaseFetcher};
use crate::config::settings::AppSettings;
use crate::league::discovery::ClientLocator;
use crate::league::service::{LeagueService, ServiceEvent};
use crate::notifications::discord::{DiscordWebhookClient, WebhookSecret};
use crate::notifications::service::{NotificationService, NotificationSettings};

/// Commands the UI sends to the notification backend.
#[derive(Clone, Copy, Debug)]
pub enum NotificationCommand {
    TestWebhook,
}

/// Commands the UI sends for the release check.
///
/// There is exactly one, and it carries nothing: the endpoint, the headers, the body cap and the
/// timeout are all fixed. It is a separate channel from [`NotificationCommand`] so the released
/// Discord command path is untouched by this feature.
#[derive(Clone, Copy, Debug)]
pub enum UpdateCommand {
    Check,
}

/// Handles for driving the background service from the UI.
pub struct BackgroundService {
    pub enabled_flag: Arc<AtomicBool>,
    pub notifications: async_channel::Sender<NotificationCommand>,
    pub update_check: async_channel::Sender<UpdateCommand>,
}

/// Starts the League service + notifications and bridges their events to `tx`.
///
/// `webhook` is the shared webhook secret. It is handed only to the notification sender; the UI
/// holds the settings store, not this handle, and sees just its non-secret status.
pub fn spawn_league_service(
    tx: async_channel::Sender<AppEvent>,
    settings: Arc<RwLock<AppSettings>>,
    webhook: Arc<RwLock<WebhookSecret>>,
    auto_accept_enabled: bool,
) -> BackgroundService {
    let enabled_flag = Arc::new(AtomicBool::new(auto_accept_enabled));
    let enabled_for_service = enabled_flag.clone();
    let (notification_tx, notification_rx) = async_channel::unbounded::<NotificationCommand>();
    let (update_tx, update_rx) = async_channel::unbounded::<UpdateCommand>();

    // The stop report has to go to the channel the UI is waiting on, not to the service channel the
    // bridge forwards — the bridge may be the very thing that died.
    let report_tx = tx.clone();
    let spawned = std::thread::Builder::new()
        .name("laa-league-service".to_string())
        .spawn(move || {
            run_supervised("League service", &report_tx, move || {
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
                        let webhook = webhook.clone();
                        Arc::new(move || NotificationSettings {
                            settings: settings.read().map(|guard| guard.clone()).unwrap_or_default(),
                            webhook: webhook.read().map(|guard| guard.clone()).unwrap_or_default(),
                        })
                    };
                    let notifications = Arc::new(NotificationService::new(
                        snapshot,
                        Arc::new(DiscordWebhookClient::new()),
                    ));
                    let release_fetcher = Arc::new(GithubReleaseFetcher::new());

                    let bridge_notifications = notifications.clone();
                    let bridge_tx = tx.clone();
                    // The bridge runs in this task rather than in a second `tokio::spawn`: a spawned
                    // task's panic is swallowed by the runtime, while a panic here unwinds to the
                    // supervisor above, which is what lets the UI be told why its channel went quiet.
                    let bridge = async move {
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
                                command = update_rx.recv() => {
                                    let Ok(UpdateCommand::Check) = command else { break };
                                    // Fire-and-forget, like the Test Webhook above: the answer reaches
                                    // the UI as an `AppEvent`, and nothing here waits for it.
                                    let fetcher = release_fetcher.clone();
                                    let tx = bridge_tx.clone();
                                    tokio::spawn(async move {
                                        let notice = update::check_release(fetcher.as_ref()).await;
                                        let _ = tx.send(AppEvent::UpdateCheck(notice)).await;
                                    });
                                }
                            }
                        }
                    };

                    let mut service = LeagueService::with_enabled_flag(
                        ClientLocator::default(),
                        enabled_for_service,
                        service_tx,
                    );
                    tokio::join!(bridge, service.run());
                });
            });
        });

    if let Err(error) = spawned {
        log::error!("could not spawn League service thread: {error}");
    }

    BackgroundService {
        enabled_flag,
        notifications: notification_tx,
        update_check: update_tx,
    }
}

/// Runs one worker, turning a panic into a report on the channel the UI waits on.
///
/// A panic here means the worker has stopped for good: nothing further will arrive on `tx` from it,
/// so the report is what ends the UI's wait instead of leaving it there. Containing the panic also
/// keeps the death reportable — an unwinding thread would leave the same dead channel behind with no
/// explanation and no log.
fn run_supervised(worker: &str, tx: &async_channel::Sender<AppEvent>, work: impl FnOnce()) {
    let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)) else {
        return;
    };
    let detail = panic_detail(payload.as_ref());
    log::error!("the {worker} stopped after a panic: {detail}");
    let message = format!(
        "The {worker} stopped unexpectedly ({detail}), so League activity is no longer being \
         monitored. Restart League Auto Accept to reconnect."
    );
    if tx
        .try_send(AppEvent::Service(ServiceEvent::Stopped { message }))
        .is_err()
    {
        log::error!("could not report that the {worker} stopped: the event channel is closed");
    }
}

/// The text of a caught panic payload, in either of the shapes `panic!` produces.
fn panic_detail(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The UI waits on `tx` for the next event, so a worker that panics must still answer there.
    /// Without the report that wait never ends — which is what the supervisor exists to prevent.
    #[tokio::test]
    async fn a_panicking_worker_wakes_the_ui_waiting_for_the_next_event() {
        let (tx, rx) = async_channel::unbounded::<AppEvent>();

        run_supervised("test worker", &tx, || panic!("boom"));

        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a panicked worker must not leave the UI waiting")
            .expect("the report arrives on the open channel");
        match event {
            AppEvent::Service(ServiceEvent::Stopped { message }) => {
                assert!(message.contains("test worker"), "{message}");
                assert!(message.contains("boom"), "{message}");
            }
            other => panic!("expected a stop report, got {other:?}"),
        }
    }

    /// Only a death is news: a worker that returns normally reports nothing.
    #[test]
    fn a_worker_that_returns_normally_reports_nothing() {
        let (tx, rx) = async_channel::unbounded::<AppEvent>();

        run_supervised("test worker", &tx, || {});

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_panic_payload_is_read_in_either_shape() {
        let borrowed: Box<dyn std::any::Any + Send> = Box::new("borrowed");
        assert_eq!(panic_detail(borrowed.as_ref()), "borrowed");

        let owned: Box<dyn std::any::Any + Send> = Box::new(String::from("owned"));
        assert_eq!(panic_detail(owned.as_ref()), "owned");
    }
}
