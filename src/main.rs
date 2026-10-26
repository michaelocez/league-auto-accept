// Release builds are GUI apps: no console window opens alongside the application.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! League Auto Accept (Rust + GPUI) entry point.
//!
//! Wires the always-present tray, the single-instance guard, the headless League service and the
//! typed settings store to the GPUI window. The UI only observes application state and emits
//! intents; it owns no credentials or networking.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, RwLock};

use gpui::{
    point, prelude::*, px, size, App, Bounds, TitlebarOptions, WindowBackgroundAppearance,
    WindowBounds, WindowHandle, WindowKind, WindowOptions,
};

use league_auto_accept::app::events::{AppEvent, EventOutcome};
use league_auto_accept::app::service::spawn_league_service;
use league_auto_accept::config::store::{
    default_settings_directory, load_error_message, SettingsStore,
};
use league_auto_accept::notifications::discord::WebhookSecret;
use league_auto_accept::platform::tray::TrayCommand;
use league_auto_accept::platform::{single_instance, tray, window as platform_window};
use league_auto_accept::ui::dashboard::Dashboard;
use league_auto_accept::ui::theme;
use league_auto_accept::ui::tray_popup::TrayPopup;

fn main() {
    // The application's single logger: the same `RUST_LOG`-or-`info` filter as before, over a
    // bounded in-memory buffer, so a failure in the field can be read in Settings rather than
    // vanishing into a stderr a GUI-subsystem release build does not have. Still no log file.
    league_auto_accept::app::diagnostics::install();

    // Single instance: a second launch signals the first and exits.
    let Some(_guard) = single_instance::acquire() else {
        single_instance::signal_primary();
        eprintln!("League Auto Accept is already running; asked the existing instance to show.");
        return;
    };

    // Typed settings service (own persistence format; encrypted webhook secret). The secret handle
    // is written only by the store and read only by the notification sender; application state and
    // the rendered UI carry just a non-secret configured/not-configured status.
    let webhook = Arc::new(RwLock::new(WebhookSecret::default()));
    let store = Rc::new(RefCell::new(SettingsStore::with_directory_and_webhook(
        &default_settings_directory(),
        webhook.clone(),
    )));
    // Say where settings live: a refused write is otherwise only reported as an I/O error.
    let settings_path = store.borrow().path().to_path_buf();
    log::info!("settings file: {}", settings_path.display());
    // A failed read is degraded, not fatal: the session runs on defaults, the store refuses every
    // later write so those defaults never replace the real file, and the banner explains it from the
    // first frame instead of only logging it.
    let (settings, settings_error) = match store.borrow_mut().load() {
        Ok(settings) => (settings, None),
        Err(error) => {
            log::error!("failed to load settings, using defaults and refusing to save: {error}");
            (
                Default::default(),
                Some(load_error_message(&settings_path, &error)),
            )
        }
    };

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
        webhook.clone(),
        settings.auto_accept_enabled,
    );
    let enabled_flag = background.enabled_flag.clone();
    let notifications = background.notifications.clone();
    let updates = background.update_check.clone();
    tray::spawn(tx.clone(), settings_shared.clone());

    // Closing the main window with minimize-to-tray off exits through this channel, so the pending
    // settings write is flushed by the view before the process goes away.
    platform_window::request_exit_through(tx.clone());

    // The tray popup sends its intents through the same channel as the tray.
    let ui_tx = tx.clone();

    gpui::application().run(move |cx: &mut App| {
        gpui::init(cx);

        // Establish the single source of truth for the active visual appearance before any view
        // renders. Both are persisted, so a restart keeps the user's chosen theme and backdrop.
        theme::set_appearance(cx, settings.theme_mode.into());
        theme::set_treatment(cx, settings.window_backdrop.into());

        // Capture the native window handle once (pure Win32 hide/show; no re-entrant GPUI borrows).
        let hwnd_slot = Arc::new(AtomicIsize::new(0));
        let view = {
            let hwnd_slot = hwnd_slot.clone();
            let store = store.clone();
            let enabled_flag = enabled_flag.clone();
            let settings_shared = settings_shared.clone();
            let notifications = notifications.clone();
            let updates = updates.clone();
            let settings = settings.clone();
            let minimize_to_tray = settings.minimize_to_tray;
            let bounds = Bounds::centered(None, size(px(880.0), px(600.0)), cx);
            let (handle, view) = gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(720.0), px(520.0))),
                    // Custom-drawn title bar: hide the OS chrome and draw our own in `shell.rs`.
                    titlebar: Some(TitlebarOptions {
                        title: Some("League Auto Accept".into()),
                        appears_transparent: true,
                        traffic_light_position: None,
                    }),
                    window_background: theme::window_background(
                        theme::current(),
                        theme::current_treatment(),
                    ),
                    ..Default::default()
                },
                cx,
                move |window, cx| {
                    if let Some(hwnd) = platform_window::raw_hwnd(window) {
                        // Release: the handle is the only thing being published to the drain task
                        // below, so this is the weakest ordering that still hands it over whole.
                        hwnd_slot.store(hwnd as isize, Ordering::Release);
                    }
                    // Starts empty: the saved webhook is never decrypted into the UI. The row shows
                    // only whether one is configured; typing here sets a new one.
                    let webhook_input = cx.new(|cx| {
                        gpui::base::input::InputState::new(window, cx)
                            .placeholder("https://discord.com/api/webhooks/…")
                            .masked(true)
                    });
                    cx.new(|cx| {
                        Dashboard::new(
                            settings,
                            settings_error,
                            store,
                            enabled_flag,
                            settings_shared,
                            notifications,
                            updates,
                            webhook_input,
                            window,
                            cx,
                        )
                    })
                },
            )
            .expect("failed to open window");
            // Round the window to match the native Windows 11 look; best-effort on older builds.
            let _ = handle.update(cx, |_view, window, _cx| {
                league_auto_accept::platform::popup::apply_rounded_corners(window);
                // Let closing hide to the tray while `minimizeToTray` is on; the UI keeps the flag
                // in step whenever that setting changes.
                platform_window::intercept_close(window, minimize_to_tray);
                // Re-apply a persisted Mica/Acrylic backdrop (the window was created with the
                // matching GPUI background, but the DWM backdrop type is set explicitly).
                let treatment = theme::current_treatment();
                if treatment != theme::SurfaceTreatment::Opaque {
                    let _ = league_auto_accept::platform::glass::apply(
                        window,
                        theme::current(),
                        treatment,
                    );
                }
            });
            view
        };

        cx.activate(true);

        // Bridge: drain backend/tray events on the GPUI foreground executor and update state.
        let hwnd_slot = hwnd_slot.clone();
        cx.spawn(async move |cx| {
            let mut popup: Option<WindowHandle<TrayPopup>> = None;
            loop {
                let Ok(event) = rx.recv().await else {
                    break;
                };
                // The tray popup is a separate window owned by the shell, not app state.
                if matches!(&event, AppEvent::Tray(TrayCommand::OpenPopup)) {
                    cx.update(|app| {
                        if let Some(handle) = popup {
                            let _ = handle.update(app, |_view, window, _cx| window.remove_window());
                        }
                        popup = open_tray_popup(app, view.clone(), ui_tx.clone());
                    });
                    continue;
                }
                let outcome = cx.update(|app| {
                    view.update(app, |dashboard, cx| {
                        let outcome = dashboard.apply_event(event, cx);
                        cx.notify();
                        outcome
                    })
                });
                match outcome {
                    EventOutcome::ShowWindow => {
                        // Acquire: pairs with the Release store above, so the handle is visible here
                        // even if the drain task is not the thread that created the window.
                        let raw = hwnd_slot.load(Ordering::Acquire);
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

/// Opens the custom tray popup anchored to the bottom-right of the primary display's work area
/// (just above the tray). Recreated per invocation; it dismisses itself on focus loss.
fn open_tray_popup(
    app: &mut App,
    dashboard: gpui::Entity<Dashboard>,
    tx: async_channel::Sender<AppEvent>,
) -> Option<WindowHandle<TrayPopup>> {
    let area = app
        .primary_display()
        .map(|display| display.visible_bounds())
        .unwrap_or_else(|| Bounds::new(point(px(0.0), px(0.0)), size(px(1920.0), px(1080.0))));
    let width = px(300.0);
    let height = px(300.0);
    let margin = px(8.0);
    let origin = point(
        area.origin.x + area.size.width - width - margin,
        area.origin.y + area.size.height - height - margin,
    );
    let bounds = Bounds::new(origin, size(width, height));

    match app.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::PopUp,
            focus: true,
            show: true,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            // Created opaque; `glass::apply` enables the backdrop (or falls back) when one is active.
            window_background: WindowBackgroundAppearance::Opaque,
            ..Default::default()
        },
        move |window, cx| cx.new(|cx| TrayPopup::new(dashboard, tx, window, cx)),
    ) {
        Ok(handle) => {
            // Borderless popups are square with no shadow by default; ask DWM to round + shadow it.
            let _ = handle.update(app, |_view, window, _cx| {
                league_auto_accept::platform::popup::apply_rounded_shadow(window);
                if theme::current_treatment() != theme::SurfaceTreatment::Opaque {
                    let _ = league_auto_accept::platform::glass::apply(
                        window,
                        theme::current(),
                        theme::current_treatment(),
                    );
                }
            });
            Some(handle)
        }
        Err(error) => {
            log::error!("failed to open tray popup: {error}");
            None
        }
    }
}
