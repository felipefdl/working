//! Linux: the `WebKitGTK` cookie manager of the webview's website data manager.
//!
//! Goes to the cookie manager directly rather than through `wry::WebView::cookies` and `set_cookie`: those wait for
//! `WebKit`'s answer by spinning the GTK main loop, which re-enters GPUI while the app is borrowed. Here every call
//! answers through its callback, which runs when gpui-webview next drives the GTK loop.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gchat_core::SessionCookie;
use webkit2gtk::{CookieManager, CookieManagerExt, WebViewExt, WebsiteDataManagerExt};
use wry::WebViewExtUnix;

/// The pages whose cookies make up the session: each answer holds the domain cookies (`.google.com`) and the
/// host's own.
const PAGES: [&str; 2] = ["https://chat.google.com/", "https://accounts.google.com/"];

/// The webview's cookie manager. Every wry webview has a website data manager, and with it a cookie manager.
fn manager(web: &wry::WebView) -> Option<CookieManager> {
  web.webview().website_data_manager().and_then(|data| data.cookie_manager())
}

/// The session cookie a `WebKit` cookie is, when it belongs to the session's domains.
fn session_cookie(mut cookie: soup::Cookie) -> Option<SessionCookie> {
  let domain = cookie.domain()?.to_string();
  if !matches!(domain.as_str(), ".google.com" | "chat.google.com" | "accounts.google.com") {
    return None;
  }
  Some(SessionCookie {
    name: cookie.name()?.to_string(),
    value: cookie.value()?.to_string(),
    domain,
    path: cookie.path().map_or_else(|| "/".to_owned(), |path| path.to_string()),
  })
}

/// Adds the cookies of one page to those read so far, each once.
fn merge(read: &mut Vec<SessionCookie>, cookies: Vec<soup::Cookie>) {
  for cookie in cookies.into_iter().filter_map(session_cookie) {
    let held = read
      .iter()
      .any(|seen| seen.name == cookie.name && seen.domain == cookie.domain && seen.path == cookie.path);
    if !held {
      read.push(cookie);
    }
  }
}

/// Reads the session's cookies as the webview holds them now: what it would send to Chat and to the account pages.
pub(crate) fn read(web: &wry::WebView) -> flume::Receiver<Vec<SessionCookie>> {
  let (read_tx, read_rx) = flume::bounded(1);
  let Some(manager) = manager(web) else {
    let _ = read_tx.send(Vec::new());
    return read_rx;
  };
  let read = Rc::new(RefCell::new(Vec::new()));
  let pending = Rc::new(Cell::new(PAGES.len()));
  for page in PAGES {
    let read = Rc::clone(&read);
    let pending = Rc::clone(&pending);
    let read_tx = read_tx.clone();
    manager.cookies(page, None::<&webkit2gtk::gio::Cancellable>, move |cookies| {
      match cookies {
        Ok(cookies) => merge(&mut read.borrow_mut(), cookies),
        Err(error) => tracing::warn!("reading the webview's cookies failed: {error}"),
      }
      pending.set(pending.get().saturating_sub(1));
      if pending.get() == 0 {
        let _ = read_tx.send(read.take());
      }
    });
  }
  read_rx
}

#[cfg(test)]
mod tests {
  use super::*;

  fn cookie(name: &str, domain: &str, path: &str) -> soup::Cookie {
    soup::Cookie::new(name, "value", domain, path, -1)
  }

  #[test]
  fn keeps_the_leading_dot_and_only_session_domains() {
    let mut read = Vec::new();
    merge(
      &mut read,
      vec![
        cookie("SID", ".google.com", "/"),
        cookie("OSID", "chat.google.com", "/"),
        cookie("NID", ".youtube.com", "/"),
      ],
    );
    let domains: Vec<_> = read.iter().map(|cookie| cookie.domain.as_str()).collect();
    assert_eq!(domains, [".google.com", "chat.google.com"]);
  }

  #[test]
  fn a_cookie_both_pages_send_is_read_once() {
    let mut read = Vec::new();
    merge(
      &mut read,
      vec![cookie("SID", ".google.com", "/"), cookie("OSID", "chat.google.com", "/")],
    );
    merge(
      &mut read,
      vec![cookie("SID", ".google.com", "/"), cookie("OSID", "accounts.google.com", "/")],
    );
    assert_eq!(read.len(), 3);
  }
}
