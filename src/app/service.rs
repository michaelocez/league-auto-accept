//! Spike C: a background async service on its own thread + tokio runtime, bridging to the UI
//! only through an `async_channel`.
//!
//! No LCU credentials, networking or GPUI types exist here. This is the shape the real League
//! service will follow in Phase 2.

use std::time::{Duration, Instant};

use crate::app::events::AppEvent;

/// Starts a stub backend that emits a heartbeat every second and a one-off connection event.
/// Returns immediately; the service runs on a dedicated OS thread with its own tokio runtime.
pub fn spawn_stub_backend(tx: async_channel::Sender<AppEvent>) {
    std::thread::Builder::new()
        .name("laa-backend".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
            {
                Ok(rt) => rt,
                Err(error) => {
                    log::error!("failed to build backend runtime: {error}");
                    return;
                }
            };

            runtime.block_on(async move {
                let start = Instant::now();
                let mut announced = false;
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let uptime = start.elapsed().as_secs();
                    if !announced {
                        announced = true;
                        if tx
                            .send(AppEvent::Connection {
                                connected: true,
                                message: "League Client connected.".to_string(),
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    if tx
                        .send(AppEvent::BackendTick {
                            uptime_secs: uptime,
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
        })
        .expect("failed to spawn backend thread");
}
