//! Windows: the `WebView2` cookie manager, reached through wry's `WebViewExtWindows`.
#![allow(unsafe_code, reason = "WebView2 cookie manager COM calls")]
//!
//! Goes to the cookie manager directly: `wry::WebView::set_cookie` drops the leading dot of a domain, which turns
//! `.google.com` cookies into ones for the bare `google.com` host that `meet.google.com` never receives, and
//! `wry::WebView::cookies` waits by pumping the message loop, which re-enters GPUI while the app is borrowed.

use gchat_core::SessionCookie;
use webview2_com::GetCookiesCompletedHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::{
  ICoreWebView2_2, ICoreWebView2Cookie, ICoreWebView2CookieList, ICoreWebView2CookieManager,
};
use windows::core::{HSTRING, Interface, PCWSTR, PWSTR};
use wry::WebViewExtWindows;

fn manager(web: &wry::WebView) -> windows::core::Result<ICoreWebView2CookieManager> {
  let webview = web.webview().cast::<ICoreWebView2_2>()?;
  // SAFETY: a live WebView2, on the thread that made it.
  unsafe { webview.CookieManager() }
}

/// Reads the session's cookies as the webview holds them now. The channel closes without an answer when the
/// webview cannot list them.
pub fn read(web: &wry::WebView) -> flume::Receiver<Vec<SessionCookie>> {
  let (read_tx, read_rx) = flume::bounded(1);
  let handler = GetCookiesCompletedHandler::create(Box::new(move |result, list| {
    result?;
    let cookies = list.map_or_else(|| Ok(Vec::new()), |list| session(&list))?;
    let _ = read_tx.send(cookies);
    Ok(())
  }));
  // A null URI lists every cookie of the profile.
  // SAFETY: a live cookie manager; WebView2 calls the handler later, on this thread.
  if let Err(error) = manager(web).and_then(|manager| unsafe { manager.GetCookies(PCWSTR::null(), &handler) }) {
    tracing::warn!("could not read the webview's cookies: {error}");
  }
  read_rx
}

/// The session's cookies in `list`: those of `.google.com`, `chat.google.com`, and `accounts.google.com`.
fn session(list: &ICoreWebView2CookieList) -> windows::core::Result<Vec<SessionCookie>> {
  let mut count = 0;
  // SAFETY: a live cookie list, handed to the completion handler.
  unsafe { list.Count(&raw mut count) }?;
  let mut cookies = Vec::new();
  for index in 0..count {
    // SAFETY: `index` is below the list's count.
    let cookie = unsafe { list.GetValueAtIndex(index) }?;
    // SAFETY: each getter fills a string the callee allocated, which `text` takes and frees.
    let domain = text(|out| unsafe { cookie.Domain(out) })?;
    if matches!(domain.as_str(), ".google.com" | "chat.google.com" | "accounts.google.com") {
      cookies.push(SessionCookie {
        // SAFETY: as above.
        name: text(|out| unsafe { cookie.Name(out) })?,
        // SAFETY: as above.
        value: text(|out| unsafe { cookie.Value(out) })?,
        domain,
        // SAFETY: as above.
        path: text(|out| unsafe { cookie.Path(out) })?,
      });
    }
  }
  Ok(cookies)
}

/// A string from a `WebView2` getter that allocates it for the caller.
fn text(get: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>) -> windows::core::Result<String> {
  let mut value = PWSTR::null();
  get(&raw mut value)?;
  Ok(webview2_com::take_pwstr(value))
}

/// Puts the saved session's cookies into the webview. The returned channel gets a message once all of them are
/// added: a cookie counts for the requests made after `AddOrUpdateCookie`, and the first page load waits for it.
pub fn install(web: &wry::WebView) -> flume::Receiver<()> {
  let (done_tx, done_rx) = flume::bounded(1);
  match manager(web) {
    Ok(manager) => {
      for cookie in gchat_core::session_cookies() {
        if let Err(error) = add(&manager, &cookie) {
          tracing::warn!("could not store a session cookie in the webview: {error}");
        }
      }
    },
    Err(error) => tracing::warn!("could not reach the webview's cookies: {error}"),
  }
  let _ = done_tx.send(());
  done_rx
}

fn add(manager: &ICoreWebView2CookieManager, cookie: &SessionCookie) -> windows::core::Result<()> {
  // SAFETY: a live cookie manager, and a cookie it made.
  unsafe {
    let made: ICoreWebView2Cookie = manager.CreateCookie(
      &HSTRING::from(cookie.name.as_str()),
      &HSTRING::from(cookie.value.as_str()),
      &HSTRING::from(cookie.domain.as_str()),
      &HSTRING::from(cookie.path.as_str()),
    )?;
    made.SetIsSecure(true)?;
    manager.AddOrUpdateCookie(&made)
  }
}
