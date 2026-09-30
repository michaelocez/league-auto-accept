//! `Surface` container and `Divider`, styled from the design tokens.

use gpui::base::StyledExt as _;
use gpui::prelude::*;
use gpui::{div, px, AnyElement, App, Div, IntoElement, Rgba, StyleRefinement, Window};

use crate::ui::theme::{self, Theme};

/// Visual weight of a surface, from flat grouping to a floating layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Elevation {
    /// A flat grouping surface sitting on the page background.
    Base,
    /// A slightly lifted surface — the default for grouped content.
    #[default]
    Raised,
    /// A floating surface for overlays and popovers.
    Overlay,
}

/// A rounded, hairline-bordered container with a token-driven background.
#[derive(IntoElement)]
pub struct Surface {
    base: Div,
    style: StyleRefinement,
    elevation: Elevation,
    bg_override: Option<Rgba>,
    padded: bool,
    children: Vec<AnyElement>,
}

impl Surface {
    pub fn new() -> Self {
        Self {
            base: div(),
            style: StyleRefinement::default(),
            elevation: Elevation::Raised,
            bg_override: None,
            padded: true,
            children: Vec::new(),
        }
    }

    pub fn elevation(mut self, elevation: Elevation) -> Self {
        self.elevation = elevation;
        self
    }

    /// Overrides the token background (e.g. an accent-tinted hero surface).
    pub fn background(mut self, color: Rgba) -> Self {
        self.bg_override = Some(color);
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
        let background = self.bg_override.unwrap_or(match self.elevation {
            Elevation::Base => t.surface,
            Elevation::Raised => t.bg_elevated,
            Elevation::Overlay => t.overlay,
        });
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

/// Orientation of a [`Divider`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Orientation {
    #[default]
    Horizontal,
    Vertical,
}

/// A one-pixel hairline separator.
#[derive(IntoElement)]
pub struct Divider {
    orientation: Orientation,
}

impl Divider {
    pub fn new() -> Self {
        Self {
            orientation: Orientation::Horizontal,
        }
    }

    pub fn orientation(mut self, orientation: Orientation) -> Self {
        self.orientation = orientation;
        self
    }

    pub fn horizontal() -> Self {
        Self::new()
    }

    pub fn vertical() -> Self {
        Self::new().orientation(Orientation::Vertical)
    }
}

impl Default for Divider {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for Divider {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx);
        match self.orientation {
            Orientation::Horizontal => div().w_full().h(px(1.0)).bg(t.border),
            Orientation::Vertical => div().h_full().w(px(1.0)).bg(t.border),
        }
    }
}
