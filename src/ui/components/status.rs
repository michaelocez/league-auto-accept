//! `StatusPill`: a compact state indicator (coloured dot + label).

use gpui::base::StyledExt as _;
use gpui::prelude::*;
use gpui::{div, px, App, FontWeight, IntoElement, Rgba, SharedString, StyleRefinement, Window};

use crate::ui::theme::{self, Theme};

#[derive(IntoElement)]
pub struct StatusPill {
    style: StyleRefinement,
    label: SharedString,
    color: Rgba,
    pulsing: bool,
}

impl StatusPill {
    pub fn new(label: impl Into<SharedString>, color: Rgba) -> Self {
        Self {
            style: StyleRefinement::default(),
            label: label.into(),
            color,
            pulsing: false,
        }
    }

    pub fn pulsing(mut self, pulsing: bool) -> Self {
        self.pulsing = pulsing;
        self
    }
}

impl Styled for StatusPill {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for StatusPill {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx);
        let dot = self.color;
        let halo = {
            let mut c = self.color;
            c.a = if self.pulsing { 0.35 } else { 0.0 };
            c
        };
        div()
            .flex()
            .items_center()
            .gap(theme::space_2())
            .px(theme::space_3())
            .py(px(4.0))
            .bg(t.surface_hover)
            .border_1()
            .border_color(t.border)
            .rounded_full()
            .child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(8.0))
                    .child(div().absolute().size(px(8.0)).rounded_full().bg(halo))
                    .child(div().size(px(7.0)).rounded_full().bg(dot)),
            )
            .child(
                div()
                    .text_size(theme::text_small())
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.text_muted)
                    .child(self.label),
            )
            .refine_style(&self.style)
    }
}
