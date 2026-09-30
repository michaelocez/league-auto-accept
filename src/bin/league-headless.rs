//! Headless League Auto Accept service runner.
//!
//! Exercises the full headless pipeline (discovery -> verify -> connect -> observe -> state
//! machine -> accept) without GPUI. Intended for development and manual live-League testing;
//! the graphical application is the `league-auto-accept` binary.
//!
//! Diagnostic modes (read-only; never accept a ready check):
//!   `league-headless --discover`      list installations and lockfile state
//!   `league-headless --observe [secs]` prove REST auth + WSS subscription + passive events

use std::time::{Duration, Instant};

use league_auto_accept::league::discovery::ClientLocator;
use league_auto_accept::league::lockfile::{parse_lockfile, MAX_LOCKFILE_BYTES};
use league_auto_accept::league::service::{LeagueService, ServiceEvent};
use league_auto_accept::league::transport::{LcuEventSocket, LcuRestClient, SocketEvent};

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--discover") {
        discover_report();
        return;
    }
    if let Some(seconds) = observe_seconds(&args) {
        observe(seconds).await;
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

fn observe_seconds(args: &[String]) -> Option<u64> {
    let mut seconds = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--observe" {
            seconds = Some(
                iter.next()
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(15),
            );
        } else if let Some(value) = arg.strip_prefix("--observe=") {
            seconds = Some(value.parse::<u64>().unwrap_or(15));
        }
    }
    seconds
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

/// Passive connectivity check: REST verify + WSS subscription + observed events.
/// Never sends an accept request.
async fn observe(seconds: u64) {
    let locator = ClientLocator::default();
    let Some(credentials) = locator.find_running_client() else {
        println!("no running League Client found (no valid lockfile)");
        return;
    };
    println!(
        "lockfile parsed: pid {} port {} process {}",
        credentials.process_id, credentials.port, credentials.process_name
    );

    let rest = LcuRestClient::new(credentials.clone());
    match rest.verify().await {
        Ok(()) => println!("REST verify GET /lol-summoner/v1/current-summoner: OK"),
        Err(error) => println!("REST verify: FAILED: {error}"),
    }
    match rest.get_gameflow_phase().await {
        Ok(phase) => println!("REST gameflow phase: {phase}"),
        Err(error) => println!("REST gameflow: FAILED: {error}"),
    }

    match LcuEventSocket::new(credentials).connect().await {
        Ok(mut socket) => {
            println!("WSS connected + subscribed to OnJsonApiEvent; observing {seconds}s (passive, no accept)…");
            let deadline = Instant::now() + Duration::from_secs(seconds);
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    println!("observation window ended");
                    break;
                }
                match tokio::time::timeout(remaining, socket.next_event()).await {
                    Ok(Ok(SocketEvent::Text(text))) => println!("event: {}", summarize(&text)),
                    Ok(Ok(SocketEvent::Closed)) => {
                        println!("WSS closed by peer");
                        break;
                    }
                    Ok(Err(error)) => {
                        println!("WSS error: {error}");
                        break;
                    }
                    Err(_) => {
                        println!("observation window ended");
                        break;
                    }
                }
            }
        }
        Err(error) => println!("WSS connect: FAILED: {error}"),
    }
}

fn summarize(text: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) => {
            if let Some(payload) = value.get(2) {
                let uri = payload.get("uri").and_then(|v| v.as_str()).unwrap_or("?");
                let event_type = payload
                    .get("eventType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let data = payload
                    .get("data")
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                let data = if data.chars().count() > 120 {
                    format!("{}…", data.chars().take(120).collect::<String>())
                } else {
                    data
                };
                format!("{uri} [{event_type}] {data}")
            } else {
                "unexpected event shape".into()
            }
        }
        Err(_) => format!("<non-JSON: {} bytes>", text.len()),
    }
}
