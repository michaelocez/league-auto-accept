//! `LabeledField`: a form field — label, control, and helper/error text.

use gpui::base::StyledExt as _;
use gpui::prelude::*;
use gpui::{div, AnyElement, App, Div, IntoElement, SharedString, StyleRefinement, Window};

use crate::ui::theme::{self, Theme};

#[derive(IntoElement)]
pub struct LabeledField {
    base: Div,
    style: StyleRefinement,
    label: Option<SharedString>,
    helper: Option<SharedString>,
    error: Option<SharedString>,
    children: Vec<AnyElement>,
}

impl LabeledField {
    pub fn new() -> Self {
        Self {
            base: div(),
            style: StyleRefinement::default(),
            label: None,
            helper: None,
            error: None,
            children: Vec::new(),
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Helper text shown under the control (suppressed when an error is set).
    pub fn helper(mut self, helper: impl Into<SharedString>) -> Self {
        self.helper = Some(helper.into());
        self
    }

    /// Error text shown under the control. Takes precedence over the helper.
    pub fn error(mut self, error: impl Into<SharedString>) -> Self {
        self.error = Some(error.into());
        self
    }
}

impl Default for LabeledField {
    fn default() -> Self {
        Self::new()
    }
}

impl Styled for LabeledField {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for LabeledField {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for LabeledField {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx);
        let caption = self.error.clone().or_else(|| self.helper.clone());
        let caption_color = if self.error.is_some() {
            t.danger
        } else {
            t.text_faint
        };

        self.base
            .flex()
            .flex_col()
            .gap(theme::space_2())
            .when_some(self.label, |this, label| {
                this.child(
                    div()
                        .text_size(theme::text_small())
                        .font_weight(theme::weight_medium())
                        .text_color(t.text_muted)
                        .child(label),
                )
            })
            .children(self.children)
            .when_some(caption, |this, caption| {
                this.child(
                    div()
                        .text_size(theme::text_caption())
                        .text_color(caption_color)
                        .child(caption),
                )
            })
            .refine_style(&self.style)
    }
}
