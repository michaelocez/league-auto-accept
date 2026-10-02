//! `Surface` container and `Divider`, styled from the design tokens.

use gpui::base::StyledExt as _;
use gpui::prelude::*;
use gpui::{div, px, AnyElement, App, Div, IntoElement, StyleRefinement, Window};

use crate::ui::theme::{self, Theme};

/// Visual weight of a surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Elevation {
    /// A flat grouping surface sitting on the page background.
    Base,
    /// A slightly lifted surface — the default for grouped content.
    #[default]
    Raised,
}

/// A rounded, hairline-bordered container with a token-driven background.
#[derive(IntoElement)]
pub struct Surface {
    base: Div,
    style: StyleRefinement,
    elevation: Elevation,
    padded: bool,
    children: Vec<AnyElement>,
}

impl Surface {
    pub fn new() -> Self {
        Self {
            base: div(),
            style: StyleRefinement::default(),
            elevation: Elevation::Raised,
            padded: true,
            children: Vec::new(),
        }
    }

    pub fn elevation(mut self, elevation: Elevation) -> Self {
        self.elevation = elevation;
        self
    }

    /// Whether the surface applies its own default padding (16px). Disable to lay out edge-to-edge.
    pub fn padded(mut self, padded: bool) -> Self {
        self.padded = padded;
        self
    }
}

impl Default for Surface {
    fn default() -> Self {
        Self::new()
    }
}

impl Styled for Surface {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for Surface {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Surface {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx);
        let background = match self.elevation {
            Elevation::Base => t.surface,
            Elevation::Raised => t.bg_elevated,
        };
        self.base
            .flex()
            .flex_col()
            .bg(background)
            .border_1()
            .border_color(t.border)
            .rounded(theme::radius_lg())
            .when(self.padded, |this| this.p_4())
            .children(self.children)
            .refine_style(&self.style)
    }
}

/// A one-pixel horizontal hairline separator.
#[derive(IntoElement)]
pub struct Divider;

impl Divider {
    pub fn horizontal() -> Self {
        Self
    }
}

impl RenderOnce for Divider {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx);
        div().w_full().h(px(1.0)).bg(t.border)
    }
}
