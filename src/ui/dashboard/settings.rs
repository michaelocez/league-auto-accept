//! The Settings screen: window behaviour, appearance, the backdrop treatment, and — only once
//! something has actually failed — the diagnostics that explain it.

use gpui::prelude::*;
use gpui::{div, px, ClickEvent, ClipboardItem, Context};
use serde_json::json;

use crate::app::{diagnostics, update};
use crate::config::settings::ThemeMode;
use crate::platform::glass;
use crate::ui::components::{Button, ButtonVariant, Row, SettingsGroup};
use crate::ui::decisions::{
    diagnostics_preview, diagnostics_summary, diagnostics_visible, ToggleSetting,
};
use crate::ui::theme::{self, Appearance, SurfaceTreatment, Theme};

use super::Dashboard;

/// The mini window preview shown beside each theme choice.
const PREVIEW_WIDTH: f32 = 96.0;
const PREVIEW_HEIGHT: f32 = 64.0;

/// Where "Open an issue" goes. A fixed address in this repository, and never anything a response
/// could name — the update check's guard exists for the link it receives from the network, and this
/// one never leaves this file.
const ISSUE_PAGE: &str = "https://github.com/michaelocez/league-auto-accept/issues/new";

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
            .gap(theme::space_half())
            .p(theme::space_half())
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

        let mut content = div()
            .flex()
            .flex_col()
            .gap(theme::space_6())
            .child(window)
            .child(appearance_group);

        // Nothing to show means no section at all: a Diagnostics heading that is always there would
        // be decoration, and would teach the user to ignore the one case where it matters.
        if let Some(section) = self.diagnostics_group(cx) {
            content = content.child(section);
        }

        content.child(self.version_line(cx)).into_any_element()
    }

    /// The Diagnostics section: what failed this session, and the two ways to report it.
    ///
    /// Everything shown is already in memory — the app writes no log — so the section says what it
    /// knows rather than sending the user to a file. The report is the whole of the relevant record
    /// tail, redacted before it was ever retained, and the issue page is this project's fixed one.
    fn diagnostics_group(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let t = Theme::of(cx);
        let errors = diagnostics::error_count();
        let banner = self.state.settings_error.clone();
        if !diagnostics_visible(errors, banner.is_some()) {
            return None;
        }

        // The newest error is the one worth reading first; the banner is the fallback for a failure
        // that was reported before the buffer saw it.
        let preview = diagnostics::latest_error()
            .or(banner)
            .map(|text| diagnostics_preview(&text))
            .unwrap_or_default();

        let actions = div()
            .flex()
            .items_center()
            .gap(theme::space_3())
            .child(
                Button::new("copy-diagnostics", "Copy diagnostics")
                    .variant(ButtonVariant::Secondary)
                    .on_click(cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        // GPUI's own clipboard: no extra dependency, and the same mechanism the
                        // text fields already paste through.
                        cx.write_to_clipboard(ClipboardItem::new_string(diagnostics::report()));
                        this.diagnostics_copied = true;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("open-issue", "Open an issue")
                    .variant(ButtonVariant::Ghost)
                    .on_click(|_event: &ClickEvent, _window, app| app.open_url(ISSUE_PAGE)),
            );

        Some(
            SettingsGroup::new()
                .title("Diagnostics")
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::space_3())
                        .px(theme::space_4())
                        .py(theme::space_3())
                        .child(
                            div()
                                .text_size(theme::text_small())
                                .text_color(t.text)
                                .child(diagnostics_summary(errors)),
                        )
                        .child(
                            div()
                                .text_size(theme::text_caption())
                                .text_color(t.text_muted)
                                .child(preview),
                        )
                        .child(actions)
                        .when(self.diagnostics_copied, |this| {
                            this.child(
                                div()
                                    .text_size(theme::text_caption())
                                    .text_color(t.success)
                                    .child("Copied to the clipboard."),
                            )
                        }),
                )
                .into_any_element(),
        )
    }

    /// The version footer: the number this build actually shipped as, and a way to reach the
    /// releases page. Deliberately the quietest text on the screen — it is reference information,
    /// not a setting.
    ///
    /// The line is only extended once the session's single release check has found a newer release.
    /// A check still in flight, a check that found nothing newer, and a check that failed are all
    /// the bare version, so no failure is ever shown here and no "up to date" claim is made.
    ///
    /// The link is the project's fixed releases page unless there is a newer release to point at, in
    /// which case it is that release's own page — already put through the guard in `update`, so it
    /// cannot be a URL that arrived over the network unchecked.
    fn version_line(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::of(cx);
        let (label, color, url) = match self.state.newer_release.as_ref() {
            Some(notice) => (
                format!(
                    "League Auto Accept {} · version {} is available",
                    update::VERSION,
                    notice.version
                ),
                t.text_muted,
                notice.url.clone(),
            ),
            None => (
                format!("League Auto Accept {}", update::VERSION),
                t.text_faint,
                update::RELEASES_PAGE.to_string(),
            ),
        };

        div()
            .id("version-line")
            .pt(theme::space_2())
            .text_size(theme::text_caption())
            .text_color(color)
            .cursor_pointer()
            .hover(move |style| style.text_color(t.text))
            .on_click(move |_event: &ClickEvent, _window, app| app.open_url(&url))
            .child(label)
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
        // The preview follows the backdrop the user is on, so switching Opaque → Mica → Acrylic shows
        // the translucency in the swatches themselves, not only in the window behind them.
        let target = value.theme_with(theme::current_treatment());
        let label = match value {
            Appearance::Dark => "Dark",
            Appearance::Light => "Light",
        };
        let ring = if selected {
            current.accent
        } else {
            // An unselected swatch still needs an edge in light mode, where `border` nearly vanishes.
            current.border_strong
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
                    .w(px(PREVIEW_WIDTH))
                    .h(px(PREVIEW_HEIGHT))
                    .rounded(theme::radius_md())
                    .overflow_hidden()
                    .border_2()
                    .border_color(ring)
                    .bg(target.bg)
                    // The rail runs the full height, as it does in the window, and the content uses the
                    // strongest surface tones available: in light mode the normal surface fills sit
                    // within a few percent of the page, which turned the swatch into one white box.
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(theme::space_half())
                            .w(px(20.0))
                            .h_full()
                            .flex_shrink_0()
                            .pt(px(6.0))
                            .bg(target.surface_active)
                            .child(div().size(px(4.0)).rounded_full().bg(target.text_faint))
                            .child(div().size(px(4.0)).rounded_full().bg(target.border_strong))
                            .child(div().size(px(4.0)).rounded_full().bg(target.border_strong)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .gap(theme::space_1())
                            .p(px(7.0))
                            .child(
                                div()
                                    .w_full()
                                    .h(px(7.0))
                                    .rounded(theme::radius_xs())
                                    .bg(target.surface_active),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .h(px(7.0))
                                    .rounded(theme::radius_xs())
                                    .bg(target.border_strong),
                            )
                            .child(
                                div()
                                    .w(px(28.0))
                                    .h(px(7.0))
                                    .rounded(theme::radius_xs())
                                    .bg(target.border_strong),
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
