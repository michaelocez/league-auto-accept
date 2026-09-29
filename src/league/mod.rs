//! League Client (LCU) integration.
//!
//! Phase 1 only proves the WebSocket transport (see `tests/lcu_ws_spike.rs`). Discovery,
//! authentication, the REST client and the ready-check state machine arrive in Phase 2
//! (AGENTS.md sec.10). The endpoint constants below are the fixed set from the reference
//! implementation (`Electron V1/src/main/auto-accept-controller.ts`); arbitrary endpoints
//! are explicitly out of scope.

/// Health/verification endpoint (`LcuClient.verify`).
pub const SUMMONER_ENDPOINT: &str = "/lol-summoner/v1/current-summoner";
/// Gameflow phase endpoint (bootstrap + events).
pub const GAMEFLOW_ENDPOINT: &str = "/lol-gameflow/v1/gameflow-phase";
/// Ready-check endpoint (bootstrap + events).
pub const READY_CHECK_ENDPOINT: &str = "/lol-matchmaking/v1/ready-check";
/// The one endpoint used for auto acceptance. Fixed by design.
pub const READY_CHECK_ACCEPT_ENDPOINT: &str = "/lol-matchmaking/v1/ready-check/accept";

/// The single subscription frame the reference implementation sends after the handshake.
pub const EVENT_SUBSCRIPTION_FRAME: &str = "[5,\"OnJsonApiEvent\"]";

/// Username used for LCU HTTP Basic auth (password comes from the lockfile).
pub const LCU_USERNAME: &str = "riot";
