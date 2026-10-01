//! Notifications — the only mechanism is a user-configured Discord webhook (AGENTS.md sec.22).
//! Windows toasts are explicitly out of scope. Delivery is strictly downstream of Auto Accept.

pub mod discord;
pub mod service;
