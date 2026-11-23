//! macOS and Linux: wry's default builder, and custom protocols named as they were given.

use wry::WebViewBuilder;

/// Hands `build` a builder with wry's defaults.
pub fn with_builder<R>(build: impl FnOnce(WebViewBuilder<'_>) -> R) -> R {
  build(WebViewBuilder::new())
}

/// `WebKit` names a custom protocol's pages as they were given.
pub fn is_under(url: &str, base: &str) -> bool {
  url.starts_with(base)
}
