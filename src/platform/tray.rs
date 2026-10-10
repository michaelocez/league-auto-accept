//! System tray.
//!
//! The tray is **always present while the app runs**, independent of any setting (decision A1).
//! `minimizeToTray` only changes what closing the window does.
//!
//! tray-icon requires a message loop running on the thread that owns the icon (Windows/Linux).
//! To avoid colliding with GPUI's own loop, the tray lives on a dedicated thread with its own
//! Win32 message pump and forwards commands to the UI through an `async_channel`.

use std::sync::{Arc, RwLock};

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

use crate::app::events::AppEvent;
use crate::config::settings::AppSettings;

/// Commands the tray can issue. They map to user intents, not state mutations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    ShowWindow,
    /// Open (or refresh) the custom GPUI tray popup near the icon.
    OpenPopup,
    ToggleAutoAccept,
    ToggleDiscordNotifications,
    Quit,
}

const TRAY_ICON_PNG: &[u8] = include_bytes!("../../assets/tray-icon.png");

/// Creates the tray on a dedicated thread and returns immediately.
///
/// `settings` is the shared, authoritative settings snapshot (the same one the main window and
/// popup write). The native right-click menu's check items mirror it, so the fallback menu can
/// never disagree with the app.
pub fn spawn(tx: async_channel::Sender<AppEvent>, settings: Arc<RwLock<AppSettings>>) {
    let spawned = std::thread::Builder::new()
        .name("laa-tray".to_string())
        .spawn(move || {
            if let Err(error) = run(tx, settings) {
                log::error!("tray thread failed: {error}");
            }
        });
    if let Err(error) = spawned {
        log::error!("could not spawn tray thread: {error}");
    }
}

fn load_icon() -> Result<Icon, String> {
    let image = image::load_from_memory(TRAY_ICON_PNG)
        .map_err(|error| error.to_string())?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).map_err(|error| error.to_string())
}

fn run(
    tx: async_channel::Sender<AppEvent>,
    settings: Arc<RwLock<AppSettings>>,
) -> Result<(), String> {
    let icon = load_icon()?;

    let (auto_accept_on, discord_on) = settings
        .read()
        .map(|s| (s.auto_accept_enabled, s.discord_notifications_enabled))
        .unwrap_or((false, false));

    let menu = Menu::new();
    let show = MenuItem::new("Show League Auto Accept", true, None);
    // Seed the check items from the real state so they are correct even before the first toggle.
    let toggle = CheckMenuItem::new("Auto Accept", true, auto_accept_on, None);
    let discord = CheckMenuItem::new("Discord notifications", true, discord_on, None);
    let quit = MenuItem::new("Quit", true, None);
    menu.append(&show).map_err(|error| error.to_string())?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|error| error.to_string())?;
    menu.append(&toggle).map_err(|error| error.to_string())?;
    menu.append(&discord).map_err(|error| error.to_string())?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|error| error.to_string())?;
    menu.append(&quit).map_err(|error| error.to_string())?;

    let show_id = show.id().clone();
    let toggle_id = toggle.id().clone();
    let discord_id = discord.id().clone();
    let quit_id = quit.id().clone();
    let menu_tx = tx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let command = if event.id == show_id {
            Some(TrayCommand::ShowWindow)
        } else if event.id == toggle_id {
            Some(TrayCommand::ToggleAutoAccept)
        } else if event.id == discord_id {
            Some(TrayCommand::ToggleDiscordNotifications)
        } else if event.id == quit_id {
            Some(TrayCommand::Quit)
        } else {
            None
        };
        if let Some(command) = command {
            let _ = menu_tx.try_send(AppEvent::Tray(command));
        }
    }));

    // Left-click opens our custom popup; right-click still shows the native menu (system fallback).
    let click_tx = tx.clone();
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            let _ = click_tx.try_send(AppEvent::Tray(TrayCommand::OpenPopup));
        }
    }));

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("League Auto Accept")
        .with_icon(icon)
        .build()
        .map_err(|error| error.to_string())?;

    log::info!("tray icon created");
    // Keep the native check items in step with the authoritative settings. muda items are not
    // `Send` (they hold an `Rc<RefCell<..>>`), so the update must happen on this thread — poll the
    // shared settings from the message loop, which wakes at least every `SYNC_INTERVAL_MS`.
    run_message_loop(move || {
        let (auto_accept_on, discord_on) = settings
            .read()
            .map(|s| (s.auto_accept_enabled, s.discord_notifications_enabled))
            .unwrap_or((false, false));
        if toggle.is_checked() != auto_accept_on {
            toggle.set_checked(auto_accept_on);
        }
        if discord.is_checked() != discord_on {
            discord.set_checked(discord_on);
        }
    });
    Ok(())
}

/// Minimal Win32 message pump for the tray icon's message window. It wakes on a message or every
/// `SYNC_INTERVAL_MS`, drains the queue, then calls `sync` so menu state can be refreshed.
fn run_message_loop(mut sync: impl FnMut()) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG,
        PM_REMOVE, QS_ALLINPUT,
    };

    const SYNC_INTERVAL_MS: u32 = 250;
    let mut message: MSG = unsafe { std::mem::zeroed() };
    loop {
        unsafe {
            MsgWaitForMultipleObjects(0, std::ptr::null(), 0, SYNC_INTERVAL_MS, QS_ALLINPUT);
            while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        sync();
    }
}
