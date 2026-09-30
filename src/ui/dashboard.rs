//! Main window — Phase 1 spike dashboard.
//!
//! Demonstrates the GPUI <-> state <-> backend bridge: the view renders `AppState`, drains
//! events already applied by the bridge, and emits user intents by mutating state directly
//! (the real app will route intents through an actions layer in Phase 4).

use gpui::{div, prelude::*, rgb, ClickEvent, Context, FocusHandle, IntoElement, Render, Window};

use crate::app::events::{AppEvent, EventOutcome};
use crate::app::state::{AppState, ConnectionStatus};
use crate::config::settings::AppSettings;
use crate::platform::tray::TrayCommand;
use crate::ui::theme;

pub struct Dashboard {
    pub state: AppState,
    focus_handle: FocusHandle,
}

impl Dashboard {
    pub fn new(settings: AppSettings, cx: &mut Context<Self>) -> Self {
        Self {
            state: AppState::new(settings),
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Applies a backend/tray event. Pure state transition; returns what the shell should do.
    pub fn apply_event(&mut self, event: AppEvent) -> EventOutcome {
        match event {
            AppEvent::Tray(TrayCommand::ShowWindow) => return EventOutcome::ShowWindow,
            AppEvent::Tray(TrayCommand::Quit) => return EventOutcome::Quit,
            AppEvent::Tray(TrayCommand::ToggleAutoAccept) => {
                self.state.settings.auto_accept_enabled = !self.state.settings.auto_accept_enabled;
                log::info!(
                    "auto accept toggled from tray -> {}",
                    self.state.settings.auto_accept_enabled
                );
            }
            AppEvent::BackendTick { uptime_secs } => {
                self.state.backend_uptime_secs = uptime_secs;
            }
            AppEvent::Connection { connected, message } => {
                self.state.connection = if connected {
                    ConnectionStatus::Connected
                } else {
                    ConnectionStatus::Disconnected
                };
                self.state.connection_message = message;
            }
        }
        EventOutcome::Continue
    }
}

fn status_color(summary: &str) -> gpui::Rgba {
    match summary {
        "Active" => theme::success(),
        "Connecting" => theme::warning(),
        "League offline" => theme::danger(),
        _ => theme::text_secondary(),
    }
}

impl Dashboard {
    fn toggle_card(
        id: &'static str,
        title: &'static str,
        subtitle: &'static str,
        checked: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> impl IntoElement {
        let (track, knob) = if checked {
            (theme::accent(), theme::text_primary())
        } else {
            (theme::surface_raised(), theme::text_secondary())
        };
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .px_4()
            .py_3()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::border())
            .rounded_lg()
            .cursor_pointer()
            .hover(|style| style.bg(theme::surface_raised()))
            .on_click(on_click)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_color(theme::text_primary()).child(title))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text_secondary())
                            .child(subtitle),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .w(gpui::px(38.0))
                    .h(gpui::px(21.0))
                    .px(gpui::px(3.0))
                    .bg(track)
                    .border_1()
                    .border_color(theme::border())
                    .rounded_full()
                    .child(div().size(gpui::px(13.0)).bg(knob).rounded_full()),
            )
    }
}

impl Render for Dashboard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.state.summary();
        let pill_color = status_color(summary);
        let auto_accept = self.state.settings.auto_accept_enabled;
        let minimize = self.state.settings.minimize_to_tray;
        let connection_message = format!("{} - {}", self.state.connection_message, summary);
        let backend_line = format!(
            "Backend service heartbeat: {}s (background async service -> channel -> GPUI)",
            self.state.backend_uptime_secs
        );

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::background())
            .text_color(theme::text_primary())
            .p_6()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_xl().child("League Auto Accept"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::text_secondary())
                                    .child("Rust + GPUI architecture spike"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_1()
                            .border_1()
                            .border_color(theme::border())
                            .rounded_full()
                            .child(div().size(gpui::px(7.0)).bg(pill_color).rounded_full())
                            .child(div().text_sm().child(summary.to_string())),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(connection_message),
            )
            .child(Self::toggle_card(
                "toggle-auto-accept",
                "Auto Accept",
                "Tray can toggle this; window reflects it immediately.",
                auto_accept,
                cx.listener(|this, _event: &ClickEvent, _window, cx| {
                    this.state.settings.auto_accept_enabled =
                        !this.state.settings.auto_accept_enabled;
                    cx.notify();
                }),
            ))
            .child(Self::toggle_card(
                "toggle-minimize",
                "Minimize to tray",
                "On: closing hides to tray. Off: closing exits (tray stays present).",
                minimize,
                cx.listener(|this, _event: &ClickEvent, _window, cx| {
                    this.state.settings.minimize_to_tray = !this.state.settings.minimize_to_tray;
                    cx.notify();
                }),
            ))
            .child(
                div()
                    .px_4()
                    .py_3()
                    .bg(theme::surface_raised())
                    .border_1()
                    .border_color(theme::border())
                    .rounded_lg()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(backend_line),
            )
            .child(
                div().flex().justify_end().child(
                    div()
                        .id("quit")
                        .px_4()
                        .py_2()
                        .bg(theme::accent())
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|style| style.opacity(0.9))
                        .on_click(cx.listener(|_this, _event: &ClickEvent, _window, cx| {
                            cx.quit();
                        }))
                        .child(div().text_color(rgb(0xffffff)).child("Quit")),
                ),
            )
            .track_focus(&self.focus_handle)
    }
}
