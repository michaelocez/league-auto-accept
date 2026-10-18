//! The Auto Accept screen: the status hero and the recent-activity timeline.

use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, Rgba};

use crate::app::state::ActivityKind;
use crate::ui::components::{Divider, Elevation, Surface, Toggle};
use crate::ui::theme::{self, Theme};

use super::Dashboard;

impl Dashboard {
    pub(super) fn render_dashboard(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::of(cx);
        let auto_accept = self.state.settings.auto_accept_enabled;
        let ready_message = self.state.ready_check_message.clone();
        let summary = self.state.summary();

        let state_color = match summary {
            "Active" => t.success,
            "Connecting" => t.warning,
            "League offline" => t.danger,
            _ => t.text_faint,
        };
        let hero_detail = if auto_accept {
            ready_message
        } else {
            "Off — turn on Auto Accept to monitor ready checks.".to_string()
        };

        let hero = Surface::new().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(theme::space_6())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::space_2())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme::space_3())
                                .child(div().size(px(9.0)).rounded_full().bg(state_color))
                                .child(
                                    div()
                                        .text_size(theme::text_title())
                                        .font_weight(theme::weight_semibold())
                                        .text_color(t.text)
                                        .child(summary),
                                ),
                        )
                        .child(
                            div()
                                .text_size(theme::text_small())
                                .text_color(t.text_muted)
                                .child(hero_detail),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme::space_3())
                        .child(
                            div()
                                .text_size(theme::text_small())
                                .text_color(t.text_muted)
                                .child("Auto Accept"),
                        )
                        .child(
                            Toggle::new("toggle-auto-accept")
                                .checked(auto_accept)
                                .on_change(cx.listener(
                                    move |this, _event: &ClickEvent, _window, cx| {
                                        this.toggle_auto_accept();
                                        cx.notify();
                                    },
                                )),
                        ),
                ),
        );

        let mut body = div().flex().flex_col().gap(theme::space_5()).child(hero);
        // Only show the timeline once there is something real to show — an empty placeholder adds
        // noise without information.
        if !self.state.activity.is_empty() {
            body = body.child(self.render_activity_timeline(&t));
        }
        body.into_any_element()
    }

    fn render_activity_timeline(&self, t: &Theme) -> gpui::AnyElement {
        let clear = Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        };
        // `last` only tags the final row, so an empty list must render nothing rather than underflow.
        let last = self.state.activity.len().saturating_sub(1);
        let items: Vec<gpui::AnyElement> = self
            .state
            .activity
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let top = if index == 0 { clear } else { t.border };
                let bottom = if index == last { clear } else { t.border };
                div()
                    .flex()
                    .gap(theme::space_3())
                    .min_h(px(32.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .w(px(10.0))
                            .flex_shrink_0()
                            .child(div().w(px(1.0)).h(px(8.0)).bg(top))
                            .child(
                                div()
                                    .size(px(8.0))
                                    .rounded_full()
                                    .bg(activity_color(item.kind, t)),
                            )
                            .child(div().w(px(1.0)).flex_1().bg(bottom)),
                    )
                    .child(
                        div()
                            .pb(theme::space_3())
                            .text_size(theme::text_body())
                            .text_color(t.text)
                            .child(item.label.clone()),
                    )
                    .into_any_element()
            })
            .collect();

        Surface::new()
            .elevation(Elevation::Base)
            .padded(false)
            .child(
                div()
                    .px(theme::space_4())
                    .py(theme::space_3())
                    .text_size(theme::text_small())
                    .font_weight(theme::weight_medium())
                    .text_color(t.text_muted)
                    .child("Recent activity"),
            )
            .child(Divider::horizontal())
            .child(
                div()
                    .px(theme::space_4())
                    .pt(theme::space_3())
                    .children(items),
            )
            .into_any_element()
    }
}

fn activity_color(kind: ActivityKind, t: &Theme) -> Rgba {
    match kind {
        ActivityKind::Info => t.text_muted,
        ActivityKind::Accepted => t.success,
    }
}
