//! `Button`: a token-styled action with hover/pressed/disabled states.

use std::rc::Rc;

use gpui::base::StyledExt as _;
use gpui::prelude::*;
use gpui::{div, App, ClickEvent, ElementId, IntoElement, SharedString, StyleRefinement, Window};

use crate::ui::theme::{self, Theme};

/// Semantic style of a button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    /// Filled accent — the single primary action on a page.
    #[default]
    Primary,
    /// Filled neutral — secondary actions.
    Secondary,
    /// Transparent until hovered — low-emphasis actions.
    Ghost,
}

/// Size of a button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonSize {
    Sm,
    #[default]
    Md,
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    style: StyleRefinement,
    label: SharedString,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            label: label.into(),
            variant: ButtonVariant::Primary,
            size: ButtonSize::Md,
            disabled: false,
            on_click: None,
        }
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl Styled for Button {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx);
        let disabled = self.disabled;

        // Each variant names all of its states, so a press is a real step past hover rather than an
        // opacity tweak that stacks with whatever happens to be behind the button.
        let (bg, fg, hover_bg, active_bg, hover_fg) = match self.variant {
            ButtonVariant::Primary => (
                t.accent,
                t.accent_fg,
                t.accent_hover,
                t.accent_active,
                t.accent_fg,
            ),
            ButtonVariant::Secondary => {
                (t.surface, t.text, t.surface_hover, t.surface_active, t.text)
            }
            // A ghost button carries no fill until it is pointed at, and its label steps up to full
            // contrast at the same moment the fill appears.
            ButtonVariant::Ghost => (
                t.surface.with_alpha(0.0),
                t.text_muted,
                t.surface_hover,
                t.surface_active,
                t.text,
            ),
        };
        let (px_x, py_y) = match self.size {
            ButtonSize::Sm => (theme::space_3(), theme::space_1()),
            ButtonSize::Md => (theme::space_4(), theme::space_2()),
        };

        div()
            .id(self.id)
            .flex()
            .items_center()
            .justify_center()
            .gap(theme::space_2())
            .px(px_x)
            .py(py_y)
            .bg(bg)
            .border_1()
            .border_color(match self.variant {
                ButtonVariant::Secondary => t.border,
                _ => bg,
            })
            .rounded(theme::radius_md())
            .text_size(theme::text_body())
            .text_color(fg)
            .font_weight(theme::weight_medium())
            .when(disabled, |this| this.opacity(0.5).cursor_default())
            .when(!disabled, |this| {
                this.cursor_pointer()
                    .hover(move |style| style.bg(hover_bg).text_color(hover_fg))
                    .active(move |style| style.bg(active_bg))
                    .when_some(self.on_click, |this, handler| {
                        this.on_click(move |event, window, app| handler(event, window, app))
                    })
            })
            .child(self.label)
            .refine_style(&self.style)
    }
}

// Small internal helper so ghost buttons can be fully transparent regardless of theme alpha.
trait WithAlpha {
    fn with_alpha(self, alpha: f32) -> Self;
}

impl WithAlpha for gpui::Rgba {
    fn with_alpha(mut self, alpha: f32) -> Self {
        self.a = alpha;
        self
    }
}
