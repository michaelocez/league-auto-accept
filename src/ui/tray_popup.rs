//! Custom tray popup: a small GPUI surface shown on tray left-click.
//!
//! It is a second window whose root is this view. It holds no state of its own — it reads the
//! authoritative `AppState` from the main [`Dashboard`] entity and sends intents through the same
//! `AppEvent` channel as the tray, so the main window and the popup can never disagree. The native
//! tray menu remains the right-click fallback; the tray backend stays state/command-only.

use async_channel::Sender;
use gpui::prelude::*;
use gpui::{
    div, px, ClickEvent, Context, Entity, FocusHandle, IntoElement, KeyDownEvent, Render,
    Subscription, Window,
};

use crate::app::events::AppEvent;
use crate::platform::tray::TrayCommand;
use crate::ui::components::{Button, ButtonVariant, Divider, Toggle};
use crate::ui::dashboard::{status_color, Dashboard};
use crate::ui::shell::app_mark;
use crate::ui::theme::{self, Theme};

pub struct TrayPopup {
    dashboard: Entity<Dashboard>,
    tx: Sender<AppEvent>,
    #[allow(dead_code)]
    focus_handle: FocusHandle,
    #[allow(dead_code)]
    subscriptions: Vec<Subscription>,
}

impl TrayPopup {
    pub fn new(
        dashboard: Entity<Dashboard>,
        tx: Sender<AppEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        // Re-render whenever the authoritative app state changes.
        let observe = cx.observe(&dashboard, |_this, _dashboard, cx| cx.notify());
        // Dismiss the popup when it loses OS focus (click elsewhere), like a native flyout.
        let activation = cx.observe_window_activation(window, |_this, window, _cx| {
            if !window.is_window_active() {
                window.remove_window();
            }
        });
        window.focus(&focus_handle, cx);
        // Force OS activation so a later click elsewhere reliably deactivates (and dismisses) us.
        window.activate_window();

        Self {
            dashboard,
            tx,
            focus_handle,
            subscriptions: vec![observe, activation],
        }
    }
}

impl Render for TrayPopup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let dashboard = self.dashboard.read(cx);
        let summary = dashboard.state.summary();
        let auto_accept = dashboard.state.settings.auto_accept_enabled;
        let connection = dashboard.state.connection.label();
        let status = status_color(summary);

        let toggle_tx = self.tx.clone();
        let open_tx = self.tx.clone();
        let quit_tx = self.tx.clone();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(t.window_bg)
            .border_1()
            .border_color(t.border_strong)
            .text_color(t.text)
            .track_focus(&self.focus_handle)
            .on_key_down(|event: &KeyDownEvent, window, _cx| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            })
            // Header
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(theme::space_4())
                    .py(theme::space_3())
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::space_2())
                            .child(app_mark(&t))
                            .child(
                                div()
                                    .text_size(theme::text_body())
                                    .font_weight(theme::weight_semibold())
                                    .child("League Auto Accept"),
                            ),
                    )
                    .child(
                        div()
                            .id("popup-close")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(24.0))
                            .rounded(theme::radius_sm())
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(t.text_muted)
                            .hover(|style| style.bg(t.surface_hover).text_color(t.text))
                            .on_click(|_event: &ClickEvent, window, _app| {
                                window.remove_window();
                            })
                            .child("\u{2715}"), // ✕
                    ),
            )
            // Status + primary control
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::space_3())
                    .p(theme::space_4())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::space_2())
                            .child(div().size(px(8.0)).rounded_full().bg(status))
                            .child(
                                div()
                                    .text_size(theme::text_heading())
                                    .font_weight(theme::weight_medium())
                                    .child(summary),
                            ),
                    )
                    .child(
                        div()
                            .text_size(theme::text_small())
                            .text_color(t.text_muted)
                            .child(connection),
                    )
                    .child(Divider::horizontal())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_size(theme::text_body()).child("Auto Accept"))
                            .child(
                                Toggle::new("popup-auto-accept")
                                    .checked(auto_accept)
                                    .on_change(move |_event: &ClickEvent, _window, _app| {
                                        let _ = toggle_tx.try_send(AppEvent::Tray(
                                            TrayCommand::ToggleAutoAccept,
                                        ));
                                    }),
                            ),
                    ),
            )
            // Footer actions
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme::space_2())
                    .px(theme::space_4())
                    .py(theme::space_3())
                    .border_t_1()
                    .border_color(t.border)
                    .child(
                        Button::new("popup-open", "Open")
                            .variant(ButtonVariant::Secondary)
                            .on_click(move |_event: &ClickEvent, window, _app| {
                                let _ = open_tx.try_send(AppEvent::Tray(TrayCommand::ShowWindow));
                                window.remove_window();
                            }),
                    )
                    .child(
                        // Explicitly "Quit app" (not "close"): the ✕ / click-away close the popup.
                        Button::new("popup-quit", "Quit app")
                            .variant(ButtonVariant::Ghost)
                            .on_click(move |_event: &ClickEvent, _window, _app| {
                                let _ = quit_tx.try_send(AppEvent::Tray(TrayCommand::Quit));
                            }),
                    ),
            )
    }
}
