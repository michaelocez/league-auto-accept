//! Ready-check / auto-accept state machine.
//!
//! A faithful, side-effect-free port of the reference behaviour
//! (`Electron V1/src/main/auto-accept-controller.ts`). The machine makes no network calls and
//! reads no clock: it consumes events and emits `Effect`s, which the async service executes.
//! This keeps the subtle invariants (one accept at a time, duplicate coalescing, bounded retry,
//! generation-guarded staleness, cancellation) fully deterministic and unit-testable.

use super::{GAMEFLOW_ENDPOINT, READY_CHECK_ENDPOINT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Idle,
    Searching,
    Ready,
    Accepting,
    Accepted,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    QueuePopped,
    AutoAccepted,
    GameStarted,
}

/// Something the service must do in response to a state transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Publish a ready-check status to application state.
    State(Status, String),
    /// Emit a downstream lifecycle notification (Discord is downstream only).
    Lifecycle(Lifecycle),
    /// Perform the one-time snapshot bootstrap for a new connection.
    Bootstrap { generation: u64 },
    /// Schedule an accept attempt after `delay_ms` (cancels any pending timer first).
    ScheduleAccept { generation: u64, delay_ms: u64 },
    /// Cancel the pending accept timer.
    CancelTimer,
    /// Perform `POST /lol-matchmaking/v1/ready-check/accept` for `generation`.
    Accept { generation: u64 },
}

pub struct ReadyCheckMachine {
    enabled: bool,
    connected: bool,
    active: bool,
    accepted: bool,
    in_flight: bool,
    timer_pending: bool,
    attempts: u32,
    generation: u64,
    last_phase: Option<String>,
    max_attempts: u32,
    accept_delay_ms: u64,
    retry_delay_ms: u64,
    last_published: Option<(Status, String)>,
}

impl ReadyCheckMachine {
    pub fn new(enabled: bool) -> Self {
        Self::with_timings(enabled, 50, 250, 2)
    }

