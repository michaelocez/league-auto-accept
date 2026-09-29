# League Auto Accept

A focused Windows Electron utility for League of Legends ready checks.

## Current status

The initial release candidate provides a focused Auto Accept and notification utility:

- sandboxed Electron renderer with context isolation and no Node integration;
- narrow, typed preload bridge;
- versioned local settings with Windows-backed encryption for the webhook URL
  when Electron's OS encryption is available;
- one-page settings interface;
- optional minimize/close-to-tray behaviour;
- single-instance lifecycle;
- automatic Windows Riot installation and running lockfile discovery;
- authenticated, loopback-only League Client health checks;
- bounded LCU WebSocket framing with handshake verification and reconnects;
- live League Client status in the window and tray;
- a main-process ready-check state machine that continues while the window is hidden;
- duplicate-event coalescing and one accept request at a time;
- cancellation when Auto Accept is disabled or the League Client disconnects;
- one bounded retry after a failed accept request;
- live ready-check status in the window and tray;
- optional queue-pop, successful auto-accept, and in-game Discord notifications;
- exactly one notification of each enabled type per matching lifecycle transition;
- editable message templates with a `{mentions}` placeholder;
- an explicit allowlist of up to five Discord user mentions with local nicknames;
- a Test Webhook action with success and failure feedback;
- strict Discord webhook URL validation, bounded HTTPS requests, and no redirects;
- an original application and tray icon;
- an as-invoker Windows installer with optional install location and shortcuts;
- settings validation and persistence tests.

Discord delivery is downstream of Auto Accept: lifecycle notifications are
fire-and-forget, rejected promises are contained, and even a synchronous
notification failure cannot delay, retry, or change the accept result.

## Development

Requirements: Node.js 22.12 or newer and npm 10 or newer.

```powershell
npm install
npm run dev
```

Validation:

```powershell
npm run verify
```

Create the Windows installer:

```powershell
npm run package:win
```

The installer is written to `release/`. The application does not request
administrator privileges or configure itself to launch when Windows starts.
Minimize-to-tray is enabled by default and can be turned off in the app.

Local development builds are not digitally signed and may therefore trigger a
Windows SmartScreen warning. A publicly distributed release should be signed
with a trusted Windows code-signing certificate before publication.

## Local data and outbound requests

Settings are stored in Electron's per-user application-data directory. The
Discord webhook URL uses Windows-backed encryption when Electron reports that
OS encryption is available. Other preferences, Discord user IDs, and local
nicknames are stored as ordinary local settings. Nicknames identify saved IDs
inside the app; they do not rename users or get transmitted to Discord.

The application talks to the League Client only through its authenticated
loopback API. It contacts Discord only when an enabled notification fires or
the user presses **Test Webhook**. It has no analytics or update service.

## Security model

The renderer cannot access Node.js, the filesystem, Electron IPC primitives, or
network credentials directly. Main-process IPC handlers accept calls only from
the current application window. Navigation, popups, webviews, permissions, and
remote content are denied by default.

LCU credentials and webhook delivery remain in the main process. The renderer
can request a test through one narrow IPC method, but it cannot send arbitrary
URLs or payloads: the main process reads the normalized saved settings and
validates them again before delivery.

Discord webhook delivery sends the rendered message and configured Discord user
IDs to the selected Discord webhook. `allowed_mentions` disables automatic
parsing and permits only those explicit user IDs, so text such as `@everyone`
does not create an unintended mass mention.

Auto Accept calls only the fixed League Client endpoint
`/lol-matchmaking/v1/ready-check/accept`. It coalesces repeated events, permits
one request at a time, and stops after two total attempts for a ready-check
cycle. No live matchmaking or queue acceptance is performed by the automated
test suite.

## Attribution

Architectural patterns are adapted from League Profile Tool under the MIT
License. League Auto Accept is independently authored by michaelocez; upstream
contributors did not participate in this project. See the project
[license](LICENSE) and [third-party notices](NOTICE.md).
