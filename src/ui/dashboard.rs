//! Main window: a compact status + control surface for Auto Accept.
//!
//! The view renders `AppState` and emits user intents (toggles) that are applied to the settings
//! store and the shared service enable flag. It contains no networking, credentials or filesystem
//! logic beyond the typed settings service (AGENTS.md sec.13).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gpui::{div, prelude::*, rgb, ClickEvent, Context, FocusHandle, IntoElement, Render, Window};
use serde_json::json;

use crate::app::events::{AppEvent, EventOutcome};
use crate::app::state::{ActivityKind, AppState};
use crate::config::settings::AppSettings;
use crate::config::store::SettingsStore;
use crate::platform::tray::TrayCommand;
use crate::ui::theme;

pub struct Dashboard {
    pub state: AppState,
    store: Rc<RefCell<SettingsStore>>,
    enabled_flag: Arc<AtomicBool>,
    focus_handle: FocusHandle,
}

impl Dashboard {
    pub fn new(
        settings: AppSettings,
        store: Rc<RefCell<SettingsStore>>,
        enabled_flag: Arc<AtomicBool>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            state: AppState::new(settings),
            store,
            enabled_flag,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Persists a settings patch through the typed store and mirrors the result into state.
    fn persist(&mut self, patch: serde_json::Value) {
        match self.store.borrow_mut().update(&patch) {
            Ok(updated) => self.state.settings = updated,
            Err(error) => log::error!("failed to persist settings: {error}"),
        }
    }

    pub fn apply_event(&mut self, event: AppEvent) -> EventOutcome {
        match event {
            AppEvent::Tray(TrayCommand::ShowWindow) => return EventOutcome::ShowWindow,
            AppEvent::Tray(TrayCommand::Quit) => return EventOutcome::Quit,
            AppEvent::Tray(TrayCommand::ToggleAutoAccept) => {
                let next = !self.state.settings.auto_accept_enabled;
                self.set_auto_accept(next);
            }
            AppEvent::Service(service_event) => {
                self.state.apply_service_event(&service_event);
            }
        }
        EventOutcome::Continue
    }

    fn set_auto_accept(&mut self, enabled: bool) {
        self.state.settings.auto_accept_enabled = enabled;
        self.enabled_flag.store(enabled, Ordering::Relaxed);
        self.persist(json!({ "autoAcceptEnabled": enabled }));
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

fn activity_color(kind: ActivityKind) -> gpui::Rgba {
    match kind {
        ActivityKind::Info => theme::text_secondary(),
        ActivityKind::Accepted => theme::success(),
        ActivityKind::Warning => theme::danger(),
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
        let connection_message = self.state.connection_message.clone();
        let ready_message = self.state.ready_check_message.clone();
        let monitoring = self.state.monitoring();

        let activity: Vec<gpui::AnyElement> = if self.state.activity.is_empty() {
            vec![div()
                .px_4()
                .py_3()
                .text_sm()
                .text_color(theme::text_secondary())
                .child("No activity yet.")
                .into_any_element()]
        } else {
            self.state
                .activity
                .iter()
                .map(|item| {
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_4()
                        .py_2()
                        .child(
                            div()
                                .size(gpui::px(6.0))
                                .rounded_full()
                                .bg(activity_color(item.kind)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme::text_primary())
                                .child(item.label.clone()),
                        )
                        .into_any_element()
                })
                .collect()
        };

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
                                    .child("Ready-check automation"),
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
                if monitoring {
                    "Monitoring ready checks."
                } else {
                    "Accepts one ready check per queue."
                },
                auto_accept,
                cx.listener(|this, _event: &ClickEvent, _window, cx| {
                    let next = !this.state.settings.auto_accept_enabled;
                    this.set_auto_accept(next);
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
                    .child(ready_message),
            )
            .child(Self::toggle_card(
                "toggle-minimize",
                "Minimize to tray",
                "On: closing hides to tray. Off: closing exits (tray stays present).",
                minimize,
                cx.listener(|this, _event: &ClickEvent, _window, cx| {
                    let next = !this.state.settings.minimize_to_tray;
                    this.state.settings.minimize_to_tray = next;
                    this.persist(json!({ "minimizeToTray": next }));
                    cx.notify();
                }),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(theme::border())
                    .rounded_lg()
                    .bg(theme::surface())
                    .child(
                        div()
                            .px_4()
                            .pt_3()
                            .pb_1()
                            .text_sm()
                            .text_color(theme::text_secondary())
                            .child("Recent activity"),
                    )
                    .children(activity),
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
