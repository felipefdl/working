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
- When a dependency (GPUI, gpui-kit, wry, ...) lacks something, such as a corner radius on text highlights, do not work around it in app code. Stop and ask the developer whether to open an issue or a PR on that library. A workaround lands only when the feature is really needed, with an entry in [docs/workarounds.md](docs/workarounds.md) (what, why, the code location, and the upstream change that would remove it), so it can be dropped once the library catches up.
- New crates add a child `AGENTS.md` in the same change, with only local obligations. Protocol, cache, and MCP: [crates/core/AGENTS.md](crates/core/AGENTS.md). GPUI, Meet, and theming: [crates/app/AGENTS.md](crates/app/AGENTS.md).
- `scripts/bundle.sh` signs `Working.app` with the Apple Development identity when the keychain has one. macOS ties camera, microphone, and screen-recording grants to that signature; an ad-hoc signature changes every rebuild and drops them.

## Release

- `just release <version>` bumps `[workspace.package].version`, commits `chore(release): v<version>`, and tags. Push the tag (`git push origin HEAD --tags`), review the draft release, then publish it.
- `.github/workflows/publish.yml` (tag `v*`, `macos-26`) runs `scripts/release.sh`: universal binary built with `WORKING_UPDATES=1`, Developer ID signature with hardened runtime and `crates/app/assets/Working.entitlements`, notarized and stapled. It uploads `Working_<v>_universal.dmg`, `.app.tar.gz` plus minisign `.sig`, and `latest.json` to a draft release.
- Publishing the release runs `.github/workflows/homebrew.yml`, which writes only `Casks/working.rb` in the shared `felipefdl/homebrew-tap` (deploy key `HOMEBREW_TAP_DEPLOY_KEY`). The tap holds every felipefdl app.
- Secrets: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_CONTENT`, `PACKAGER_SIGN_PRIVATE_KEY`, `PACKAGER_SIGN_PRIVATE_KEY_PASSWORD`, `HOMEBREW_TAP_DEPLOY_KEY`.
- Self-update lives in `crates/app/src/updater.rs`: no setting, only builds with `WORKING_UPDATES=1` check. The minisign public key there must match `PACKAGER_SIGN_PRIVATE_KEY`.

## Commands

```bash
just check            # quality gate
just login            # optional Chrome sign-in; writes ~/.config/working/cookies.json
just run              # release Working.app and open it
just run-readonly     # same binary, WORKING_READONLY=1
just probe            # read-only core smoke: counts and event types, never message text
just logo             # rebuild top-bar PNGs from logo/logo.svg (needs resvg)
```
