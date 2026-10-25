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
- **Discord notifications**: webhook alerts for match found,
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
pre-installed and leaves no install footprint — to remove it, delete the file. What it creates is its
own per-user settings file (see [Configuration](#configuration)) plus, only when a write is refused or
a settings file cannot be read, a diagnostic beside it: `settings.json.invalid`, a transient
`settings.json.bak` or `settings-write-probe.tmp`, and `settings-error.log` (past 64 KiB the live log
is rotated to `settings-error.log.1`, and the new one starts empty).

There is **no application log file**: the recent log records live in memory only and die with the
process. When something actually fails at error level — or the Settings banner reports a refused write
— the Settings page grows a **Diagnostics** section saying how many errors were recorded and showing
the most recent one, with **Copy diagnostics** (the app version, the Windows version and build, and the
record tail, with the Discord webhook URL, any LCU credential and the Windows account name redacted)
and **Open an issue**. A session with nothing wrong shows no such section.

> The executable is unsigned, so Windows SmartScreen may warn on first run ("More info" →
> "Run anyway"). Code signing is out of scope for now.

## Usage

- **Enable Auto Accept** from the main window or the tray popup. It only accepts ready checks while
  the League Client is connected; the UI shows `Active`, `Connecting`, `League offline`, or
  `Disabled` so you always know the real state.
- **Notifications**: open the Notifications page, enable Discord notifications, paste a Discord
  webhook URL and press **Save** (the field starts empty and the saved URL is never shown back),
  optionally add people to mention (nickname + Discord user ID), and use **Test Webhook** to
  verify. Each lifecycle event — match found, ready check accepted, game started — has its own
  switch and its own editable message (use `{mentions}` where the mentioned people should appear;
  up to 5 are ever mentioned). A fresh install has only **ready check accepted** switched on, so
  match-found and game-started notifications stay silent until you enable their switches.
- **Appearance**: on the Settings page choose **Dark** or **Light**, and a window backdrop —
  **Opaque**, **Mica**, or **Acrylic** (default). Your choices are saved and restored on next
  launch.

## Configuration

Settings are stored as JSON at:

```
%APPDATA%\League Auto Accept\settings.json
```

The only secret is the Discord webhook URL. It is protected with the OS codec (DPAPI on Windows),
written only through the settings store's typed, validating setter, and read only by the notification
sender. If the OS refuses to protect it, the change is rejected and the webhook already stored stays
in use — the URL is never written in the clear. It is never part of application state, the tray or the
rendered UI: the window shows only whether a webhook is configured and its host. Your theme and window
backdrop are stored here too, so they are restored on next launch.

The app must be able to write that file. If the write is refused — the directory is read-only, or the
app was started from a sandboxed/low-integrity context — the Settings and Notifications pages show a
banner naming the file and the error, so nothing fails silently. A refused **webhook** change is not
applied at all: the webhook already stored stays in use, the field keeps what you typed so it can be
retried, and the banner says so. Other changes still take effect for the session and are lost when
the app exits. Run the app from an ordinary user session to persist changes.

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

The test suite runs without a live League client. The LCU transport is exercised end to end against
self-signed mock TLS servers on loopback, including the Basic-auth header, response parsing and the
WebSocket handshake; as with the real LCU, certificate verification is disabled for that loopback
connection only.

## Validation

```powershell
cargo fmt --check
cargo check --all-targets
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Formatting, lints and the test suite are enforced in CI on every pull request and on `main`; the
release workflow re-runs the suite in release mode and builds the portable executable.

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

The window layer is `gpui-kit` 0.7 (`gpui = { package = "gpui-kit", … }` in `Cargo.toml`), a
repackaged snapshot of GPUI rather than Zed's own crates.io crate; it is aliased to `gpui` so the
imports read normally. The pin is deliberate — GPUI is pre-1.0, so it moves only as its own change,
with the full validation suite and a Windows build behind it.

## Security model

- LCU credentials stay in the privileged backend layer; the UI cannot access them — the password is
  private, `Debug` output redacts it, and the change-detection signature is a digest.
- Only the League Client endpoints the app uses are reachable: the request builder matches an
  explicit endpoint allowlist and exposes a closed set of calls.
- The Discord webhook URL is written through a typed, validating setter and read only by the
  notification sender; it is absent from application state and redacted in `Debug` output.
- Discord webhook URLs are validated; outbound requests are constrained and do not follow redirects.
- No background network requests: the app makes none at startup, on a timer, or while it is closed.
- Opening **Settings** asks GitHub's public API **once per session** for the latest release tag. That
  request carries no query parameters and no identifying information — just the app name and version
  as its `User-Agent` — and the answer is used only to say whether a newer version exists. It never
  downloads or runs anything, failures are silent, and a link from the answer is opened only if it is
  on this project's own GitHub path. Otherwise the fixed releases page is used.
- No analytics, telemetry, updater, or remote content.

### What this app asks of the League client

Everything the app does with League runs over the loopback connection described by the client's own
lockfile. It makes exactly four requests, each on a fixed allowlist matched by exact equality — there
is no arbitrary-endpoint path:

| Request | Why |
| --- | --- |
| `GET /lol-summoner/v1/current-summoner` | Confirms and authenticates the connection when it connects |
| `GET /lol-gameflow/v1/gameflow-phase` | The current game phase, for notifications |
| `GET /lol-matchmaking/v1/ready-check` | The ready-check snapshot taken on connect |
| `POST /lol-matchmaking/v1/ready-check/accept` | The only state-changing call: accepting a ready check |

It also subscribes to the client's event stream. That subscription is the client's global JSON API
event feed, and the app filters it in memory down to the two events it acts on — ready checks and
game-phase changes. The subscription is read-only and does not widen the request surface above; the
named event scopes are community knowledge rather than documented API, so it is deliberately left
global rather than narrowed on a guess.

The app talks only to `127.0.0.1` (that local client), to the Discord webhook you configure yourself,
and — once per session, when you open Settings — to GitHub's public API for the latest release tag.
It sends no telemetry, runs no other process, and the only thing it changes in the League client is
accepting a ready check you would otherwise have clicked. Riot does not document or support
third-party use of this API; this app is an ordinary user-mode program.

## License

MIT — see [LICENSE](LICENSE).

League Auto Accept isn't endorsed by Riot Games and doesn't reflect the views or opinions of Riot
Games or anyone officially involved in producing or managing Riot Games properties. Riot Games and
all associated properties are trademarks or registered trademarks of Riot Games, Inc.
