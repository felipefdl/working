//! Building a webview in a window: the one way the sign-in page, the Meet call, the viewer, and Mail's emails
//! make theirs.
//!
//! Every OS builds through `gpui_webview::WebView::build` at the end of the current update. What differs sits in a
//! file per OS: `unix.rs` (macOS, Linux) and `windows.rs`, where `WebView2` keeps its data in the app's own folder
//! and serves custom protocols under an HTTP host.

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

/// Builds a webview as a child of `window` and hands it to `ready`, on the view that asked for it, after the
/// current update.
///
/// `configure` sets the builder up (handlers, colors, custom protocols, initialization scripts, the first URL).
/// `ready` does not run when the view is gone by then.
pub fn build<V: 'static>(
  configure: impl for<'a> FnOnce(WebViewBuilder<'a>) -> WebViewBuilder<'a> + 'static,
  window: &Window,
  cx: &mut Context<V>,
  ready: impl FnOnce(&mut V, wry::Result<WebView>, &mut Window, &mut Context<V>) + 'static,
) {
  cx.defer_in(window, move |this, window, cx| {
    let web = os::with_builder(|builder| WebView::build(configure(builder), window, cx));
    ready(this, web, window, cx);
  });
}

/// Whether `url`, as a navigation handler hears it, is under `base`, an address of a custom protocol such as
/// `media://localhost/`: the name this OS's webview gives it (`WebView2` serves `media://localhost/` at
/// `http://media.localhost/`).
pub fn is_under(url: &str, base: &str) -> bool {
  os::is_under(url, base)
}
