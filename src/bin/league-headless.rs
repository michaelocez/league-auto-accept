//! Headless League Auto Accept service runner.
//!
//! Exercises the full headless pipeline (discovery → verify → connect → observe → state
//! machine → accept) without GPUI. Intended for development and manual live-League testing;
//! the graphical application is the `league-auto-accept` binary.

use league_auto_accept::league::discovery::ClientLocator;
use league_auto_accept::league::service::{LeagueService, ServiceEvent};

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let (tx, rx) = async_channel::unbounded::<ServiceEvent>();
    tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            log::info!("{event:?}");
        }
    });

    log::info!("starting headless League Auto Accept service (auto-accept enabled)");
    let mut service = LeagueService::new(ClientLocator::default(), true, tx);
    service.run().await;
}
