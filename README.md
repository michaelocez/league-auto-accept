# League Auto Accept

A small, native Windows utility that watches the League of Legends client and automatically accepts
ready checks, with optional Discord webhook notifications. Built with **Rust + GPUI** as a
portable, single-executable app.

It is an ordinary user-mode application: no installer, no administrator privileges, no background
service or driver, no telemetry, and no remote content in the UI.

## Features

- **Detects the League Client** automatically (install discovery + lockfile) and authenticates
  against its loopback LCU API.
- **Monitors ready checks** over the LCU WebSocket and **auto-accepts** them — one accept per ready
  check, with bounded retries, cancellation, and reconnect handling.
- **Discord notifications**: webhook alerts for queue popped,
  auto-accepted, and game-started, with optional `{mentions}` of up to five Discord users.
- **Appearance**: **Dark** and **Light** themes, plus a window backdrop of **Opaque**,
  **Mica** (subtle, samples the desktop wallpaper) or **Acrylic** (blurs what is behind the window
  in real time). Mica/Acrylic require Windows 11; elsewhere the window stays opaque.
- **Always-present system tray**: live status, plus quick toggles for **Auto Accept** and
  **Discord notifications**: in both the left-click popup and the native right-click menu; with
  Open and Quit.
- **Settings** persist per user (Auto Accept state, theme, window backdrop, minimize-to-tray,
  Discord config, mentions).
- **Single instance**; minimize/close-to-tray is configurable.

## Download and run

1. Download `league-auto-accept.exe` from the
   [latest release](https://github.com/michaelocez/league-auto-accept/releases/latest).
2. Put it anywhere and run it. That's it.

It is **portable**: a single self-contained executable (statically linked C runtime, embedded
icons), with **no installer and no setup**. It runs on a clean Windows machine with nothing
pre-installed and leaves no install footprint — to remove it, delete the file. The only thing it
creates is its own per-user settings file (see [Configuration](#configuration)).

> The executable is unsigned, so Windows SmartScreen may warn on first run ("More info" →
> "Run anyway"). Code signing is out of scope for now.

## Usage

- **Enable Auto Accept** from the main window or the tray popup. It only accepts ready checks while
  the League Client is connected; the UI shows `Active`, `Connecting`, `League offline`, or
  `Disabled` so you always know the real state.
- **Notifications**: open the Notifications page, enable Discord notifications, paste a Discord
  webhook URL, optionally add people to mention (nickname + Discord user ID), and use **Test
  Webhook** to verify.
- **Appearance**: on the Settings page choose **Dark** or **Light**, and a window backdrop —
  **Opaque** (default), **Mica**, or **Acrylic**. Your choices are saved and restored on next
  launch.

## Configuration

Settings are stored as JSON at:

```
%APPDATA%\League Auto Accept\settings.json
```

The only secret is the Discord webhook URL; it is encrypted with the OS codec where available and
is never logged or exposed to the UI layer. Your theme and window backdrop are stored here too, so
they are restored on next launch.

## Requirements

- Windows 10/11 (x64).

That's it for running the release build. No League installation is required to build or run.

### Building from source

- Rust (stable) with the MSVC toolchain and Visual Studio Build Tools (`link.exe`).

## Development

```powershell
cargo run            # debug build; opens a console for logs
```

## Building a release

```powershell
cargo build --release
# -> target\release\league-auto-accept.exe
```

The release profile is configured for portable distribution: the C runtime is statically linked
(`.cargo/config.toml`) and the binary is a GUI-subsystem app (no console window), with the
application icon embedded into the executable.

## Testing

```powershell
cargo test
```

The test suite runs without a live League client; the LCU transport is verified against a
self-signed mock server.

## Validation

```powershell
cargo fmt --check
cargo check --all-targets
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

## Architecture

```text
GPUI views
   -> application state / actions
      -> background async service (tokio)
         -> League service -> LCU (REST + WebSocket)
         -> Discord notifier
```

The UI observes application state and emits intents; it never owns LCU credentials or performs
networking. The League backend runs on a background thread and communicates with the UI over a
channel, so the client being closed or restarted never blocks or crashes the app.

## Security model

- LCU credentials stay in the privileged backend layer; the UI cannot access them.
- Only the fixed League Client ready-check endpoints are used.
- Discord webhook URLs are validated; outbound requests are constrained and do not follow redirects.
- No analytics, telemetry, updater, or remote content.

## License

MIT — see [LICENSE](LICENSE).

League Auto Accept isn't endorsed by Riot Games and doesn't reflect the views or opinions of Riot
Games or anyone officially involved in producing or managing Riot Games properties. Riot Games and
all associated properties are trademarks or registered trademarks of Riot Games, Inc.
