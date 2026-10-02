//! Reusable GPUI presentation primitives, styled exclusively from the design tokens.
//!
//! These are thin, original building blocks (not a component framework): each renders plain GPUI
//! elements and reads the active [`crate::ui::theme::Theme`]. Views compose them; they contain no
//! application logic.

mod button;
mod group;
mod row;
mod surface;
mod toggle;

pub use button::{Button, ButtonSize, ButtonVariant};
pub use group::SettingsGroup;
pub use row::Row;
pub use surface::{Divider, Elevation, Surface};
pub use toggle::Toggle;
