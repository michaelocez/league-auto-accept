//! Headless League service: discovery → verify → connect → observe → state machine → accept.
//!
//! UI-independent: it reports through a `ServiceEvent` channel and never touches GPUI or the app
//! layer. Discord stays downstream (lifecycle events are plain data).
//!
//! Behaviour: 3-second discovery/reconnect polling, credential-change detection, one accept
//! request at a time, bounded retry, cancellation, and generation-guarded staleness.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::discovery::ClientLocator;
use super::lockfile::LcuCredentials;
use super::ready_check::{parse_lcu_event, Effect, LcuEvent, Lifecycle, ReadyCheckMachine, Status};
use super::transport::{LcuEventSocket, LcuRestClient, SocketEvent};

const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// Observable output of the service. The app maps these onto application state / notifications.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceEvent {
    Connection { connected: bool },
    ReadyCheck { status: Status, message: String },
    Lifecycle(Lifecycle),
}

enum MachineInput {
    TimerFired { generation: u64, timer: u64 },
    AcceptResult { generation: u64, ok: bool },
}

/// The state-machine half of a connection attempt: the ready-check machine plus the shared Auto
/// Accept flag.
///
/// Every transition reconciles the flag first, so a toggle is honoured at the next decision
/// instead of waiting for the discovery poll. Kept separate from the transport loop so the
/// connection lifecycle (connect, events, timer ticks, disconnect) is testable without an LCU.
struct MachineSession {
    machine: ReadyCheckMachine,
    enabled: Arc<AtomicBool>,
}

impl MachineSession {
    fn new(enabled: Arc<AtomicBool>) -> Self {
        let initial = enabled.load(Ordering::Relaxed);
        Self {
            machine: ReadyCheckMachine::new(initial),
            enabled,
        }
    }

    /// Applies a runtime Auto Accept change; a no-op while the flag is unchanged.
    fn sync_enabled(&mut self) -> Vec<Effect> {
        let wanted = self.enabled.load(Ordering::Relaxed);
        if wanted == self.machine.enabled() {
            Vec::new()
        } else {
            self.machine.set_enabled(wanted)
        }
    }

    fn on_connection(&mut self, connected: bool) -> Vec<Effect> {
        let mut out = self.machine.on_connection(connected);
        if connected {
            out.extend(self.sync_enabled());
        }
        out
    }

    fn on_ready_check(&mut self, state: &str, player_response: &str) -> Vec<Effect> {
        let mut out = self.sync_enabled();
        out.extend(self.machine.on_ready_check(state, player_response));
        out
    }

    fn on_gameflow_phase(&mut self, phase: &str, emit: bool) -> Vec<Effect> {
        let mut out = self.sync_enabled();
        out.extend(self.machine.on_gameflow_phase(phase, emit));
        out
    }

    /// The accept decision point: the live flag is read here, immediately before the machine
    /// decides whether to request an accept.
    fn on_timer_fired(&mut self, generation: u64, timer: u64) -> Vec<Effect> {
        let mut out = self.sync_enabled();
        out.extend(self.machine.on_timer_fired(generation, timer));
        out
    }

    fn on_accept_result(&mut self, generation: u64, ok: bool) -> Vec<Effect> {
        let mut out = self.sync_enabled();
        out.extend(self.machine.on_accept_result(generation, ok));
        out
    }
}

/// The installation the service is bound to, together with the REST client that proved it live.
struct BoundClient {
    credentials: LcuCredentials,
    rest: Arc<LcuRestClient>,
}

/// Picks the installation the service should talk to.
///
/// Installations are listed in a fixed order, but a lockfile on disk does not prove that a client is
/// running: only a health check does. So each candidate gets one turn at `verify()`, and the first
/// one to answer wins. This is the only place a client is chosen — the poll re-checks the
/// installation chosen here instead of running discovery again, because an earlier installation
/// whose client has exited still has a parseable lockfile and would otherwise evict a live one.
async fn select_client(candidates: &[LcuCredentials]) -> Option<BoundClient> {
    for credentials in candidates {
        let rest = Arc::new(LcuRestClient::new(credentials.clone()));
        if rest.verify().await.is_ok() {
            return Some(BoundClient {
                credentials: credentials.clone(),
                rest,
            });
        }
        log::debug!(
            "LCU at {} did not respond; trying the next installation",
            credentials.install_path.display()
        );
    }
    None
}

