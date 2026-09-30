//! Reusable GPUI presentation primitives, styled exclusively from the design tokens.
//!
//! These are thin, original building blocks (not a component framework): each renders plain GPUI
//! elements and reads the active [`crate::ui::theme::Theme`]. Views compose them; they contain no
//! application logic.

mod button;
mod field;
mod section;
mod status;
mod surface;
mod toggle;

pub use button::{Button, ButtonSize, ButtonVariant};
pub use field::LabeledField;
pub use section::Section;
pub use status::StatusPill;
pub use surface::{Divider, Elevation, Orientation, Surface};
pub use toggle::Toggle;
