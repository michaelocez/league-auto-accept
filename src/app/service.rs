//! Background service bridge.
//!
//! Runs the headless `LeagueService` on a dedicated OS thread with its own tokio runtime and
//! forwards `ServiceEvent`s into the UI event channel. The UI never owns LCU credentials or
//! networking (AGENTS.md sec.13); it only receives application events.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::app::events::AppEvent;
use crate::league::discovery::ClientLocator;
use crate::league::service::{LeagueService, ServiceEvent};

/// Starts the League service and bridges its events to `tx`. Returns the shared enable flag so the
/// UI/tray can drive Auto Accept at runtime.
pub fn spawn_league_service(
    tx: async_channel::Sender<AppEvent>,
    auto_accept_enabled: bool,
) -> Arc<AtomicBool> {
    let enabled = Arc::new(AtomicBool::new(auto_accept_enabled));
    let enabled_for_service = enabled.clone();

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

                let bridge = {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        while let Ok(event) = service_rx.recv().await {
                            if tx.send(AppEvent::Service(event)).await.is_err() {
                                break;
                            }
                        }
                    })
                };

                let mut service = LeagueService::with_enabled_flag(
                    ClientLocator::default(),
                    enabled_for_service,
                    service_tx,
                );
                service.run().await;
                bridge.abort();
            });
        });

    if let Err(error) = spawned {
        log::error!("could not spawn League service thread: {error}");
    }
    enabled
}
