//! Windows platform integration: tray, single-instance, window behaviour, process integrity.

pub mod glass;
pub(crate) mod integrity;
pub mod popup;
pub mod single_instance;
pub mod tray;
pub mod window;
