//! The one place a surface treatment is applied to a live window.
//!
//! Two things decide how a window is drawn, and they are set separately: the OS backdrop behind it
//! ([`crate::platform::glass`]) and the app's own theme, whose surfaces are translucent exactly when
//! the [`SurfaceTreatment`] global says so. Windows does not always accept a backdrop — Windows 10, a
//! DWM refusal, a remote session — and `glass` then leaves the *window* opaque while the theme would
//! stay translucent, so every fill composites against an opaque clear colour and the window looks
//! washed out.
//!
//! [`apply`] is therefore the single place that decides "applied, or fell back to `Opaque`": it asks
//! `glass`, settles the theme on what really took effect, and records a refusal at `warn` level. The
//! startup paths (the main window and the tray popup) and both of Settings' controls that re-apply a
//! treatment go through it, so they cannot drift apart.

use gpui::{App, Window};

use crate::platform::glass;
use crate::ui::decisions::effective_treatment;
use crate::ui::theme::{self, SurfaceTreatment};

/// Applies `requested` to `window`, leaves the active theme describing what really took effect, and
/// returns that treatment.
///
/// The user's saved choice is deliberately neither read nor written here: a refusal is an environment
/// fact, worth a `warn` record — it reaches the in-memory diagnostics tail without earning the
/// Diagnostics section — but not worth overwriting a choice a later OS may accept.
pub fn apply(window: &mut Window, cx: &mut App, requested: SurfaceTreatment) -> SurfaceTreatment {
    let applied = glass::apply(window, theme::current(), requested);
    let effective = effective_treatment(requested, applied);
    theme::set_treatment(cx, effective);
    if effective != requested {
        log::warn!(
            "Windows did not accept the {requested:?} window backdrop on this machine; using \
             {effective:?} for this session. The saved setting is unchanged."
        );
        // The theme changed under a window that is already drawn, so ask for a frame: this is what
        // makes the fallback visible now rather than at the next incidental repaint.
        window.refresh();
    }
    effective
}
