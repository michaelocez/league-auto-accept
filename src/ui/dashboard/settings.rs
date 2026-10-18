//! The Settings screen: window behaviour, appearance and the backdrop treatment.

use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context};
use serde_json::json;

use crate::config::settings::ThemeMode;
use crate::platform::glass;
use crate::ui::components::{Row, SettingsGroup};
use crate::ui::decisions::ToggleSetting;
use crate::ui::theme::{self, Appearance, SurfaceTreatment, Theme};

use super::Dashboard;

impl Dashboard {
    pub(super) fn render_settings(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::of(cx);
        let appearance = theme::current();
        let treatment = theme::current_treatment();
        let minimize = self.state.settings.minimize_to_tray;

        let window = SettingsGroup::new().title("Window").child(Self::toggle_row(
            "toggle-minimize",
            "Close to tray",
            "Keep running in the system tray when the window is closed.",
            minimize,
            ToggleSetting::MinimizeToTray,
            cx,
        ));

        let theme_control = div()
            .flex()
            .gap(theme::space_3())
            .child(self.appearance_option(
                "theme-dark",
                Appearance::Dark,
                appearance == Appearance::Dark,
                cx,
            ))
            .child(self.appearance_option(
                "theme-light",
                Appearance::Light,
                appearance == Appearance::Light,
                cx,
            ));

        let backdrop_control = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .p(px(2.0))
            .bg(t.surface_active)
            .border_1()
            .border_color(t.border)
            .rounded(theme::radius_md())
            .child(self.backdrop_option(
                "backdrop-opaque",
                SurfaceTreatment::Opaque,
                treatment == SurfaceTreatment::Opaque,
                cx,
            ))
            .child(self.backdrop_option(
                "backdrop-mica",
                SurfaceTreatment::Mica,
                treatment == SurfaceTreatment::Mica,
                cx,
            ))
            .child(self.backdrop_option(
                "backdrop-acrylic",
                SurfaceTreatment::Acrylic,
                treatment == SurfaceTreatment::Acrylic,
                cx,
            ));

        let appearance_group = SettingsGroup::new()
            .title("Appearance")
            .child(Row::new("Theme").control(theme_control))
            .child(
                Row::new("Window backdrop")
                    .description(
                        "Mica tints from the desktop wallpaper; Acrylic blurs what is behind the \
                         window in real time. Falls back to opaque where unsupported.",
                    )
                    .control(backdrop_control),
            );

        div()
            .flex()
            .flex_col()
            .gap(theme::space_6())
            .child(window)
            .child(appearance_group)
            .into_any_element()
    }

    /// A backdrop choice rendered as one segment of a segmented control.
    fn backdrop_option(
        &self,
        id: &'static str,
        value: SurfaceTreatment,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = Theme::of(cx);
        let label = match value {
            SurfaceTreatment::Opaque => "Opaque",
            SurfaceTreatment::Mica => "Mica",
            SurfaceTreatment::Acrylic => "Acrylic",
        };
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .px(theme::space_3())
            .py(theme::space_1())
            .rounded(theme::radius_sm())
            .cursor_pointer()
            .when(selected, |this| this.bg(t.accent).text_color(t.accent_fg))
            .when(!selected, |this| this.text_color(t.text_muted))
            .hover(move |style| {
                if selected {
                    style
                } else {
                    style.text_color(t.text)
                }
            })
            .on_click(cx.listener(move |this, _event: &ClickEvent, window, cx| {
                this.set_treatment(window, cx, value);
            }))
            .child(
                div()
                    .text_size(theme::text_small())
                    .font_weight(theme::weight_medium())
                    .child(label),
            )
            .into_any_element()
    }

    /// A theme choice rendered as a mini window preview in that theme, so the effect is visible
    /// before it is applied.
    fn appearance_option(
        &self,
        id: &'static str,
        value: Appearance,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let current = Theme::of(cx);
        let target = value.theme();
        let label = match value {
            Appearance::Dark => "Dark",
            Appearance::Light => "Light",
        };
        let ring = if selected {
            current.accent
        } else {
            current.border
        };

        div()
            .id(id)
            .flex()
            .flex_col()
            .items_center()
            .gap(theme::space_2())
            .cursor_pointer()
            .on_click(cx.listener(move |this, _event: &ClickEvent, window, cx| {
                theme::set_appearance(cx, value);
                this.persist(json!({ "themeMode": ThemeMode::from(value) }));
                // Mica/Acrylic map to a different backdrop variant per appearance, so re-apply.
                let treatment = theme::current_treatment();
                if treatment != SurfaceTreatment::Opaque {
                    let _ = glass::apply(window, value, treatment);
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(96.0))
                    .h(px(64.0))
                    .rounded(theme::radius_md())
                    .overflow_hidden()
                    .border_2()
                    .border_color(ring)
                    .bg(target.window_bg)
                    .child(
                        div()
                            .w_full()
                            .h(px(10.0))
                            .flex_shrink_0()
                            .bg(target.sidebar_bg)
                            .border_b_1()
                            .border_color(target.border),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h(px(0.0))
                            .child(div().w(px(18.0)).h_full().bg(target.sidebar_bg))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .gap(px(4.0))
                                    .p(px(7.0))
                                    .child(
                                        div()
                                            .w_full()
                                            .h(px(7.0))
                                            .rounded(px(2.0))
                                            .bg(target.surface_hover),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .h(px(7.0))
                                            .rounded(px(2.0))
                                            .bg(target.surface_hover),
                                    )
                                    .child(
                                        div()
                                            .w(px(28.0))
                                            .h(px(7.0))
                                            .rounded(px(2.0))
                                            .bg(target.surface_hover),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(theme::text_small())
                    .font_weight(if selected {
                        theme::weight_medium()
                    } else {
                        theme::weight_regular()
                    })
                    .text_color(if selected {
                        current.text
                    } else {
                        current.text_muted
                    })
                    .child(label),
            )
            .into_any_element()
    }
}
