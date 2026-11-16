# Working

Native Google Chat client for macOS, Linux, and Windows. The GPUI app is `crates/app` (crate `working`); the Tokio engine is `crates/core` (crate `gchat-core`).

## Quality bar

- Gate: `just check`. Contract: [docs/rust-quality.md](docs/rust-quality.md). Machine config: `rustfmt.toml`, `clippy.toml`, `deny.toml`, `.cargo/config.toml`, root `[workspace.lints]`.
- Edition 2024, MSRV 1.100.0. Do not weaken workspace lints or deny gates to land a change.
- Mid-change: `just task <package>` or `just t '<nextest filter>'`. Completion claims still require `just check`.
- The gate builds for the host only: a change to per-OS code also needs clippy and tests on that OS (on Windows, the cargo commands behind `just task`). `.github/workflows/ci.yml` runs the gate on macOS plus clippy and tests on Linux and Windows for every push to `main` and every pull request.

## Conventions

- The UI talks to the core only through `Command` / `Event` on `gchat_core::Core`. HTTP, protobuf, cookies, and the WebChannel stream stay in `gchat-core`.
- Session and settings live in `gchat_core::config_dir()` (`cookies.json`, `settings.json`, `mcp-token`, and `session.log`, one line each time Google refuses the session): `~/.config/working` on macOS, `$XDG_CONFIG_HOME/working` on Linux, `%APPDATA%\working` on Windows. The disk cache is `cache_dir()`: `~/Library/Caches/working`, `$XDG_CACHE_HOME/working`, `%LOCALAPPDATA%\dev.felipefdl.working\cache`. Code reaches them only through `crates/core/src/paths.rs`. Those paths are machine-local: never commit, print, or paste cookie or token values.
- Live runs that were not asked to write to the account use `WORKING_READONLY=1` (`just run-readonly`, `just probe`). Send, read marks, reactions, typing, presence, and DND writes mutate the signed-in Google account.
- `WORKING_DEMO=1` (`just demo`) swaps the engine for `crates/core/src/demo`: a made-up workspace in memory that answers every `Command`, with replies, typing, slash commands, and card buttons. No sign-in, network, or disk cache; settings live under the temp dir. Use it to check UI features without an account. A new `Command` needs a demo answer too.
- Hard rule: tests, examples, fixtures, docs, comments, tool descriptions, and commit messages use made-up people (`Jordan Park`, `Riley Brooks`, `jpark@example.com`). Never a name, email, space, file, or message from a screenshot, the signed-in account, or the developer's own life, not even a first name. Real coworker names landed in `fuzzy.rs` tests and the MCP tool text once.
- `WORKING_TRACE=1` prints stream array types (and Meet screen-share shapes). Never message text, cookie values, or token values.
- The product is unshipped: replace a surface in place rather than keeping a compatibility path.
- When a dependency (GPUI, gpui-kit, wry, ...) lacks something, such as a corner radius on text highlights, do not work around it in app code. Stop and ask the developer whether to open an issue or a PR on that library. A workaround lands only when the feature is really needed, with an entry in [docs/workarounds.md](docs/workarounds.md) (what, why, the code location, and the upstream change that would remove it), so it can be dropped once the library catches up.
- New crates add a child `AGENTS.md` in the same change, with only local obligations. Protocol, cache, and MCP: [crates/core/AGENTS.md](crates/core/AGENTS.md). GPUI, Meet, and theming: [crates/app/AGENTS.md](crates/app/AGENTS.md). The public site (Bun, outside the Cargo workspace and `just check`): [website/AGENTS.md](website/AGENTS.md).
- `scripts/bundle.sh` signs `Working.app` with the Apple Development identity when the keychain has one. macOS ties camera and microphone grants to that signature; an ad-hoc signature changes every rebuild and drops them.

## Platforms

- Per-OS code sits behind a seam: a module whose `mod.rs` holds the shared signatures and passes each call to `macos.rs`, `linux.rs`, or `windows.rs` (`notify/`, `session/`, `updater/`; `webview/` has `unix.rs` for macOS and Linux), so callers never name an OS. Keys live in `keys.rs` (`secondary-` is Cmd on macOS, Ctrl elsewhere; `keys::text` prints a binding the way the OS writes it); window chrome in `chrome.rs`; paths in `gchat_core` `paths.rs`. A new OS difference goes into one of those places or a new seam, not as `#[cfg(target_os)]` scattered through views.
- macOS only: `Working.app`, screen sharing through `screen.rs`, the Permissions settings, named notification sounds, Dock reopen and the Hide / Window menus.
- Linux runs GPUI in X11 mode (XWayland on Wayland) with WebKitGTK through wry. Emoji are drawn with a bundled Noto Color Emoji (`emoji_font.rs`, [docs/workarounds.md](docs/workarounds.md)): cosmic-text and swash cannot draw COLRv1, Fedora's only color emoji font. Linux has no call window: Fedora's WebKitGTK has no WebRTC and wry refuses its media permission requests, so `call::open` (a module in `main.rs` there; `call.rs` is macOS and Windows only) hands every meeting to the default browser, and Settings hides the Built-in Google Meet and Camera switches.
- Linux AppImage desktop integration is opt-in, as the AppImage specification asks: `desktop_entry.rs` asks once with a card in the main window (the answer is `Settings::desktop_entry`; a switch under Appearance changes it), writes only `~/.local/share/applications/working.desktop` (marked `X-Working-Self-Integrated=true`) and its hicolor icon, and neither asks nor writes outside an AppImage, under the specification's opt-outs, or beside an entry it did not write.
- Windows uses WebView2 through wry, built from a GPUI task outside the app borrow (`webview/windows.rs`, [docs/workarounds.md](docs/workarounds.md)): wry's blocking build pumps window messages, which runs GPUI tasks while the app is borrowed and aborts the app. The installer and the exe are not Authenticode-signed.

