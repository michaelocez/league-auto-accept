//! `Row`: one setting inside a [`super::SettingsGroup`] — a label (+ optional description) on the
//! left, an optional control on the right, and optional full-width content beneath.

use gpui::prelude::*;
use gpui::{div, px, AnyElement, App, IntoElement, SharedString, Window};

use crate::ui::theme::{self, Theme};

#[derive(IntoElement)]
pub struct Row {
    label: SharedString,
    description: Option<SharedString>,
    control: Option<AnyElement>,
    children: Vec<AnyElement>,
}

impl Row {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            description: None,
            control: None,
            children: Vec::new(),
        }
    }

    /// Explanatory text under the label.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// A right-aligned control (a toggle, a button set, theme previews, …).
    pub fn control(mut self, control: impl IntoElement) -> Self {
        self.control = Some(control.into_any_element());
        self
    }
}

impl ParentElement for Row {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Row {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx);
        let has_children = !self.children.is_empty();

        div()
            .flex()
            .flex_col()
            .gap(theme::space_3())
            .px(theme::space_4())
            .py(theme::space_3())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme::space_4())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .text_size(theme::text_body())
                                    .text_color(t.text)
                                    .child(self.label),
                            )
                            .when_some(self.description, |this, description| {
                                this.child(
                                    div()
                                        .text_size(theme::text_small())
                                        .text_color(t.text_muted)
                                        .child(description),
                                )
                            }),
                    )
                    .when_some(self.control, |this, control| this.child(control)),
            )
            .when(has_children, |this| this.children(self.children))
    }
}
