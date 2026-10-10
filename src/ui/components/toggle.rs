//! `Toggle`: a token-styled on/off switch whose knob slides and whose colours interpolate via a
//! GPUI spring, so state changes read as motion rather than an instant flip. The spring also smooths
//! rapid toggles (momentum is preserved) and the animation respects the system reduce-motion setting.

use std::rc::Rc;
use std::sync::Arc;

use gpui::base::StyledExt as _;
use gpui::prelude::*;
use gpui::{
    div, px, AnimationExt as _, AnimationPhase, App, ClickEvent, ElementId, IntoElement,
    SpringAnimation, SpringConfig, StyleRefinement, Window,
};

use crate::ui::theme::Theme;

type ChangeHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

const TRACK_WIDTH: f32 = 44.0;
const TRACK_HEIGHT: f32 = 24.0;
const KNOB: f32 = 18.0;
const PAD: f32 = 3.0;
/// Horizontal travel of the knob between the off and on positions.
const TRAVEL: f32 = TRACK_WIDTH - KNOB - PAD * 2.0;

#[derive(IntoElement)]
pub struct Toggle {
    id: ElementId,
    style: StyleRefinement,
    checked: bool,
    disabled: bool,
    on_change: Option<ChangeHandler>,
}

impl Toggle {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            checked: false,
            disabled: false,
            on_change: None,
        }
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called when the switch is clicked. The caller flips its own state, since it already knows
    /// the current `checked` value it rendered.
    pub fn on_change(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl Styled for Toggle {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Toggle {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx);
        let checked = self.checked;
        let disabled = self.disabled;

        // A near-critically-damped spring: quick and smooth, with no visible overshoot.
        let spring = SpringConfig::new(180.0, 26.0, 1.0);
        let target = AnimationPhase(if checked { 1.0 } else { 0.0 });
        let track_anim = SpringAnimation::new(spring).to(target).with_epsilon(0.001);
        let knob_anim = SpringAnimation::new(spring).to(target).with_epsilon(0.001);

        let track_id = ElementId::NamedChild(Arc::new(self.id.clone()), "track".into());
        let knob_id = ElementId::NamedChild(Arc::new(self.id.clone()), "knob".into());

        let knob = div()
            .absolute()
            .top(px(PAD))
            .size(px(KNOB))
            .rounded_full()
            .with_spring(knob_id, knob_anim, move |el, phase: AnimationPhase| {
                el.left(phase.interpolate_between(0.0..=1.0, px(PAD), px(PAD + TRAVEL)))
                    .bg(phase.interpolate_between_clamped(0.0..=1.0, t.text_muted, t.accent_fg))
            });

        let track = div()
            .id(self.id)
            .relative()
            .w(px(TRACK_WIDTH))
            .h(px(TRACK_HEIGHT))
            .rounded_full()
            .border_1()
            .when(disabled, |this| this.opacity(0.5).cursor_default())
            .when(!disabled, |this| {
                this.cursor_pointer()
                    .hover(|style| style.opacity(0.92))
                    .active(|style| style.opacity(0.85))
                    .when_some(self.on_change, |this, handler| {
                        this.on_click(move |event, window, app| handler(event, window, app))
                    })
            })
            .child(knob)
            .refine_style(&self.style);

        track.with_spring(track_id, track_anim, move |el, phase: AnimationPhase| {
            el.bg(phase.interpolate_between_clamped(0.0..=1.0, t.surface_active, t.accent))
                .border_color(phase.interpolate_between_clamped(
                    0.0..=1.0,
                    t.border_strong,
                    t.accent,
                ))
        })
    }
}
