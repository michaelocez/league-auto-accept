//! League Auto Accept (Rust + GPUI) entry point.
//!
//! Wires the always-present tray, the single-instance guard, the headless League service and the
//! typed settings store to the GPUI window. The UI only observes application state and emits
//! intents; it owns no credentials or networking.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, RwLock};

use gpui::{prelude::*, px, size, App, Bounds, WindowBounds, WindowOptions};

use league_auto_accept::app::events::{AppEvent, EventOutcome};
use league_auto_accept::app::service::spawn_league_service;
use league_auto_accept::config::store::{default_settings_directory, SettingsStore};
use league_auto_accept::platform::{single_instance, tray, window as platform_window};
use league_auto_accept::ui::dashboard::Dashboard;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Single instance: a second launch signals the first and exits.
    let Some(_guard) = single_instance::acquire() else {
        single_instance::signal_primary();
        eprintln!("League Auto Accept is already running; asked the existing instance to show.");
        return;
    };

    // Typed settings service (own persistence format; encrypted webhook secret).
    let store = Rc::new(RefCell::new(SettingsStore::with_directory(
        &default_settings_directory(),
    )));
    let settings = store.borrow_mut().load().unwrap_or_else(|error| {
        log::error!("failed to load settings, using defaults: {error}");
        Default::default()
    });

    let (tx, rx) = async_channel::unbounded::<AppEvent>();

    let show_tx = tx.clone();
    single_instance::spawn_primary_watcher(move || {
        let _ = show_tx.try_send(AppEvent::Tray(tray::TrayCommand::ShowWindow));
    });

    // Shared settings snapshot: the UI updates it, the notification service reads it.
    let settings_shared = Arc::new(RwLock::new(settings.clone()));

    // Headless League service + notifications + tray on their own threads.
    let background = spawn_league_service(
        tx.clone(),
        settings_shared.clone(),
        settings.auto_accept_enabled,
    );
    let enabled_flag = background.enabled_flag.clone();
    let notifications = background.notifications.clone();
    tray::spawn(tx.clone());

    gpui::application().run(move |cx: &mut App| {
        gpui::init(cx);

        // Capture the native window handle once (pure Win32 hide/show; no re-entrant GPUI borrows).
        let hwnd_slot = Arc::new(AtomicIsize::new(0));
        let view = {
            let hwnd_slot = hwnd_slot.clone();
            let store = store.clone();
            let enabled_flag = enabled_flag.clone();
            let settings_shared = settings_shared.clone();
            let notifications = notifications.clone();
            let settings = settings.clone();
            let bounds = Bounds::centered(None, size(px(620.0), px(560.0)), cx);
            let (_handle, view) = gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                move |window, cx| {
                    if let Some(hwnd) = platform_window::raw_hwnd(window) {
                        hwnd_slot.store(hwnd as isize, Ordering::SeqCst);
                    }
                    let webhook_value = settings.discord_webhook_url.clone();
                    let webhook_input = cx.new(|cx| {
                        let mut state = gpui::base::input::InputState::new(window, cx)
                            .placeholder("https://discord.com/api/webhooks/…")
                            .masked(true);
                        if !webhook_value.is_empty() {
                            state.set_value(webhook_value.clone(), window, cx);
                        }
                        state
                    });
                    let view = cx.new(|cx| {
                        Dashboard::new(
                            settings,
                            store,
                            enabled_flag,
                            settings_shared,
                            notifications,
                            webhook_input,
                            window,
                            cx,
                        )
                    });
                    let view_for_close = view.clone();
                    // Hide (don't close) when minimize-to-tray is on; the window — and all state —
                    // is preserved and restored in place from the tray.
                    window.on_window_should_close(cx, move |window, cx| {
                        let minimize_to_tray =
                            view_for_close.read(cx).state().settings.minimize_to_tray;
                        if minimize_to_tray {
                            log::info!("close intercepted: hiding to tray");
                            platform_window::hide(window);
                            false
                        } else {
                            log::info!("close allowed: exiting application");
                            true
                        }
                    });
                    view
                },
            )
            .expect("failed to open window");
            view
        };

        cx.activate(true);

        // Bridge: drain backend/tray events on the GPUI foreground executor and update state.
        let hwnd_slot = hwnd_slot.clone();
        cx.spawn(async move |cx| {
            while let Ok(event) = rx.recv().await {
                let outcome = cx.update(|app| {
                    view.update(app, |dashboard, cx| {
                        let outcome = dashboard.apply_event(event);
                        cx.notify();
                        outcome
                    })
                });
                match outcome {
                    EventOutcome::ShowWindow => {
                        let raw = hwnd_slot.load(Ordering::SeqCst);
                        if raw != 0 {
                            platform_window::show_hwnd(raw);
                        }
                    }
                    EventOutcome::Quit => {
                        cx.update(|app| app.quit());
                        break;
                    }
                    EventOutcome::Continue => {}
                }
            }
        })
        .detach();
    });
}
