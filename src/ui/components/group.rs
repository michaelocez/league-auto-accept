//! `SettingsGroup`: an optional title and one filled surface of [`super::Row`]s, separated by their
//! own padding rather than by hairlines, so the rows read as a single object.

use gpui::base::StyledExt as _;
use gpui::prelude::*;
use gpui::{div, AnyElement, App, Div, IntoElement, SharedString, StyleRefinement, Window};

use crate::ui::theme::{self, Theme};

#[derive(IntoElement)]
pub struct SettingsGroup {
    base: Div,
    style: StyleRefinement,
    title: Option<SharedString>,
    children: Vec<AnyElement>,
}

impl SettingsGroup {
    pub fn new() -> Self {
        Self {
            base: div(),
            style: StyleRefinement::default(),
            title: None,
            children: Vec::new(),
        }
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
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

        // No inset hairlines and no card border: the rows sit on one filled surface and are
        // separated by their own padding, so the group reads as a single object rather than a pane
        // sliced into strips.
        self.base
            .flex()
            .flex_col()
            .gap(theme::space_2())
            .when_some(self.title, |this, title| {
                this.child(
                    div()
                        .px(theme::space_half())
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
                    .rounded(theme::radius_lg())
                    .children(self.children),
            )
            .refine_style(&self.style)
    }
}