    pub fn with_timings(
        enabled: bool,
        accept_delay_ms: u64,
        retry_delay_ms: u64,
        max_attempts: u32,
    ) -> Self {
        Self {
            enabled,
            connected: false,
            active: false,
            accepted: false,
            in_flight: false,
            timer_pending: false,
            attempts: 0,
            generation: 0,
            last_phase: None,
            max_attempts,
            accept_delay_ms,
            retry_delay_ms,
            last_published: None,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn set_enabled(&mut self, enabled: bool) -> Vec<Effect> {
        let mut out = Vec::new();
        if self.enabled == enabled {
            return out;
        }
        self.enabled = enabled;
        if !enabled {
            self.cancel_timer(&mut out);
            if self.active && !self.accepted && !self.in_flight {
                self.publish(
                    Status::Ready,
                    "Ready check detected; Auto Accept is off.".into(),
                    &mut out,
                );
            }
            return out;
        }
        if self.connected && self.active && !self.accepted {
            self.schedule_accept(self.accept_delay_ms, &mut out);
        }
        out
    }

    pub fn on_connection(&mut self, connected: bool) -> Vec<Effect> {
        let mut out = Vec::new();
        if self.connected == connected {
            return out;
        }
        self.connected = connected;
        self.generation += 1;
        if !connected {
            self.reset(&mut out);
            self.last_phase = None;
            self.publish_idle(&mut out);
        } else {
            self.cancel_timer(&mut out);
            out.push(Effect::Bootstrap {
                generation: self.generation,
            });
        }
        out
    }

    /// Feed a ready-check event/snapshot. `state` and `player_response` are the raw LCU values.
    pub fn on_ready_check(&mut self, state: &str, player_response: &str) -> Vec<Effect> {
        let mut out = Vec::new();
        let state = state.to_ascii_lowercase();
        if state != "inprogress" {
            if self.active {
                self.reset(&mut out);
                self.publish_idle(&mut out);
            }
            return out;
        }

        if !self.active {
            self.generation += 1;
            self.active = true;
            self.accepted = false;
            self.in_flight = false;
            self.timer_pending = false;
            self.attempts = 0;
            out.push(Effect::Lifecycle(Lifecycle::QueuePopped));
        }

        if player_response.eq_ignore_ascii_case("accepted") {
            self.accepted = true;
            self.cancel_timer(&mut out);
            self.publish(Status::Accepted, "Ready check accepted.".into(), &mut out);
            return out;
        }
        if self.accepted || self.in_flight {
            return out;
        }

        let message = if self.enabled {
            "Ready check detected; accepting…"
        } else {
            "Ready check detected; Auto Accept is off."
        };
        self.publish(Status::Ready, message.into(), &mut out);
        if self.enabled && self.connected {
            self.schedule_accept(self.accept_delay_ms, &mut out);
        }
        out
    }

    /// Feed a gameflow-phase event. `emit` is false for the initial bootstrap snapshot.
    pub fn on_gameflow_phase(&mut self, phase: &str, emit: bool) -> Vec<Effect> {
        let mut out = Vec::new();
        let phase = phase.to_ascii_lowercase();
        if phase == "inprogress" && self.last_phase.as_deref() != Some("inprogress") && emit {
            out.push(Effect::Lifecycle(Lifecycle::GameStarted));
        }
        self.last_phase = Some(phase.clone());

        let in_queue_flow = phase == "readycheck" || phase == "matchmaking";
        if !in_queue_flow && self.active {
            self.reset(&mut out);
        }
        if phase == "matchmaking" && !self.active {
            self.publish(Status::Searching, "Searching for a match.".into(), &mut out);
        } else if !in_queue_flow && !self.active {
            self.publish_idle(&mut out);
        }
        out
    }

    /// Called by the service when a scheduled accept timer fires.
    pub fn on_timer_fired(&mut self, generation: u64) -> Vec<Effect> {
        let mut out = Vec::new();
        if generation != self.generation {
            return out;
        }
        self.timer_pending = false;
        if !self.enabled || !self.connected || !self.active || self.accepted || self.in_flight {
            return out;
        }
        self.in_flight = true;
        self.attempts += 1;
        self.publish(Status::Accepting, "Accepting ready check…".into(), &mut out);
        out.push(Effect::Accept { generation });
        out
    }

    /// Called by the service after the accept request settles.
    pub fn on_accept_result(&mut self, generation: u64, ok: bool) -> Vec<Effect> {
        let mut out = Vec::new();
        if generation != self.generation {
            return out;
        }
        self.in_flight = false;
        if ok {
            self.accepted = true;
            self.publish(
                Status::Accepted,
                "Ready check auto accepted.".into(),
                &mut out,
            );
            out.push(Effect::Lifecycle(Lifecycle::AutoAccepted));
            return out;
        }
        if self.enabled && self.connected && self.active && self.attempts < self.max_attempts {
            self.publish(
                Status::Ready,
                "Accept attempt failed; retrying once…".into(),
                &mut out,
            );
            self.schedule_accept(self.retry_delay_ms, &mut out);
        } else {
            self.publish(
                Status::Error,
                "Could not auto accept this ready check.".into(),
                &mut out,
            );
        }
        out
    }

    fn reset(&mut self, out: &mut Vec<Effect>) {
        self.generation += 1;
        self.cancel_timer(out);
        self.active = false;
        self.accepted = false;
        self.in_flight = false;
        self.timer_pending = false;
        self.attempts = 0;
    }

    fn cancel_timer(&mut self, out: &mut Vec<Effect>) {
        if self.timer_pending {
            self.timer_pending = false;
            out.push(Effect::CancelTimer);
        }
    }

    fn schedule_accept(&mut self, delay_ms: u64, out: &mut Vec<Effect>) {
        if self.timer_pending || self.in_flight || self.accepted || !self.active {
            return;
        }
        self.timer_pending = true;
        out.push(Effect::ScheduleAccept {
            generation: self.generation,
            delay_ms,
        });
    }

    fn publish_idle(&mut self, out: &mut Vec<Effect>) {
        self.publish(Status::Idle, "Waiting for a ready check.".into(), out);
    }

    fn publish(&mut self, status: Status, message: String, out: &mut Vec<Effect>) {
        if self.last_published.as_ref() == Some(&(status, message.clone())) {
            return;
        }
        self.last_published = Some((status, message.clone()));
        out.push(Effect::State(status, message));
    }
}

/// Interpret an LCU JSON API event into machine inputs. Returns `None` for unrelated events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LcuEvent {
    ReadyCheck {
        state: String,
        player_response: String,
    },
    GameflowPhase {
        phase: String,
    },
}

/// Parses the LCU `[8, "OnJsonApiEvent", {uri,eventType,data}]` envelope into a machine event.
pub fn parse_lcu_event(value: &serde_json::Value) -> Option<LcuEvent> {
    let payload = value.get(2)?;
    let uri = payload.get("uri")?.as_str()?;
    let event_type = payload
        .get("eventType")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let data = payload.get("data")?;

    if uri == READY_CHECK_ENDPOINT {
        if event_type.eq_ignore_ascii_case("delete") {
            return Some(LcuEvent::ReadyCheck {
                state: "Completed".into(),
                player_response: String::new(),
            });
        }
        let object = data.as_object()?;
        let state = object
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let player_response = object
            .get("playerResponse")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Some(LcuEvent::ReadyCheck {
            state,
            player_response,
        })
    } else if uri == GAMEFLOW_ENDPOINT {
        let phase = data.as_str()?.to_string();
        Some(LcuEvent::GameflowPhase { phase })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepts(effects: &[Effect]) -> Option<u64> {
        effects.iter().find_map(|effect| match effect {
            Effect::Accept { generation } => Some(*generation),
            _ => None,
        })
    }

    fn schedules(effects: &[Effect]) -> Option<u64> {
        effects.iter().find_map(|effect| match effect {
            Effect::ScheduleAccept { generation, .. } => Some(*generation),
            _ => None,
        })
    }

    fn connected_machine(enabled: bool) -> ReadyCheckMachine {
        let mut machine = ReadyCheckMachine::new(enabled);
        machine.on_connection(true);
        machine
    }

    #[test]
    fn coalesces_duplicate_ready_check_events_into_one_accept() {
        let mut machine = connected_machine(true);
        let first = machine.on_ready_check("InProgress", "None");
        assert!(schedules(&first).is_some());
        // Duplicate events while a timer is pending must not schedule another accept.
        assert!(schedules(&machine.on_ready_check("InProgress", "None")).is_none());
        assert!(schedules(&machine.on_ready_check("InProgress", "None")).is_none());

        let generation = machine.generation();
        assert_eq!(
            accepts(&machine.on_timer_fired(generation)),
            Some(generation)
        );
        // A second timer fire for the same generation must not accept twice.
        assert!(accepts(&machine.on_timer_fired(generation)).is_none());
        let result = machine.on_accept_result(generation, true);
        assert!(result.contains(&Effect::Lifecycle(Lifecycle::AutoAccepted)));
    }

    #[test]
    fn does_not_accept_while_disabled_but_accepts_when_enabled_mid_cycle() {
        let mut machine = connected_machine(false);
        assert!(schedules(&machine.on_ready_check("InProgress", "None")).is_none());
        let enabling = machine.set_enabled(true);
        assert!(schedules(&enabling).is_some());
    }

    #[test]
    fn disabling_cancels_a_scheduled_accept() {
        let mut machine = connected_machine(true);
        machine.on_ready_check("InProgress", "None");
        let disabling = machine.set_enabled(false);
        assert!(disabling.contains(&Effect::CancelTimer));
        assert!(accepts(&machine.on_timer_fired(machine.generation())).is_none());
    }

    #[test]
    fn disconnecting_cancels_and_returns_to_idle() {
        let mut machine = connected_machine(true);
        machine.on_ready_check("InProgress", "None");
        let effects = machine.on_connection(false);
        assert!(effects.contains(&Effect::CancelTimer));
        assert!(accepts(&machine.on_timer_fired(machine.generation())).is_none());
        assert!(!machine.active);
    }

    #[test]
    fn does_not_post_when_the_player_already_accepted() {
        let mut machine = connected_machine(true);
        let effects = machine.on_ready_check("InProgress", "Accepted");
        assert!(schedules(&effects).is_none());
        assert!(effects
            .iter()
            .any(|e| matches!(e, Effect::State(Status::Accepted, _))));
    }

    #[test]
    fn retries_a_failed_request_once_and_stops_at_the_bound() {
        let mut machine = connected_machine(true);
        machine.on_ready_check("InProgress", "None");

        // Attempt 1 fails -> schedule retry.
        let generation = machine.generation();
        machine.on_timer_fired(generation);
        let after_first = machine.on_accept_result(generation, false);
        assert!(schedules(&after_first).is_some());

        // Attempt 2 fails -> error, no further retry.
        machine.on_timer_fired(generation);
        let after_second = machine.on_accept_result(generation, false);
        assert!(schedules(&after_second).is_none());
        assert!(after_second
            .iter()
            .any(|e| matches!(e, Effect::State(Status::Error, _))));
    }

    #[test]
    fn succeeds_on_retry() {
        let mut machine = connected_machine(true);
        machine.on_ready_check("InProgress", "None");
        let generation = machine.generation();
        machine.on_timer_fired(generation);
        machine.on_accept_result(generation, false);
        machine.on_timer_fired(generation);
        let result = machine.on_accept_result(generation, true);
        assert!(result.contains(&Effect::Lifecycle(Lifecycle::AutoAccepted)));
    }

    #[test]
    fn starts_a_fresh_cycle_only_after_the_previous_ready_check_ends() {
        let mut machine = connected_machine(true);
        machine.on_ready_check("InProgress", "None");
        let gen = machine.generation();
        machine.on_timer_fired(gen);
        machine.on_accept_result(gen, true);

        // Still the same cycle: no new queuePopped.
        assert!(!machine
            .on_ready_check("InProgress", "None")
            .contains(&Effect::Lifecycle(Lifecycle::QueuePopped)));

        // Completed ends the cycle; a new InProgress starts a fresh one.
        machine.on_ready_check("Completed", "");
        let new_cycle = machine.on_ready_check("InProgress", "None");
        assert!(new_cycle.contains(&Effect::Lifecycle(Lifecycle::QueuePopped)));
        assert!(schedules(&new_cycle).is_some());
    }

    #[test]
    fn a_delete_event_without_data_ends_the_cycle() {
        let mut machine = connected_machine(true);
        machine.on_ready_check("InProgress", "None");
        let gen = machine.generation();
        machine.on_timer_fired(gen);
        machine.on_accept_result(gen, true);
        machine.on_ready_check("Completed", "");
        assert!(machine
            .on_ready_check("InProgress", "None")
            .contains(&Effect::Lifecycle(Lifecycle::QueuePopped)));
    }

    #[test]
    fn emits_each_lifecycle_notification_once() {
        let mut machine = connected_machine(true);
        machine.on_ready_check("InProgress", "None");
        machine.on_ready_check("InProgress", "None");
        let gen = machine.generation();
        machine.on_timer_fired(gen);
        machine.on_accept_result(gen, true);
        machine.on_gameflow_phase("InProgress", true);
        machine.on_gameflow_phase("InProgress", true);

        // Count across the whole run.
        let mut lifecycle = Vec::new();
        let mut m2 = connected_machine(true);
        lifecycle.extend(collect_lifecycle(m2.on_ready_check("InProgress", "None")));
        lifecycle.extend(collect_lifecycle(m2.on_ready_check("InProgress", "None")));
        let g = m2.generation();
        m2.on_timer_fired(g);
        lifecycle.extend(collect_lifecycle(m2.on_accept_result(g, true)));
        lifecycle.extend(collect_lifecycle(m2.on_gameflow_phase("InProgress", true)));
        lifecycle.extend(collect_lifecycle(m2.on_gameflow_phase("InProgress", true)));
        assert_eq!(
            lifecycle,
            vec![
                Lifecycle::QueuePopped,
                Lifecycle::AutoAccepted,
                Lifecycle::GameStarted
            ]
        );
    }

    fn collect_lifecycle(effects: Vec<Effect>) -> Vec<Lifecycle> {
        effects
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::Lifecycle(event) => Some(event),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn parses_lcu_events_including_delete_as_completed() {
        let ready = serde_json::json!([
            8,
            "OnJsonApiEvent",
            {"uri": READY_CHECK_ENDPOINT, "eventType": "Update", "data": {"state": "InProgress", "playerResponse": "None"}}
        ]);
        match parse_lcu_event(&ready) {
            Some(LcuEvent::ReadyCheck {
                state,
                player_response,
            }) => {
                assert_eq!(state, "InProgress");
                assert_eq!(player_response, "None");
            }
            other => panic!("unexpected: {other:?}"),
        }

        let deleted = serde_json::json!([
            8,
            "OnJsonApiEvent",
            {"uri": READY_CHECK_ENDPOINT, "eventType": "Delete", "data": null}
        ]);
        match parse_lcu_event(&deleted) {
            Some(LcuEvent::ReadyCheck { state, .. }) => assert_eq!(state, "Completed"),
            other => panic!("unexpected: {other:?}"),
        }

        let phase = serde_json::json!([
            8,
            "OnJsonApiEvent",
            {"uri": GAMEFLOW_ENDPOINT, "eventType": "Update", "data": "InProgress"}
        ]);
        match parse_lcu_event(&phase) {
            Some(LcuEvent::GameflowPhase { phase }) => assert_eq!(phase, "InProgress"),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
