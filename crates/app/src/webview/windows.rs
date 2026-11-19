//! Windows: `WebView2` built from a task that does not hold the app, then handed to `gpui_webview::WebView::new`.
#![allow(unsafe_code, reason = "a borrowed window handle and a WebView2 COM call")]
//!
//! wry's blocking build waits for `WebView2` by pumping the window messages, and GPUI's Windows backend runs its
//! tasks from those messages: inside a GPUI update a task that reaches the app panics on the borrowed app. So the
//! webview is built with `build_as_child_async` from a GPUI task, which `WebView2`'s answers wake through the
//! normal message loop. The page's own scripts are added here without waiting (`AddScriptToExecuteOnDocumentCreated`
//! answers through a handler) and the first page loads after their answers. wry still waits once, while the task
//! runs and the app is free: for the `window.ipc` script it adds to every webview.

use gpui_kit::{Context, Window};
use gpui_webview::WebView;
use webview2_com::AddScriptToExecuteOnDocumentCreatedCompletedHandler;
use windows::core::HSTRING;
use wry::raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
use wry::{WebViewBuilder, WebViewExtWindows};

pub fn build<V: 'static>(
  configure: impl for<'a> FnOnce(WebViewBuilder<'a>) -> WebViewBuilder<'a> + 'static,
  scripts: Vec<&'static str>,
  url: Option<String>,
  window: &Window,
  cx: &mut Context<V>,
  ready: impl FnOnce(&mut V, wry::Result<WebView>, &mut Window, &mut Context<V>) + 'static,
) {
  // `Window` has an inherent `window_handle` too, GPUI's own handle.
  let parent = HasWindowHandle::window_handle(window).map(|handle| Parent(handle.as_raw()));
  cx.spawn_in(window, async move |this, cx| {
    let built = match parent {
      Ok(parent) => open(&parent, configure, scripts, url).await,
      Err(error) => Err(wry::Error::WindowHandleError(error)),
    };
    // A view that is gone drops the webview with this closure.
    cx.update(|window, cx| {
      this.update(cx, |this, cx| {
        let web = built.map(|web| WebView::new(web, window, cx));
        ready(this, web, window, cx);
      })
    })
    .ok();
  })
  .detach();
}

#[expect(
  clippy::future_not_send,
  reason = "WebView2 belongs to the UI thread; a GPUI foreground task runs this"
)]
async fn open(
  parent: &Parent,
  configure: impl for<'a> FnOnce(WebViewBuilder<'a>) -> WebViewBuilder<'a>,
  scripts: Vec<&'static str>,
  url: Option<String>,
) -> wry::Result<wry::WebView> {
  // `WebView2` keeps its data (cookies, cache, storage) in the app's own folder rather than beside the executable,
  // which an install under Program Files cannot write.
  let mut context = wry::WebContext::new(Some(gchat_core::webview_dir()));
  let web = configure(WebViewBuilder::new_with_web_context(&mut context))
    .build_as_child_async(parent)
    .await?;
  let added: Vec<_> = scripts
    .into_iter()
    .map(|script| add_script(&web, script))
    .collect::<Result<_, _>>()?;
  for answer in added {
    // A closed channel means `WebView2` dropped the handler, which happens only as the webview goes away.
    if let Ok(result) = answer.recv_async().await {
      result?;
    }
  }
  if let Some(url) = url {
    web.load_url(&reachable(&url))?;
  }
  Ok(web)
}

/// Adds `script` to every document the webview opens from now on, without waiting: the channel gets
/// `WebView2`'s answer.
fn add_script(web: &wry::WebView, script: &str) -> windows::core::Result<flume::Receiver<windows::core::Result<()>>> {
  let (answer_tx, answer_rx) = flume::bounded(1);
  let handler = AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(move |result, _id| {
    let _ = answer_tx.send(result);
    Ok(())
  }));
  // SAFETY: a live `WebView2`, on the thread that made it; it calls the handler later, on this thread.
  unsafe {
    web
      .webview()
      .AddScriptToExecuteOnDocumentCreated(&HSTRING::from(script), &handler)
  }?;
  Ok(answer_rx)
}

/// `WebView2` serves a custom protocol (`media://localhost/…`) at `http://media.localhost/…`. wry rewrites a
/// builder's URL that way, but not one loaded later.
fn reachable(url: &str) -> String {
  match url.split_once("://") {
    Some((scheme, rest)) if !matches!(scheme, "http" | "https") => format!("http://{scheme}.{rest}"),
    _ => url.to_owned(),
  }
}

/// `WebView2` names a custom protocol's pages by their HTTP host (`reachable`).
pub fn is_under(url: &str, base: &str) -> bool {
  url.starts_with(&reachable(base))
}

/// The GPUI window's handle, kept for the build, which outlives the update the handle was read in.
struct Parent(RawWindowHandle);

impl HasWindowHandle for Parent {
  fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
    // SAFETY: a Win32 window handle names a window rather than pointing at memory: if the window closes during the
    // build, the calls made with it fail instead of reaching freed memory.
    Ok(unsafe { WindowHandle::borrow_raw(self.0) })
  }
}

#[cfg(test)]
mod tests {
  use super::{is_under, reachable};

  #[test]
  fn custom_protocols_load_through_their_http_host() {
    assert_eq!(reachable("media://localhost/index.html"), "http://media.localhost/index.html");
    assert_eq!(reachable("https://accounts.google.com/"), "https://accounts.google.com/");
    assert_eq!(reachable("about:blank"), "about:blank");
  }

  #[test]
  fn a_custom_protocol_page_is_known_by_its_http_host() {
    assert!(is_under("http://mailbody.localhost/", "mailbody://localhost/"));
    assert!(is_under("http://mailbody.localhost/#top", "mailbody://localhost/"));
    assert!(!is_under("https://example.com/", "mailbody://localhost/"));
  }
}
