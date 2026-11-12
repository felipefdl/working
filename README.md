<br/>
<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="crates/app/assets/logo.png">
    <img src="crates/app/assets/logo-light.png" width="200px" alt="Working"></img>
  </picture>
</p>

# Working

Native macOS client for Google Chat.

Repo: [felipefdl/working](https://github.com/felipefdl/working)

---

Working is a GPUI desktop app. It signs in with a Google Chat web session, keeps the protocol and cache in a Tokio engine, and draws spaces, DMs, Meet calls, and notifications as a Mac app.

It is independent of Google.

## Prerequisites

- macOS 13 or newer
- Rust **1.99.1** (workspace `rust-version`)
- [just](https://github.com/casey/just)
- Xcode (`actool`) to wrap `Working.app`

## Quick start

```bash
git clone https://github.com/felipefdl/working.git
cd working
just run
```

`just run` builds `Working.app` and opens it. Sign in with Google on the welcome screen. That writes `~/.config/working/cookies.json`.

`just login` still opens Chrome for the same session file, if you want to sign in outside the app. It needs Google Chrome, [agent-browser](https://github.com/vercel-labs/agent-browser) (`npm i -g agent-browser && agent-browser install`), and python3.

An Apple Development signing identity in the keychain keeps camera, microphone, and screen-recording grants across rebuilds. Without one, the bundle is ad-hoc signed and those grants reset every build.

## Commands

```bash
just login            # optional Chrome sign-in; writes ~/.config/working/cookies.json
just run              # release Working.app and open it
just run-readonly     # same binary, WORKING_READONLY=1
just probe            # read-only core smoke: counts and event types, never message text
just check            # quality gate
just logo             # rebuild top-bar PNGs from logo/logo.svg (needs resvg)
```

`WORKING_READONLY=1` refuses sends, read marks, reactions, typing, presence, and DND writes. Use it for live runs that should not change the Google account.

Session files live in `~/.config/working` (`cookies.json`, `settings.json`, `mcp-token`). The disk cache is `~/Library/Caches/working`. Do not commit, print, or paste cookie or token values.

## MCP

The app can serve a loopback MCP server (default `http://127.0.0.1:3021/mcp`) so local agents can read, search, and optionally send as the signed-in user. Turn it on in Settings. The secret is `~/.config/working/mcp-token`. The server listens on localhost only.

## Layout

| Path | Role |
|------|------|
| `crates/app` | GPUI app (crate `working`), bundled as `Working.app` |
| `crates/core` | Tokio engine (crate `gchat-core`): session, protobuf, WebChannel, cache, MCP |
| `docs/rust-quality.md` | Quality bar |
| `logo/` | Mark and app icon |
| `scripts/` | Login, bundle, logo |

The UI talks to the core only through `Command` / `Event` on `gchat_core::Core`.

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) and the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

Working is **MIT** (Copyright (c) 2026 Felipe Lima). See [LICENSE.md](LICENSE.md).
