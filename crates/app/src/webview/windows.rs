//! Windows: `WebView2` keeps its data in the app's own folder, and serves custom protocols under an HTTP host.

use wry::WebViewBuilder;

/// Hands `build` a builder whose `WebView2` keeps its data (cookies, cache, storage) in `gchat_core::webview_dir()`
/// rather than beside the executable, which an install under Program Files cannot write.
pub fn with_builder<R>(build: impl FnOnce(WebViewBuilder<'_>) -> R) -> R {
  let mut context = wry::WebContext::new(Some(gchat_core::webview_dir()));
  build(WebViewBuilder::new_with_web_context(&mut context))
}

/// `WebView2` names a custom protocol's pages by their HTTP host: `media://localhost/…` is
/// `http://media.localhost/…`.
pub fn is_under(url: &str, base: &str) -> bool {
  match base.split_once("://") {
    Some((scheme, rest)) if !matches!(scheme, "http" | "https") => url.starts_with(&format!("http://{scheme}.{rest}")),
    _ => url.starts_with(base),
  }
}

#[cfg(test)]
mod tests {
  use super::is_under;

  #[test]
  fn a_custom_protocol_page_is_known_by_its_http_host() {
    assert!(is_under("http://mailbody.localhost/", "mailbody://localhost/"));
    assert!(is_under("http://mailbody.localhost/#top", "mailbody://localhost/"));
    assert!(!is_under("https://example.com/", "mailbody://localhost/"));
    assert!(is_under("https://accounts.google.com/x", "https://accounts.google.com/"));
  }
}
