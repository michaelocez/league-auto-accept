//! League Auto Accept (Rust + GPUI) — Phase 1 architecture spike entry point.
//!
//! Proves: GPUI window, always-present tray, minimize-to-tray, single-instance, and the
//! background-async-service <-> UI bridge. No League service, settings system or Discord yet.

use gpui::{prelude::*, px, size, App, Application, Bounds, WindowBounds, WindowOptions};

use league_auto_accept::app::events::{AppEvent, EventOutcome};
use league_auto_accept::config::settings::Settings;
use league_auto_accept::platform::{single_instance, tray};
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
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            {
                let view = view.clone();
                move |window, cx| {
                    let view_for_close = view.clone();
                    window.on_window_should_close(cx, move |_window, cx| {
                        let minimize_to_tray =
                            view_for_close.read(cx).state().settings.minimize_to_tray;
                        if minimize_to_tray {
                            log::info!("close intercepted: hiding to tray");
                            cx.hide();
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

        cx.activate(true);

        // Bridge: drain backend/tray events on the GPUI foreground executor and update state.
        let view = view.clone();
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
                        let _ = cx.update(|app| app.activate(true));
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
