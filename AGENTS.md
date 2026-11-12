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
- Hard rule: tests, examples, fixtures, docs, comments, tool descriptions, and commit messages use made-up people (`Jordan Park`, `Riley Brooks`, `jpark@example.com`). Never a name, email, space, file, or message from a screenshot, the signed-in account, or the developer's own life, not even a first name. Real coworker names landed in `fuzzy.rs` tests and the MCP tool text once.
- `WORKING_TRACE=1` prints stream array types (and Meet screen-share shapes). Never message text, cookie values, or token values.
- The product is unshipped: replace a surface in place rather than keeping a compatibility path.
- When a dependency (GPUI, gpui-kit, wry, ...) lacks something, such as a corner radius on text highlights, do not work around it in app code. Stop and ask the developer whether to open an issue or a PR on that library. A workaround lands only when the feature is really needed, with an entry in [docs/workarounds.md](docs/workarounds.md) (what, why, the code location, and the upstream change that would remove it), so it can be dropped once the library catches up.
- New crates add a child `AGENTS.md` in the same change, with only local obligations. Protocol, cache, and MCP: [crates/core/AGENTS.md](crates/core/AGENTS.md). GPUI, Meet, and theming: [crates/app/AGENTS.md](crates/app/AGENTS.md).
- `scripts/bundle.sh` signs `Working.app` with the Apple Development identity when the keychain has one. macOS ties camera, microphone, and screen-recording grants to that signature; an ad-hoc signature changes every rebuild and drops them.

## Release

- `just release <version>` is the whole release: it checks for a clean `main`, bumps `[workspace.package].version`, commits `chore(release): v<version>`, tags, and pushes. A version with a `-` (`0.0.7-rc1`) makes a draft prerelease to test by hand.
- `.github/workflows/publish.yml` (tag `v*`, one release at a time; a newer tag cancels an older run): each architecture builds on its own `macos-26` runner with `WORKING_UPDATES=1`; `scripts/release.sh` joins them into a universal binary, signs it with Developer ID (hardened runtime, `crates/app/assets/Working.entitlements`), and notarizes the dmg once, stapling the ticket to the dmg and the app. A Linux job writes the notes with `scripts/release_notes.py` (conventional commits since the latest published release, in release-notes categories), creates the release with the dmg, the `.app.tar.gz` and its minisign `.sig`, and `latest.json`, and publishes a stable tag as latest.
- Stable releases then run `.github/workflows/homebrew.yml` from the same workflow; it writes only `Casks/working.rb` in the shared `felipefdl/homebrew-tap` (deploy key `HOMEBREW_TAP_DEPLOY_KEY`). A release published by the workflow token triggers no other workflow, which is why it is called rather than triggered. Run it by hand (`workflow_dispatch`) for a release that is already published.
- Commit subjects become the notes: keep them conventional (`feat(app): …`, `fix(core): …`); `!` after the type lists the commit under Breaking changes.
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
