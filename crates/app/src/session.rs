//! `WKWebView` cookie store: read and install the Google session.
#![allow(unsafe_code, reason = "WKWebView cookie FFI")]

use std::ptr::NonNull;

use gchat_core::SessionCookie;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
  NSArray, NSHTTPCookie, NSHTTPCookieDomain, NSHTTPCookieName, NSHTTPCookiePath, NSHTTPCookiePropertyKey,
  NSHTTPCookieSecure, NSHTTPCookieValue, NSMutableDictionary, NSString, ns_string,
};
use wry::WebViewExtMacOS;

/// Reads the session's cookies as the webview holds them now.
pub(crate) fn read(web: &wry::WebView) -> flume::Receiver<Vec<SessionCookie>> {
  let (read_tx, read_rx) = flume::bounded(1);
  let read = block2::RcBlock::new(move |cookies: NonNull<NSArray<NSHTTPCookie>>| {
    // SAFETY: WebKit hands over a valid array for the duration of the call.
    let cookies = unsafe { cookies.as_ref() };
    let session = cookies
      .iter()
      .filter_map(|cookie| {
        let domain = cookie.domain().to_string();
        matches!(domain.as_str(), ".google.com" | "chat.google.com" | "accounts.google.com").then(|| SessionCookie {
          name: cookie.name().to_string(),
          value: cookie.value().to_string(),
          domain,
          path: cookie.path().to_string(),
        })
      })
      .collect();
    let _ = read_tx.send(session);
  });
  // SAFETY: called on the main thread, on a live WKWebView.
  unsafe {
    web
      .webview()
      .configuration()
      .websiteDataStore()
      .httpCookieStore()
      .getAllCookies(&read);
  };
  read_rx
}

/// Puts the saved session's cookies into the webview. The returned channel gets a message once they are stored.
///
/// Goes to `WebKit`'s cookie store directly: `wry::WebView::set_cookie` drops the leading dot of a domain, which
/// turns `.google.com` cookies into ones for the bare `google.com` host that `meet.google.com` never receives.
/// It also waits by spinning the run loop, which re-enters GPUI while the app is borrowed and panics.
pub(crate) fn install(web: &wry::WebView) -> flume::Receiver<()> {
  let (done_tx, done_rx) = flume::bounded(1);
  // SAFETY: called on the main thread (a window build callback), on a live WKWebView.
  unsafe {
    let store = web.webview().configuration().websiteDataStore().httpCookieStore();
    let cookies: Vec<Retained<NSHTTPCookie>> = gchat_core::session_cookies()
      .iter()
      .filter_map(|cookie| {
        let name = NSString::from_str(&cookie.name);
        let value = NSString::from_str(&cookie.value);
        let path = NSString::from_str(&cookie.path);
        let domain = NSString::from_str(&cookie.domain);
        let properties: Retained<NSMutableDictionary<NSHTTPCookiePropertyKey, AnyObject>> =
          NSMutableDictionary::from_slices(
            &[NSHTTPCookieName, NSHTTPCookieValue, NSHTTPCookiePath, NSHTTPCookieDomain],
            &[&name, &value, &path, &domain],
          );
        properties.insert(NSHTTPCookieSecure, ns_string!("TRUE"));
        NSHTTPCookie::cookieWithProperties(&properties)
      })
      .collect();
    // The store applies writes in order, so the last one finishing means all of them have.
    let Some((last, rest)) = cookies.split_last() else {
      let _ = done_tx.send(());
      return done_rx;
    };
    for cookie in rest {
      store.setCookie_completionHandler(cookie, None);
    }
    let done = block2::RcBlock::new(move || {
      let _ = done_tx.send(());
    });
    store.setCookie_completionHandler(last, Some(&done));
  }
  done_rx
}
