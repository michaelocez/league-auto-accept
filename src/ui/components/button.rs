//! `Button`: a token-styled action with hover/pressed/disabled states.

use std::rc::Rc;

use gpui::base::StyledExt as _;
use gpui::prelude::*;
use gpui::{
    div, App, ClickEvent, ElementId, IntoElement, Rgba, SharedString, StyleRefinement, Window,
};

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

        // Each variant names all of its states in one place, so a press is a real step past hover
        // rather than an opacity tweak that stacks with whatever is behind the button.
        let colors = variant_colors(self.variant, &t);
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
            .bg(colors.bg)
            .border_1()
            .border_color(colors.border)
            .rounded(theme::radius_md())
            .text_size(theme::text_body())
            .text_color(colors.fg)
            .font_weight(theme::weight_medium())
            .when(disabled, |this| this.opacity(0.5).cursor_default())
            .when(!disabled, |this| {
                this.cursor_pointer()
                    .hover(move |style| style.bg(colors.hover_bg).text_color(colors.hover_fg))
                    .active(move |style| style.bg(colors.active_bg))
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

/// One variant's fill, label and border in each state.
#[derive(Clone, Copy, Debug)]
struct ButtonColors {
    bg: Rgba,
    fg: Rgba,
    border: Rgba,
    hover_bg: Rgba,
    hover_fg: Rgba,
    active_bg: Rgba,
}

/// The colour half of a variant, kept as one pure mapping so a variant cannot be paired with a fill
/// from another state or another variant.
fn variant_colors(variant: ButtonVariant, t: &Theme) -> ButtonColors {
    match variant {
        ButtonVariant::Primary => ButtonColors {
            bg: t.accent,
            fg: t.accent_fg,
            // A filled button hides its hairline by making it the fill.
            border: t.accent,
            hover_bg: t.accent_hover,
            hover_fg: t.accent_fg,
            active_bg: t.accent_active,
        },
        // A secondary button is a bordered surface: on a surface of the same colour the border is
        // what identifies the control, so it takes the stronger border role. Against white that is
        // about 1.6:1 — still under WCAG's 3:1 for control boundaries, which would need a border
        // distinctly darker than this design's restrained separators.
        ButtonVariant::Secondary => ButtonColors {
            bg: t.surface,
            fg: t.text,
            border: t.border_strong,
            hover_bg: t.surface_hover,
            hover_fg: t.text,
            active_bg: t.surface_active,
        },
        // A ghost button carries no fill until it is pointed at, and its label steps up to full
        // contrast at the same moment the fill appears.
        ButtonVariant::Ghost => ButtonColors {
            bg: t.surface.with_alpha(0.0),
            fg: t.text_muted,
            border: t.surface.with_alpha(0.0),
            hover_bg: t.surface_hover,
            hover_fg: t.text,
            active_bg: t.surface_active,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every palette, since a pairing has to hold in light and dark and under a backdrop — the glass
    /// sets carry different alphas on the same colour roles.
    fn palettes() -> [Theme; 4] {
        [
            Theme::dark(),
            Theme::light(),
            Theme::dark_acrylic(),
            Theme::light_acrylic(),
        ]
    }

    fn same(a: Rgba, b: Rgba) -> bool {
        a.r == b.r && a.g == b.g && a.b == b.b && a.a == b.a
    }

    /// Base, hover and pressed must be three different colours: a press that equals its hover — or a
    /// hover that equals the base it covers — reads as no feedback at all.
    #[test]
    fn every_variant_has_three_distinct_fills() {
        for t in palettes() {
            for variant in [
                ButtonVariant::Primary,
                ButtonVariant::Secondary,
                ButtonVariant::Ghost,
            ] {
                let c = variant_colors(variant, &t);
                assert!(
                    !same(c.bg, c.hover_bg),
                    "{variant:?} hovers to its own base"
                );
                assert!(
                    !same(c.hover_bg, c.active_bg),
                    "{variant:?} presses to its own hover"
                );
            }
        }
    }

    /// A filled button's label must not change on hover: pairing a foreground with a fill from the
    /// wrong state is exactly what makes text vanish.
    #[test]
    fn primary_label_is_stable_across_states() {
        for t in palettes() {
            let c = variant_colors(ButtonVariant::Primary, &t);
            assert!(same(c.fg, t.accent_fg));
            assert!(same(c.hover_fg, c.fg));
        }
    }

    /// A ghost button is the one variant whose label does step up, and only because it gains a fill
    /// at the same moment.
    #[test]
    fn ghost_label_steps_up_only_with_its_fill() {
        for t in palettes() {
            let c = variant_colors(ButtonVariant::Ghost, &t);
            assert!(same(c.fg, t.text_muted));
            assert!(same(c.hover_fg, t.text));
            assert!(!same(c.hover_bg, c.bg));
        }
    }

    /// A secondary button rests on the surface it sits on and is identified by its border, so its
    /// base must not be the hover fill it steps to.
    #[test]
    fn secondary_rests_on_the_surface_it_borders() {
        for t in palettes() {
            let c = variant_colors(ButtonVariant::Secondary, &t);
            assert!(same(c.bg, t.surface));
            assert!(same(c.border, t.border_strong));
        }
    }
}
