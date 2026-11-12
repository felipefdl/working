# crates/app

GPUI desktop app (`working`). One main window plus optional Meet call windows.

## Conventions

- `Store` is the one entity that holds core state and the lists views render. Views own presentation state and send `Command`s; they do not talk HTTP.
- Widgets come from gpui-kit (`use gpui_kit::*;` is the kit prelude). Colors come from `theme.rs` through `rgb()`; no literal colors in other files.
- Avatars and attachment thumbnails load through `Images` (core fetch, background decode). Views do not decode bytes on the UI thread or use GPUI's default asset loader for those URLs.
- Every `cx.observe` / `cx.subscribe` / `cx.observe_global` handle is stored in a struct field; a dropped `Subscription` is a silent no-op.
- Meet runs Google's web app in a webview with a Safari user agent (`call.rs`). `call.js` strips the page and drives the controls; the window chrome is ours. Session cookies go into the webview; rotated cookies are written back through `gchat_core::save_session_cookies`.
- Sign-in is an in-app webview (`welcome.rs`) that loads Google's account page, writes `cookies.json`, and sends `Command::Connect`. Cookie FFI for Meet and sign-in lives in `session.rs`.
- `crates/app/assets/logo.png` and `logo-light.png` are built by `just logo` from `logo/logo.svg`. Edit the SVG, then regenerate.
- Notifications and the Dock badge need the `.app` bundle (`just run`). A bare `cargo run -p working` binary has no notification center.
- Rewrite tools (`rewrite.rs`) run one headless job with no instruction files, skills, tools, MCP servers, hooks, plugins, or memory. Before adding or changing one, meet [docs/rewrite-tools.md](../../docs/rewrite-tools.md); Grok is out until it has a bare mode.
