//! Headless League service: discovery → verify → connect → observe → state machine → accept.
//!
//! UI-independent: it reports through a `ServiceEvent` channel and never touches GPUI or the app
//! layer. Discord stays downstream (lifecycle events are plain data).
//!
//! Behaviour: 3-second discovery/reconnect polling, credential-change detection, one accept
//! request at a time, bounded retry, cancellation, and generation-guarded staleness.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::discovery::ClientLocator;
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

    pub fn enabled_flag(&self) -> Arc<AtomicBool> {
        self.enabled.clone()
    }

    /// Runs until the containing task is cancelled.
    pub async fn run(&mut self) {
        loop {
            self.run_once().await;
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    async fn run_once(&mut self) {
        let Some(credentials) = self.locator.find_running_client() else {
            self.disconnect();
            return;
        };

        self.emit(ServiceEvent::Connection { connected: false });

        let rest = Arc::new(LcuRestClient::new(credentials.clone()));
        if rest.verify().await.is_err() {
            self.disconnect();
            return;
        }

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
                    match self.locator.find_running_client() {
                        None => {
                            self.emit(ServiceEvent::Connection { connected: false });
                            break;
                        }
                        Some(next) if next.signature() != credentials.signature() => {
                            self.emit(ServiceEvent::Connection { connected: false });
                            break;
                        }
                        Some(_) => {}
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
