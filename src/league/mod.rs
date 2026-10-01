//! League Client (LCU) integration.
//!
//! The headless service core: lockfile parsing, installation discovery, endpoint
//! validation/request construction, WebSocket frame decoding and the ready-check /
//! auto-accept state machine. The constants below are the fixed set the application uses;
//! arbitrary endpoints are explicitly out of scope.

pub mod api;
pub mod discovery;
pub mod frame;
pub mod lockfile;
pub mod ready_check;
pub mod service;
pub mod transport;

/// Health/verification endpoint (`LcuClient.verify`).
pub const SUMMONER_ENDPOINT: &str = "/lol-summoner/v1/current-summoner";
/// Gameflow phase endpoint (bootstrap + events).
pub const GAMEFLOW_ENDPOINT: &str = "/lol-gameflow/v1/gameflow-phase";
/// Ready-check endpoint (bootstrap + events).
pub const READY_CHECK_ENDPOINT: &str = "/lol-matchmaking/v1/ready-check";
/// The one endpoint used for auto acceptance. Fixed by design.
pub const READY_CHECK_ACCEPT_ENDPOINT: &str = "/lol-matchmaking/v1/ready-check/accept";

/// The single subscription frame sent after the WebSocket handshake.
pub const EVENT_SUBSCRIPTION_FRAME: &str = "[5,\"OnJsonApiEvent\"]";

/// Username used for LCU HTTP Basic auth (password comes from the lockfile).
pub const LCU_USERNAME: &str = "riot";
