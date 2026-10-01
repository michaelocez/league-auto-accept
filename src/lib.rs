//! League Auto Accept — a native Windows utility (Rust + GPUI).
//!
//! The crate is layered so the UI never touches credentials or networking:
//!
//! ```text
//! GPUI UI  <->  application state/actions  <->  background async service  <->  LCU / Discord
//! ```

pub mod app;
pub mod config;
pub mod league;
pub mod notifications;
pub mod platform;
pub mod ui;
