//! Windows platform integration: tray, single-instance, window behaviour, process integrity,
//! and the version of Windows this process is running on.

pub mod glass;
pub(crate) mod integrity;
pub mod os_version;
pub mod popup;
pub mod single_instance;
pub mod tray;
pub mod window;
