//! Reusable GPUI presentation primitives, styled exclusively from the design tokens.
//!
//! These are thin, original building blocks (not a component framework): each renders plain GPUI
//! elements and reads the active [`crate::ui::theme::Theme`]. Views compose them; they contain no
//! application logic.

mod button;
mod field;
mod group;
mod row;
mod section;
mod surface;
mod toggle;

pub use button::{Button, ButtonSize, ButtonVariant};
pub use field::LabeledField;
pub use group::SettingsGroup;
pub use row::Row;
pub use section::Section;
pub use surface::{Divider, Elevation, Surface};
pub use toggle::Toggle;
