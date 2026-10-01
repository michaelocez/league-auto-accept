//! System tray.
//!
//! The tray is **always present while the app runs**, independent of any setting (decision A1).
//! `minimizeToTray` only changes what closing the window does.
//!
//! tray-icon requires a message loop running on the thread that owns the icon (Windows/Linux).
//! To avoid colliding with GPUI's own loop, the tray lives on a dedicated thread with its own
//! Win32 message pump and forwards commands to the UI through an `async_channel`.

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

use crate::app::events::AppEvent;

/// Commands the tray can issue. They map to user intents, not state mutations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    ShowWindow,
    /// Open (or refresh) the custom GPUI tray popup near the icon.
    OpenPopup,
    ToggleAutoAccept,
    Quit,
}

const TRAY_ICON_PNG: &[u8] = include_bytes!("../../assets/tray-icon.png");

/// Creates the tray on a dedicated thread and returns immediately.
pub fn spawn(tx: async_channel::Sender<AppEvent>) {
    let spawned = std::thread::Builder::new()
        .name("laa-tray".to_string())
        .spawn(move || {
            if let Err(error) = run(tx) {
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

fn run(tx: async_channel::Sender<AppEvent>) -> Result<(), String> {
    let icon = load_icon()?;

    let menu = Menu::new();
    let show = MenuItem::new("Show League Auto Accept", true, None);
    let toggle = CheckMenuItem::new("Auto Accept", true, false, None);
    let quit = MenuItem::new("Quit", true, None);
    menu.append(&show).map_err(|error| error.to_string())?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|error| error.to_string())?;
    menu.append(&toggle).map_err(|error| error.to_string())?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|error| error.to_string())?;
    menu.append(&quit).map_err(|error| error.to_string())?;

    let show_id = show.id().clone();
    let toggle_id = toggle.id().clone();
    let quit_id = quit.id().clone();
    let menu_tx = tx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let command = if event.id == show_id {
            Some(TrayCommand::ShowWindow)
        } else if event.id == toggle_id {
            Some(TrayCommand::ToggleAutoAccept)
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
    run_message_loop();
    Ok(())
}

/// Minimal Win32 message pump that keeps the tray icon's message window serviced.
fn run_message_loop() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage, MSG,
    };

    let mut message: MSG = unsafe { std::mem::zeroed() };
    loop {
        let result = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
