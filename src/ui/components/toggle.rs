//! `Toggle`: a token-styled on/off switch with disabled state.

use std::rc::Rc;

use gpui::base::StyledExt as _;
use gpui::prelude::*;
use gpui::{div, px, App, ClickEvent, ElementId, IntoElement, StyleRefinement, Window};

use crate::ui::theme::Theme;

type ChangeHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

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

        let track = if checked { t.accent } else { t.surface_active };
        let knob = if checked { t.accent_fg } else { t.text_muted };
        let border = if checked { t.accent } else { t.border_strong };

        div()
            .id(self.id)
            .flex()
            .items_center()
            .when(checked, |this| this.justify_end())
            .w(px(44.0))
            .h(px(24.0))
            .px(px(3.0))
            .bg(track)
            .border_1()
            .border_color(border)
            .rounded_full()
            .when(disabled, |this| this.opacity(0.5).cursor_default())
            .when(!disabled, |this| {
                this.cursor_pointer()
                    .hover(|style| style.border_color(t.border_strong))
                    .when_some(self.on_change, |this, handler| {
                        this.on_click(move |event, window, app| handler(event, window, app))
                    })
            })
            .child(div().size(px(18.0)).bg(knob).rounded_full())
            .refine_style(&self.style)
    }
}
