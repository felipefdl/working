# Working

Native macOS Google Chat client. The GPUI app is `crates/app` (crate `working`); the Tokio engine is `crates/core` (crate `gchat-core`).

## Quality bar

- Gate: `just check`. Contract: [docs/rust-quality.md](docs/rust-quality.md). Machine config: `rustfmt.toml`, `clippy.toml`, `deny.toml`, `.cargo/config.toml`, root `[workspace.lints]`.
- Edition 2024, MSRV 1.99.1. Do not weaken workspace lints or deny gates to land a change.
- Mid-change: `just task <package>` or `just t '<nextest filter>'`. Completion claims still require `just check`.

## Conventions

- The UI talks to the core only through `Command` / `Event` on `gchat_core::Core`. HTTP, protobuf, cookies, and the WebChannel stream stay in `gchat-core`.
- Session and settings live under `~/.config/working` (`cookies.json`, `settings.json`, `mcp-token`). The disk cache is `~/Library/Caches/working`. Those paths are machine-local: never commit, print, or paste cookie or token values.
- Live runs that were not asked to write to the account use `WORKING_READONLY=1` (`just run-readonly`, `just probe`). Send, read marks, reactions, typing, presence, and DND writes mutate the signed-in Google account.
- `WORKING_TRACE=1` prints stream array types (and Meet screen-share shapes). Never message text, cookie values, or token values.
- The product is unshipped: replace a surface in place rather than keeping a compatibility path.
- New crates add a child `AGENTS.md` in the same change, with only local obligations. Protocol, cache, and MCP: [crates/core/AGENTS.md](crates/core/AGENTS.md). GPUI, Meet, and theming: [crates/app/AGENTS.md](crates/app/AGENTS.md).
- `scripts/bundle.sh` signs `Working.app` with the Apple Development identity when the keychain has one. macOS ties camera, microphone, and screen-recording grants to that signature; an ad-hoc signature changes every rebuild and drops them.

## Commands

```bash
just check            # quality gate
just login            # optional Chrome sign-in; writes ~/.config/working/cookies.json
just run              # release Working.app and open it
just run-readonly     # same binary, WORKING_READONLY=1
just probe            # read-only core smoke: counts and event types, never message text
just logo             # rebuild top-bar PNGs from logo/logo.svg (needs resvg)
```
