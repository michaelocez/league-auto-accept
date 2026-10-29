//! Custom tray popup: a small GPUI surface shown on tray left-click.
//!
//! It is a second window whose root is this view. It holds no state of its own — it reads the
//! authoritative `AppState` from the main [`Dashboard`] entity and sends intents through the same
//! `AppEvent` channel as the tray, so the main window and the popup can never disagree. The native
//! tray menu remains the right-click fallback; the tray backend stays state/command-only.

use std::time::Instant;

use async_channel::Sender;
use gpui::prelude::*;
use gpui::{
    div, px, ClickEvent, Context, Entity, FocusHandle, IntoElement, KeyDownEvent, Render,
    Subscription, Window, WindowHandle, WindowId,
};

use crate::app::events::AppEvent;
use crate::platform::tray::TrayCommand;
use crate::ui::components::{Button, ButtonVariant, Divider, Toggle};
use crate::ui::dashboard::{status_color, Dashboard};
use crate::ui::decisions::{tray_popup_action, TrayPopupAction, TRAY_POPUP_REOPEN_GUARD};
use crate::ui::shell::app_mark;
use crate::ui::theme::{self, Theme};

/// The close button's hit target.
const CLOSE_SIZE: f32 = 24.0;

/// Owns the tray popup's window for the shell: what is up, when it last hid, and what a tray click
/// does to it.
///
/// The popup dismisses itself the moment it loses OS focus, so the shell cannot learn it is gone
/// from the handle alone — and the click that dismisses it *is* the click whose intent arrives next.
/// Recording when the popup disappeared is what lets [`Self::toggle`] tell "the click being handled
/// just closed this" from "this closed a while ago, so this click opens it again", through the pure
/// decision in [`crate::ui::decisions::tray_popup_action`]. This is the only place that holds the
/// window, so the popup's own render code stays unaware of it.
pub struct TrayPopupToggle {
    /// The popup while it is up, and the id the shell matches its close event against. The id is
    /// kept beside the handle because a closed handle is no longer readable, and the close event
    /// arrives after the window is already gone.
    popup: Option<(WindowHandle<TrayPopup>, WindowId)>,
    /// How the shell closes a window. Injected rather than called here so this type holds no GPUI
    /// borrow of its own: the caller passes the `App` it already has, and the controller stays a
    /// plain record of what is up.
    close: fn(&mut gpui::App, WindowHandle<TrayPopup>),
    /// When the popup last hid, whatever hid it.
    hidden_at: Option<Instant>,
}

impl TrayPopupToggle {
    pub fn new(close: fn(&mut gpui::App, WindowHandle<TrayPopup>)) -> Self {
        Self {
            popup: None,
            close,
            hidden_at: None,
        }
    }

    /// The id of the popup window while it is up; the shell matches its close event against this.
    pub fn popup_id(&self) -> Option<WindowId> {
        self.popup.as_ref().map(|(_, id)| *id)
    }

    /// Records that the popup hid, and gives up the handle: after this it is not up and not ours.
    ///
    /// Every hide ends here — the popup's own dismissal, and the close this controller performs —
    /// so a later click is always measured from the real hide and never from a stale handle.
    pub fn record_hidden(&mut self, at: Instant) {
        self.popup = None;
        self.hidden_at = Some(at);
    }

    /// Applies one tray activation: opens the popup, or closes an open one and leaves it closed.
    ///
    /// `open` is the caller's "show the popup" action, and is called only when the decision is to
    /// show, so this type never needs to know how a popup is built or where it is anchored.
    ///
    /// The already-hidden case is stamped as well: that click is the one that just dismissed the
    /// popup through its own deactivation, and stamping the moment keeps this click's dismissal
    /// attributable to it — so the next click opens, instead of closing what is already gone.
    pub fn toggle(
        &mut self,
        app: &mut gpui::App,
        now: Instant,
        open: impl FnOnce(&mut gpui::App) -> Option<WindowHandle<TrayPopup>>,
    ) {
        let is_open = self.popup.is_some();
        match tray_popup_action(is_open, self.hidden_at, now, TRAY_POPUP_REOPEN_GUARD) {
            TrayPopupAction::KeepHidden => {
                if let Some((handle, _)) = self.popup.take() {
                    (self.close)(app, handle);
                }
                self.hidden_at = Some(now);
            }
            TrayPopupAction::Show => {
                self.popup = open(app).map(|handle| {
                    let id = handle.window_id();
                    (handle, id)
                });
                // A popup that could not be opened leaves no hide stamp behind, so the next click
                // is a fresh attempt rather than one the guard swallows.
                if self.popup.is_some() {
                    self.hidden_at = None;
                }
            }
        }
    }
}

pub struct TrayPopup {
    dashboard: Entity<Dashboard>,
    tx: Sender<AppEvent>,
    focus_handle: FocusHandle,
    /// GPUI subscriptions are RAII — dropping one unsubscribes it — so the vec is held for the
    /// popup's lifetime to keep the state observation and the dismiss-on-blur active.
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
        let discord_enabled = dashboard.state.settings.discord_notifications_enabled;
        let connection = dashboard.state.connection.label();
        let status = status_color(summary, &t);

        let toggle_tx = self.tx.clone();
        let discord_tx = self.tx.clone();
        let open_tx = self.tx.clone();
        let quit_tx = self.tx.clone();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(t.surface)
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
                            .size(px(CLOSE_SIZE))
                            .rounded(theme::radius_sm())
                            .cursor_pointer()
                            .text_size(theme::text_caption())
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
                            .child(div().size(theme::size_dot()).rounded_full().bg(status))
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
                    )
                    .child(Divider::horizontal())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(theme::text_body())
                                    .child("Discord notifications"),
                            )
                            .child(
                                Toggle::new("popup-discord")
                                    .checked(discord_enabled)
                                    .on_change(move |_event: &ClickEvent, _window, _app| {
                                        let _ = discord_tx.try_send(AppEvent::Tray(
                                            TrayCommand::ToggleDiscordNotifications,
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
