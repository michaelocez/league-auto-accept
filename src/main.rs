//! League Auto Accept (Rust + GPUI) — Phase 1 architecture spike entry point.
//!
//! Proves: GPUI window, always-present tray, minimize-to-tray, single-instance, and the
//! background-async-service <-> UI bridge. No League service, settings system or Discord yet.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;

use gpui::{prelude::*, px, size, App, Application, Bounds, WindowBounds, WindowOptions};

use league_auto_accept::app::events::{AppEvent, EventOutcome};
use league_auto_accept::config::settings::Settings;
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

    let (tx, rx) = async_channel::unbounded::<AppEvent>();

    let show_tx = tx.clone();
    single_instance::spawn_primary_watcher(move || {
        let _ = show_tx.try_send(AppEvent::Tray(tray::TrayCommand::ShowWindow));
    });

    // Spike C: background async service on its own thread + tokio runtime.
    league_auto_accept::app::service::spawn_stub_backend(tx.clone());
    // Spike B: always-present tray on its own thread + message loop.
    tray::spawn(tx.clone());

    let settings = Settings::default();

    Application::new().run(move |cx: &mut App| {
        let view = cx.new(|cx| Dashboard::new(settings, cx));

        let bounds = Bounds::centered(None, size(px(600.0), px(440.0)), cx);
        let window_handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                {
                    let view = view.clone();
                    move |window, cx| {
                        let view_for_close = view.clone();
                        // Hide (don't close) when minimize-to-tray is on; the window — and all
                        // state — is preserved and restored in place from the tray.
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
                    }
                },
            )
            .expect("failed to open window");

        // Capture the native window handle once. Showing/hiding is then pure Win32, avoiding
        // any re-entrant GPUI borrows from the async event loop. The window is only ever
        // hidden, so this handle stays valid for the process lifetime.
        let hwnd_slot = Arc::new(AtomicIsize::new(0));
        if window_handle
            .update(cx, |_view, window, _cx| {
                if let Some(hwnd) = platform_window::raw_hwnd(window) {
                    hwnd_slot.store(hwnd as isize, Ordering::SeqCst);
                }
            })
            .is_err()
        {
            log::warn!("failed to capture the native window handle");
        }

        cx.activate(true);

        // Bridge: drain backend/tray events on the GPUI foreground executor and update state.
        let view = view.clone();
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
                    Ok(EventOutcome::ShowWindow) => {
                        let raw = hwnd_slot.load(Ordering::SeqCst);
                        if raw != 0 {
                            platform_window::show_hwnd(raw);
                        }
                    }
                    Ok(EventOutcome::Quit) | Err(_) => {
                        let _ = cx.update(|app| app.quit());
                        break;
                    }
                    Ok(EventOutcome::Continue) => {}
                }
            }
        })
        .detach();
    });
}