## Release

- `just release <version>` is the whole release: it checks for a clean `main`, bumps `[workspace.package].version`, commits `chore(release): v<version>`, tags, and pushes. A version with a `-` (`0.0.7-rc1`) makes a draft prerelease to test by hand.
- `.github/workflows/publish.yml` (tag `v*`, one release at a time; a newer tag cancels an older run) builds every OS with `WORKING_UPDATES=1`:
  - macOS: each architecture on its own `macos-26` runner; `scripts/release.sh` joins them into a universal binary, signs it with Developer ID (hardened runtime, `crates/app/assets/Working.entitlements`), and notarizes the dmg once, stapling the ticket to the dmg and the app. Assets: the dmg, the `.app.tar.gz` and its `.sig`.
  - Linux: `scripts/package-linux.sh` on `ubuntu-24.04` and `ubuntu-24.04-arm`. Assets: `Working_<v>_{x86_64,aarch64}.AppImage` and their `.sig`. The AppImage carries the binary (with the emoji font inside), the emoji font's license, desktop entry, and icon only; GTK 3, WebKitGTK 4.1, and libsoup 3 come from the system, and the runner's Ubuntu sets the oldest glibc it runs on.
  - Windows: `scripts/package-windows.ps1` on `windows-latest` and `windows-11-arm`. Assets: the per-user NSIS installer `Working_<v>_{x64,arm64}-setup.exe` and its `.sig`.
  - The `publish` job (Linux) writes `latest.json` (the six `{os}-{arch}` keys of cargo-packager-updater, each with its file's signature; a missing or empty `.sig` fails the run), writes the notes with `scripts/release_notes.py` (conventional commits since the latest published release, in release-notes categories), creates the release with every asset, and publishes a stable tag as latest.
- Stable releases then run `.github/workflows/homebrew.yml` from the same workflow; it writes only `Casks/working.rb` in the shared `felipefdl/homebrew-tap` (deploy key `HOMEBREW_TAP_DEPLOY_KEY`). A release published by the workflow token triggers no other workflow, which is why it is called rather than triggered. Run it by hand (`workflow_dispatch`) for a release that is already published.
- Commit subjects become the notes: keep them conventional (`feat(app): …`, `fix(core): …`); `!` after the type lists the commit under Breaking changes.
- Secrets: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_CONTENT`, `PACKAGER_SIGN_PRIVATE_KEY`, `PACKAGER_SIGN_PRIVATE_KEY_PASSWORD`, `HOMEBREW_TAP_DEPLOY_KEY`.
- Self-update lives in `crates/app/src/updater/` (the shared loop and card in `mod.rs`, one file per OS): no setting, only builds with `WORKING_UPDATES=1` check. macOS replaces the bundle; Linux writes the new AppImage over `$APPIMAGE` (only an AppImage updates itself, and only when its folder is writable: otherwise it downloads nothing and logs why once per run) and relaunches it with no inherited descriptors, so the old image's mount goes away; Windows runs the downloaded installer (`/P /R /NS`, so a deleted Desktop shortcut stays deleted) at Restart from `%LOCALAPPDATA%\dev.felipefdl.working\updates`, which the next launch empties ([docs/workarounds.md](docs/workarounds.md)). The updater downloads the raw `.AppImage` and `-setup.exe`, never an archive. The minisign public key there must match `PACKAGER_SIGN_PRIVATE_KEY`. Settings shows the running version at the bottom of its sidebar.

## Commands

```bash
just check            # quality gate
just login            # optional Chrome sign-in; writes ~/.config/working/cookies.json (macOS, Linux)
just run              # release build: Working.app on macOS, the bare binary on Linux
just run-readonly     # same binary, WORKING_READONLY=1
just demo             # same binary, WORKING_DEMO=1: made-up workspace, no sign-in
just probe            # read-only core smoke: counts and event types, never message text
just logo             # macOS: top-bar PNGs, README icon, and working.ico from logo/ (resvg, Xcode, ImageMagick)
./scripts/package-linux.sh                       # AppImage into target/dist
pwsh scripts/package-windows.ps1                 # Windows: installer into target\dist; run the app with cargo run
```
