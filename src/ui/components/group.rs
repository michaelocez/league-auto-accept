//! `SettingsGroup`: an optional title, a bordered surface of [`super::Row`]s separated by inset
//! hairlines, and an optional footer caption. The macOS System Settings grouping shape.

use gpui::base::StyledExt as _;
use gpui::prelude::*;
use gpui::{div, px, AnyElement, App, Div, IntoElement, SharedString, StyleRefinement, Window};

use crate::ui::theme::{self, Theme};

#[derive(IntoElement)]
pub struct SettingsGroup {
    base: Div,
    style: StyleRefinement,
    title: Option<SharedString>,
    footer: Option<SharedString>,
    children: Vec<AnyElement>,
}

impl SettingsGroup {
    pub fn new() -> Self {
        Self {
            base: div(),
            style: StyleRefinement::default(),
            title: None,
            footer: None,
            children: Vec::new(),
        }
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn footer(mut self, footer: impl Into<SharedString>) -> Self {
        self.footer = Some(footer.into());
        self
    }
}

impl Default for SettingsGroup {
    fn default() -> Self {
        Self::new()
    }
}

impl Styled for SettingsGroup {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for SettingsGroup {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for SettingsGroup {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx);

        // Rows are separated by a hairline inset to align with the row label, and the last row is
        // flush — the shape every macOS settings pane uses.
        let mut rows: Vec<AnyElement> = Vec::with_capacity(self.children.len() * 2);
        for (index, child) in self.children.into_iter().enumerate() {
            if index > 0 {
                rows.push(
                    div()
                        .w_full()
                        .pl(theme::space_4())
                        .child(div().h(px(1.0)).w_full().bg(t.border))
                        .into_any_element(),
                );
            }
            rows.push(child);
        }

        self.base
            .flex()
            .flex_col()
            .gap(theme::space_2())
            .when_some(self.title, |this, title| {
                this.child(
                    div()
                        .px(px(2.0))
                        .text_size(theme::text_small())
                        .font_weight(theme::weight_medium())
                        .text_color(t.text_muted)
                        .child(title),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .bg(t.surface)
                    .border_1()
                    .border_color(t.border)
                    .rounded(theme::radius_lg())
                    .children(rows),
            )
            .when_some(self.footer, |this, footer| {
                this.child(
                    div()
                        .px(px(2.0))
                        .text_size(theme::text_caption())
                        .text_color(t.text_faint)
                        .child(footer),
                )
            })
            .refine_style(&self.style)
    }
}
