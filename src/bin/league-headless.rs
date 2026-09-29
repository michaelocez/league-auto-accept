//! Headless League Auto Accept service runner.
//!
//! Exercises the full headless pipeline (discovery -> verify -> connect -> observe -> state
//! machine -> accept) without GPUI. Intended for development and manual live-League testing;
//! the graphical application is the `league-auto-accept` binary.
//!
//! Run `league-headless --discover` to print a read-only report of League installations and
//! lockfile state (no credentials are printed, no connection is made).

use league_auto_accept::league::discovery::ClientLocator;
use league_auto_accept::league::lockfile::{parse_lockfile, MAX_LOCKFILE_BYTES};
use league_auto_accept::league::service::{LeagueService, ServiceEvent};

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    if std::env::args().any(|arg| arg == "--discover") {
        discover_report();
        return;
    }

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

/// Read-only diagnostic: list discovered installations and their lockfile state.
fn discover_report() {
    let locator = ClientLocator::default();
    let installs = locator.discover_install_paths();
    println!("discovered {} League installation(s):", installs.len());
    for install in &installs {
        let lockfile = install.join("lockfile");
        match std::fs::metadata(&lockfile) {
            Ok(metadata) if metadata.len() > MAX_LOCKFILE_BYTES => {
                println!(
                    "  {} -> lockfile present but too large ({} bytes)",
                    install.display(),
                    metadata.len()
                );
            }
            Ok(_) => match std::fs::read_to_string(&lockfile)
                .ok()
                .and_then(|contents| parse_lockfile(&contents, install))
            {
                Some(credentials) => println!(
                    "  {} -> running: pid {} port {} process {}",
                    install.display(),
                    credentials.process_id,
                    credentials.port,
                    credentials.process_name
                ),
                None => println!(
                    "  {} -> lockfile present but failed validation",
                    install.display()
                ),
            },
            Err(_) => println!(
                "  {} -> no lockfile (client not running)",
                install.display()
            ),
        }
    }
    if installs.is_empty() {
        println!("  (none)");
    }
}