pub struct LeagueService {
    locator: ClientLocator,
    session: MachineSession,
    enabled: Arc<AtomicBool>,
    tx: async_channel::Sender<ServiceEvent>,
    /// The one scheduled accept timer, so a cancellation genuinely cancels it.
    timer_task: Option<tokio::task::JoinHandle<()>>,
}

impl LeagueService {
    pub fn new(
        locator: ClientLocator,
        enabled: bool,
        tx: async_channel::Sender<ServiceEvent>,
    ) -> Self {
        Self::with_enabled_flag(locator, Arc::new(AtomicBool::new(enabled)), tx)
    }

    /// Builds a service whose Auto Accept enable state can be changed at runtime via the shared
    /// flag (used by the UI/tray to drive the running service).
    pub fn with_enabled_flag(
        locator: ClientLocator,
        enabled: Arc<AtomicBool>,
        tx: async_channel::Sender<ServiceEvent>,
    ) -> Self {
        Self {
            locator,
            session: MachineSession::new(enabled.clone()),
            enabled,
            tx,
            timer_task: None,
        }
    }

    /// Runs until the containing task is cancelled.
    pub async fn run(&mut self) {
        loop {
            self.run_once().await;
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    async fn run_once(&mut self) {
        let candidates = self.locator.find_running_clients();
        if candidates.is_empty() {
            self.disconnect();
            return;
        }

        self.emit(ServiceEvent::Connection { connected: false });

        let Some(bound) = select_client(&candidates).await else {
            self.disconnect();
            return;
        };
        let BoundClient { credentials, rest } = bound;

        let mut socket = match LcuEventSocket::new(credentials.clone()).connect().await {
            Ok(socket) => socket,
            Err(error) => {
                log::warn!("LCU event connection failed: {error}");
                self.disconnect();
                return;
            }
        };

        let (input_tx, input_rx) = async_channel::unbounded::<MachineInput>();

        let effects = self.session.on_connection(true);
        self.handle_effects(effects, Some(&rest), Some(&input_tx));

        self.bootstrap(&rest, &input_tx).await;

        self.emit(ServiceEvent::Connection { connected: true });

        let mut poll = tokio::time::interval(POLL_INTERVAL);
        poll.tick().await; // consume the immediate first tick

        loop {
            tokio::select! {
                input = input_rx.recv() => {
                    if let Ok(input) = input {
                        let effects = match input {
                            MachineInput::TimerFired { generation, timer } => {
                                self.session.on_timer_fired(generation, timer)
                            }
                            MachineInput::AcceptResult { generation, ok } => {
                                self.session.on_accept_result(generation, ok)
                            }
                        };
                        self.handle_effects(effects, Some(&rest), Some(&input_tx));
                    }
                }
                event = socket.next_event() => {
                    match event {
                        Ok(SocketEvent::Text(text)) => {
                            if let Some(input) = parse_text_event(&text) {
                                let effects = match input {
                                    LcuEvent::ReadyCheck { state, player_response } => {
                                        self.session.on_ready_check(&state, &player_response)
                                    }
                                    LcuEvent::GameflowPhase { phase } => {
                                        self.session.on_gameflow_phase(&phase, true)
                                    }
                                };
                                self.handle_effects(effects, Some(&rest), Some(&input_tx));
                            }
                        }
                        Ok(SocketEvent::Closed) | Err(_) => {
                            self.emit(ServiceEvent::Connection { connected: false });
                            break;
                        }
                    }
                }
                _ = poll.tick() => {
                    let effects = self.session.sync_enabled();
                    self.handle_effects(effects, Some(&rest), Some(&input_tx));
                    // The connection is judged by the installation it was verified against. A failed
                    // read is not proof that the client exited: League rewrites its lockfile in
                    // place, so a read can land mid-write. A closed socket says so too, and the
                    // keepalive bounds how long a wedged one can hide that.
                    match connection_state(&self.locator, &credentials) {
                        ConnectionState::Live => {}
                        ConnectionState::MidWrite => {
                            log::debug!("lockfile unreadable but present; keeping the connection");
                        }
                        ConnectionState::Replaced | ConnectionState::Gone => {
                            self.emit(ServiceEvent::Connection { connected: false });
                            break;
                        }
                    }
                }
            }
        }

        let effects = self.session.on_connection(false);
        self.handle_effects(effects, Some(&rest), Some(&input_tx));
    }

    async fn bootstrap(
        &mut self,
        rest: &Arc<LcuRestClient>,
        input_tx: &async_channel::Sender<MachineInput>,
    ) {
        // Snapshots are best-effort; live events remain authoritative.
        if let Ok(phase) = rest.get_gameflow_phase().await {
            let effects = self.session.on_gameflow_phase(&phase, false);
            self.handle_effects(effects, Some(rest), Some(input_tx));
        }
        if let Ok(value) = rest.get_ready_check().await {
            let state = value.get("state").and_then(|v| v.as_str()).unwrap_or("");
            if !state.is_empty() {
                let player_response = value
                    .get("playerResponse")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let effects = self.session.on_ready_check(state, player_response);
                self.handle_effects(effects, Some(rest), Some(input_tx));
            }
        }
    }

    fn disconnect(&mut self) {
        let effects = self.session.on_connection(false);
        // No REST client is needed to unwind a connection; effects here are only state/idle.
        self.handle_effects(effects, None, None);
        self.emit(ServiceEvent::Connection { connected: false });
    }

    fn handle_effects(
        &mut self,
        effects: Vec<Effect>,
        rest: Option<&Arc<LcuRestClient>>,
        input_tx: Option<&async_channel::Sender<MachineInput>>,
    ) {
        for effect in effects {
            match effect {
                Effect::State(status, message) => {
                    self.emit(ServiceEvent::ReadyCheck { status, message });
                }
                Effect::Lifecycle(event) => {
                    self.emit(ServiceEvent::Lifecycle(event));
                }
                // Bootstrap is driven explicitly by `bootstrap`.
                Effect::Bootstrap { .. } => {}
                // The machine invalidates a cancelled timer by identity, so a tick that has
                // already been sent is ignored on arrival; aborting the sleeping task here
                // additionally avoids keeping it alive.
                Effect::CancelTimer => {
                    if let Some(task) = self.timer_task.take() {
                        task.abort();
                    }
                }
                Effect::ScheduleAccept {
                    generation,
                    timer,
                    delay_ms,
                } => {
                    let Some(input_tx) = input_tx else {
                        continue;
                    };
                    // Only one timer is ever pending; aborting a leftover handle is a guard
                    // against an unexpectedly overlapping schedule.
                    if let Some(previous) = self.timer_task.take() {
                        previous.abort();
                    }
                    let input_tx = input_tx.clone();
                    self.timer_task = Some(tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        let _ = input_tx
                            .send(MachineInput::TimerFired { generation, timer })
                            .await;
                    }));
                }
                Effect::Accept { generation } => {
                    let (Some(rest), Some(input_tx)) = (rest, input_tx) else {
                        log::warn!("cannot accept a ready check without an active LCU client");
                        continue;
                    };
                    let rest = rest.clone();
                    let input_tx = input_tx.clone();
                    let enabled = self.enabled.clone();
                    tokio::spawn(async move {
                        // Read the live flag at the last possible moment: a toggle that lands
                        // after the timer fired must still prevent the request.
                        let ok = enabled.load(Ordering::Relaxed) && rest.accept().await.is_ok();
                        let _ = input_tx
                            .send(MachineInput::AcceptResult { generation, ok })
                            .await;
                    });
                }
            }
        }
    }

