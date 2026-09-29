# League Auto Accept

A focused Windows utility for League of Legends ready checks, being rewritten as a native
**Rust + GPUI** desktop application.

> **Status: Phase 1 — architecture spike.** This branch (`rewrite/rust-gpui`) is a ground-up
> rewrite of the existing Electron application. It currently contains only the scaffolding that
> proves the technical foundations; the League service, ready-check state machine, settings
> system and Discord notifications are not implemented yet. The released Electron implementation
> remains available at tag `v0.1.0` and on `main`.

## What it will do

- Detect a running League Client and authenticate against its loopback LCU API.
- Monitor LCU events over a WebSocket and automatically accept ready checks.
- Persist user settings (including `minimizeToTray`).
- Send user-configured Discord webhook notifications, downstream of auto-accept.
- Provide a polished native GPUI window and an always-present system tray.

It will remain an ordinary user-mode application: no administrator privileges, no kernel or
service components, no telemetry, and no remote content.

## Requirements

- Windows 10/11 (x64)
- Rust (stable) with the MSVC toolchain and Visual Studio Build Tools (`link.exe`)
- No League installation is required to build.

## Development

```powershell
cargo run
```

## Building

```powershell
cargo build --release
```

## Testing

```powershell
cargo test
```

The test suite runs without a live League client. The LCU transport is verified against a
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
GPUI UI
   -> application state / actions
      -> background async service (tokio)
         -> League service -> LCU
```

The UI never owns LCU credentials or performs networking. See the local `PORTING.md` (agent-only,
untracked) for the full reconnaissance and design.

## Security model

- LCU credentials stay in the backend layer; the UI cannot access them.
- Only the fixed League Client endpoints are used.
- Discord webhook URLs are validated; requests are constrained and do not follow redirects.
- No analytics, telemetry, update service, or remote content.

## License

MIT — see [LICENSE](LICENSE). Not endorsed by Riot Games.
