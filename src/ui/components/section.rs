//! `Section`: a titled group of related controls.

use gpui::base::StyledExt as _;
use gpui::prelude::*;
use gpui::{div, AnyElement, App, Div, IntoElement, SharedString, StyleRefinement, Window};

use crate::ui::theme::{self, Theme};

#[derive(IntoElement)]
pub struct Section {
    base: Div,
    style: StyleRefinement,
    title: Option<SharedString>,
    subtitle: Option<SharedString>,
    children: Vec<AnyElement>,
}

impl Section {
    pub fn new() -> Self {
        Self {
            base: div(),
            style: StyleRefinement::default(),
            title: None,
            subtitle: None,
            children: Vec::new(),
        }
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }
}

impl Default for Section {
    fn default() -> Self {
        Self::new()
    }
}

impl Styled for Section {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for Section {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Section {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx);
        let has_header = self.title.is_some() || self.subtitle.is_some();
        let has_body = !self.children.is_empty();

        self.base
            .flex()
            .flex_col()
            .gap(theme::space_4())
            .when(has_header, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::space_1())
                        .when_some(self.title, |this, title| {
                            this.child(
                                div()
                                    .text_size(theme::text_small())
                                    .font_weight(theme::weight_semibold())
                                    .text_color(t.text_muted)
                                    .child(title),
                            )
                        })
                        .when_some(self.subtitle, |this, subtitle| {
                            this.child(
                                div()
                                    .text_size(theme::text_small())
                                    .text_color(t.text_faint)
                                    .child(subtitle),
                            )
                        }),
                )
            })
            .when(has_header && has_body, |this| {
                this.child(super::Divider::horizontal())
            })
            .when(has_body, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::space_3())
                        .children(self.children),
                )
            })
            .refine_style(&self.style)
    }
}