    fn emit(&self, event: ServiceEvent) {
        let _ = self.tx.try_send(event);
    }
}

fn parse_text_event(text: &str) -> Option<LcuEvent> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    parse_lcu_event(&value)
}

/// What the connected installation's lockfile says about the live connection.
#[derive(Debug, PartialEq, Eq)]
enum ConnectionState {
    /// The same client is still there.
    Live,
    /// The lockfile parses to different credentials: the client restarted.
    Replaced,
    /// The lockfile is present but does not parse right now: League rewrites it in place.
    MidWrite,
    /// The lockfile is gone: the client exited.
    Gone,
}

/// Judges an established connection by the installation it was verified against.
///
/// Discovery order is deliberately not consulted. A second installation can be listed first with a
/// lockfile left behind by a client that has exited, and comparing the connection against that one
/// tore down a live client on every poll.
fn connection_state(locator: &ClientLocator, connected: &LcuCredentials) -> ConnectionState {
    match locator.client_at(&connected.install_path) {
        Some(current) if current.signature() == connected.signature() => ConnectionState::Live,
        Some(_) => ConnectionState::Replaced,
        None if lockfile_is_gone(&connected.install_path) => ConnectionState::Gone,
        None => ConnectionState::MidWrite,
    }
}

/// Whether a failed discovery read means a connection's client is gone.
///
/// A lockfile that exists but does not parse is a read that landed while League was rewriting it;
/// only a missing lockfile means the client this connection belongs to has exited.
fn lockfile_is_gone(install_path: &Path) -> bool {
    !install_path.join("lockfile").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lockfile_that_is_present_but_unreadable_keeps_the_connection() {
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("League of Legends");
        std::fs::create_dir_all(&install).unwrap();

        assert!(
            lockfile_is_gone(&install),
            "an absent lockfile means the client exited"
        );

        // Mid-rewrite: the file is there, so the client is still running even though a read that
        // lands now would parse as nothing.
        std::fs::write(install.join("lockfile"), "LeagueClientUx:1234").unwrap();
        assert!(!lockfile_is_gone(&install));

        std::fs::remove_file(install.join("lockfile")).unwrap();
        assert!(lockfile_is_gone(&install));
    }

    fn lockfile_text(port: u16, secret: &str) -> String {
        format!("LeagueClientUx:1234:{port}:{secret}:https")
    }

    /// Two installations in discovery order — the first with a lockfile whose client has exited, the
    /// second live — a layout that parsing a lockfile cannot tell apart from two live clients.
    ///
    /// Returns the locator and the credentials of the installation that would answer a health check.
    fn stale_then_live() -> (tempfile::TempDir, ClientLocator, LcuCredentials) {
        let root = tempfile::tempdir().unwrap();
        let stale = root.path().join("League of Legends");
        let live = root.path().join("Pbe");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(stale.join("lockfile"), lockfile_text(54321, "stale-secret")).unwrap();
        std::fs::write(live.join("lockfile"), lockfile_text(54322, "live-secret")).unwrap();

        let locator = ClientLocator::new(
            Some(root.path().join("empty-metadata")),
            vec![stale.clone(), live.clone()],
        );
        let connected = locator
            .find_running_clients()
            .into_iter()
            .find(|client| client.install_path == live)
            .expect("the live installation is listed");
        (root, locator, connected)
    }

    #[test]
    fn a_stale_installation_listed_first_does_not_tear_down_the_connected_one() {
        let (_root, locator, connected) = stale_then_live();

        // The connection is judged by the installation that answered the health check, which is
        // still running, so the poll leaves it alone.
        assert_eq!(
            connection_state(&locator, &connected),
            ConnectionState::Live
        );

        // Discovery, meanwhile, still offers the stale installation first: its lockfile parses even
        // though its client is gone, and comparing the connection against it is what used to end a
        // live connection on every poll.
        let first_listed = locator
            .find_running_client()
            .expect("a stale lockfile still parses");
        assert_ne!(first_listed.install_path, connected.install_path);
        assert_ne!(first_listed.signature(), connected.signature());
    }

    #[test]
    fn the_connection_follows_its_own_installation_through_a_rewrite_restart_and_exit() {
        let (_root, locator, connected) = stale_then_live();
        let lockfile = connected.install_path.join("lockfile");

        // League rewrites the lockfile in place: a read can land mid-write and parse as nothing.
        std::fs::write(&lockfile, "LeagueClientUx:1234").unwrap();
        assert_eq!(
            connection_state(&locator, &connected),
            ConnectionState::MidWrite
        );

        // The client restarted with different credentials.
        std::fs::write(&lockfile, lockfile_text(60000, "restarted-secret")).unwrap();
        assert_eq!(
            connection_state(&locator, &connected),
            ConnectionState::Replaced
        );

        // The client exited.
        std::fs::remove_file(&lockfile).unwrap();
        assert_eq!(
            connection_state(&locator, &connected),
            ConnectionState::Gone
        );

        // A rewrite that keeps the same credentials is still the same client.
        std::fs::write(&lockfile, lockfile_text(54322, "live-secret")).unwrap();
        assert_eq!(
            connection_state(&locator, &connected),
            ConnectionState::Live
        );
    }

    fn session(enabled: bool) -> (MachineSession, Arc<AtomicBool>) {
        let flag = Arc::new(AtomicBool::new(enabled));
        (MachineSession::new(flag.clone()), flag)
    }

    /// Returns `(generation, timer)` for the scheduled accept, if one was emitted.
    fn scheduled(effects: &[Effect]) -> Option<(u64, u64)> {
        effects.iter().find_map(|effect| match effect {
            Effect::ScheduleAccept {
                generation, timer, ..
            } => Some((*generation, *timer)),
            _ => None,
        })
    }

    fn accepted(effects: &[Effect]) -> Vec<u64> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Accept { generation } => Some(*generation),
                _ => None,
            })
            .collect()
    }

    fn lifecycle(effects: &[Effect]) -> Vec<Lifecycle> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Lifecycle(event) => Some(*event),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn disabling_before_the_timer_fires_suppresses_the_accept() {
        let (mut session, enabled) = session(true);
        session.on_connection(true);
        let (generation, timer) = scheduled(&session.on_ready_check("InProgress", "None"))
            .expect("a timer should be scheduled");

        // Auto Accept is turned off; the discovery poll has not run yet.
        enabled.store(false, Ordering::Relaxed);

        let effects = session.on_timer_fired(generation, timer);
        assert!(accepted(&effects).is_empty());
        assert!(effects.contains(&Effect::CancelTimer));
    }

    #[test]
    fn a_cancelled_timer_cannot_fire_after_re_enabling() {
        let (mut session, enabled) = session(true);
        session.on_connection(true);
        let (generation, stale_timer) = scheduled(&session.on_ready_check("InProgress", "None"))
            .expect("a timer should be scheduled");

        enabled.store(false, Ordering::Relaxed);
        session.on_timer_fired(generation, stale_timer);

        enabled.store(true, Ordering::Relaxed);
        let re_enabled = session.on_timer_fired(generation, stale_timer);
        assert!(
            accepted(&re_enabled).is_empty(),
            "the cancelled timer must not accept"
        );
        let (_, fresh_timer) =
            scheduled(&re_enabled).expect("re-enabling should arm a fresh timer");

        // Exactly two attempts across the cycle: the first, then the one bounded retry.
        assert_eq!(
            accepted(&session.on_timer_fired(generation, fresh_timer)).len(),
            1
        );
        let after_failure = session.on_accept_result(generation, false);
        let (_, retry_timer) = scheduled(&after_failure).expect("a retry should be scheduled");
        assert_eq!(
            accepted(&session.on_timer_fired(generation, retry_timer)).len(),
            1
        );
        let exhausted = session.on_accept_result(generation, false);
        assert!(scheduled(&exhausted).is_none());

        // The cancelled timer stays inert for the rest of the cycle.
        assert!(accepted(&session.on_timer_fired(generation, stale_timer)).is_empty());
    }

    #[test]
    fn reconnecting_discards_the_previous_cycle_and_its_timers() {
        let (mut session, _enabled) = session(true);
        session.on_connection(true);
        let (generation, timer) = scheduled(&session.on_ready_check("InProgress", "None"))
            .expect("a timer should be scheduled");

        // The League client restarts: the old connection drops and a new one is established.
        let dropped = session.on_connection(false);
        assert!(dropped.contains(&Effect::CancelTimer));
        let reconnected = session.on_connection(true);
        assert!(reconnected
            .iter()
            .any(|effect| matches!(effect, Effect::Bootstrap { .. })));

        // The pre-reconnect timer must not accept against the new generation.
        assert!(accepted(&session.on_timer_fired(generation, timer)).is_empty());

        // The bootstrap snapshot opens a fresh cycle with its own timer.
        let (new_generation, new_timer) = scheduled(&session.on_ready_check("InProgress", "None"))
            .expect("the new cycle should arm a timer");
        assert_ne!(new_generation, generation);
        assert_eq!(
            accepted(&session.on_timer_fired(new_generation, new_timer)).len(),
            1
        );
    }

    #[test]
    fn a_stale_accept_result_is_ignored_after_a_reconnect() {
        let (mut session, _enabled) = session(true);
        session.on_connection(true);
        let (generation, timer) = scheduled(&session.on_ready_check("InProgress", "None"))
            .expect("a timer should be scheduled");
        assert_eq!(
            accepted(&session.on_timer_fired(generation, timer)).len(),
            1
        );

        // The request is still in flight when the client reconnects.
        session.on_connection(false);
        session.on_connection(true);

        let late = session.on_accept_result(generation, true);
        assert!(
            late.is_empty(),
            "a result from a previous generation must not apply"
        );
        assert!(!lifecycle(&late).contains(&Lifecycle::AutoAccepted));
    }

    #[test]
    fn cancelling_while_an_accept_is_in_flight_starts_no_second_request() {
        let (mut session, enabled) = session(true);
        session.on_connection(true);
        let (generation, timer) = scheduled(&session.on_ready_check("InProgress", "None"))
            .expect("a timer should be scheduled");
        assert_eq!(
            accepted(&session.on_timer_fired(generation, timer)).len(),
            1
        );

        // Auto Accept is turned off while the request is in flight: the request already sent
        // is not retracted, but no further request may follow.
        enabled.store(false, Ordering::Relaxed);
        assert!(accepted(&session.on_timer_fired(generation, timer)).is_empty());

        let settled = session.on_accept_result(generation, true);
        assert!(lifecycle(&settled).contains(&Lifecycle::AutoAccepted));
    }

    #[test]
    fn disabling_stops_the_retry_of_a_failed_accept() {
        let (mut session, enabled) = session(true);
        session.on_connection(true);
        let (generation, timer) = scheduled(&session.on_ready_check("InProgress", "None"))
            .expect("a timer should be scheduled");
        session.on_timer_fired(generation, timer);

        enabled.store(false, Ordering::Relaxed);
        let failed = session.on_accept_result(generation, false);
        assert!(scheduled(&failed).is_none());
        assert!(failed
            .iter()
            .any(|effect| matches!(effect, Effect::State(Status::Error, _))));
    }

    #[test]
    fn enabling_mid_cycle_arms_the_accept_at_the_next_event() {
        let (mut session, enabled) = session(false);
        session.on_connection(true);
        assert!(scheduled(&session.on_ready_check("InProgress", "None")).is_none());

        // The user enables Auto Accept; the next event reconciles the flag and arms the timer.
        enabled.store(true, Ordering::Relaxed);
        let effects = session.on_gameflow_phase("ReadyCheck", true);
        let (generation, timer) = scheduled(&effects).expect("enabling should arm a timer");
        assert_eq!(
            accepted(&session.on_timer_fired(generation, timer)).len(),
            1
        );
    }
}
