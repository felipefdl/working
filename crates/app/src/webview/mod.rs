//! Building a webview in a window: the one way the sign-in page, the Meet call, and the viewer make theirs.
//!
//! Each OS has a file of its own: `unix.rs` (macOS, Linux) builds through `gpui_webview::WebView::build` inside a
//! GPUI update; `windows.rs` builds `WebView2` from a task that does not hold the app, because waiting on
//! `WebView2` inside an update re-enters GPUI while the app is borrowed (`docs/workarounds.md`). Either way the
//! result reaches `ready`, after the current update, so callers never name an OS.

#[cfg(unix)]
mod unix;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(unix)]
use unix as os;
#[cfg(target_os = "windows")]
use windows as os;

use gpui_kit::{Context, Window};
use gpui_webview::WebView;
use wry::WebViewBuilder;

/// Builds a webview as a child of `window` and hands it to `ready`, on the view that asked for it.
///
/// `configure` sets the builder up (handlers, colors, custom protocols); every `scripts` entry runs at the start
/// of each document, before the page's own code; `url`, when given, is the first page, loaded once the scripts are
/// in place. `ready` does not run when the view is gone by then: the webview is dropped instead.
pub fn build<V: 'static>(
  configure: impl for<'a> FnOnce(WebViewBuilder<'a>) -> WebViewBuilder<'a> + 'static,
  scripts: Vec<&'static str>,
  url: Option<String>,
  window: &Window,
  cx: &mut Context<V>,
  ready: impl FnOnce(&mut V, wry::Result<WebView>, &mut Window, &mut Context<V>) + 'static,
) {
  os::build(configure, scripts, url, window, cx, ready);
}
