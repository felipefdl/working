//! macOS and Linux: `gpui_webview::WebView::build`, inside a GPUI update at the end of the current one.

use gpui_kit::{Context, Window};
use gpui_webview::WebView;
use wry::WebViewBuilder;

pub fn build<V: 'static>(
  configure: impl for<'a> FnOnce(WebViewBuilder<'a>) -> WebViewBuilder<'a> + 'static,
  scripts: Vec<&'static str>,
  url: Option<String>,
  window: &Window,
  cx: &mut Context<V>,
  ready: impl FnOnce(&mut V, wry::Result<WebView>, &mut Window, &mut Context<V>) + 'static,
) {
  cx.defer_in(window, move |this, window, cx| {
    let mut builder = configure(WebViewBuilder::new());
    for script in scripts {
      builder = builder.with_initialization_script(script);
    }
    if let Some(url) = url {
      builder = builder.with_url(url);
    }
    ready(this, WebView::build(builder, window, cx), window, cx);
  });
}
