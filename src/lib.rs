//! League Auto Accept — Rust + GPUI rewrite.
//!
//! Phase 1 architecture proof. This crate currently contains the spike scaffolding that
//! demonstrates the intended layering:
//!
//! ```text
//! GPUI UI  <->  application state/actions  <->  background async service  <->  LCU/networking
//! ```
//!
//! The full League service, ready-check state machine, settings system and Discord
//! notifications are intentionally **not** implemented yet (Phase 2+).

pub mod app;
pub mod config;
pub mod league;
pub mod notifications;
pub mod platform;
pub mod ui;
