//! Welcome and Google sign-in: shown while the core has no session.
#![allow(
  clippy::too_many_lines,
  reason = "sign-in webview setup and the two phases share one view"
)]

use std::sync::Arc;

use gchat_core::{Command, SessionCookie};
use gpui_kit::component::input::Escape;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_webview::WebView;
use wry::{NewWindowResponse, PageLoadEvent, WebViewBuilder};

use crate::root::TOP_BAR_HEIGHT;
use crate::session;
use crate::store::Store;
use crate::theme::{ACCENT, ACCENT_BG, BG, FIELD, MUTED, TEXT, is_light, rgb};

/// Meet and Google account pages accept Safari; a Chrome UA in `WKWebView` is often refused.
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15";
const LOGIN_URL: &str = "https://accounts.google.com/ServiceLogin?continue=https://chat.google.com/&hl=en";

enum Nav {
  Loaded(String),
  Open(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
  Welcome,
  SignIn,
}

pub(crate) struct WelcomeView {
  store: Entity<Store>,
  logo: [Arc<Image>; 2],
  phase: Phase,
  web: Option<Entity<WebView>>,
  error: Option<SharedString>,
  saving: bool,
  connecting: bool,
  _nav: Option<Task<()>>,
  _store: Subscription,
}

impl WelcomeView {
  pub(crate) fn new(store: Entity<Store>, logo: [Arc<Image>; 2], cx: &mut Context<Self>) -> Self {
    let _store = cx.observe(&store, |this, store, cx| {
      if !this.connecting {
        return;
      }
      if store.read(cx).signed_out.is_some() {
        this.connecting = false;
        this.saving = false;
        this.error = Some("Couldn't open Google Chat with that session. Sign in again.".into());
        cx.notify();
      }
    });
    Self {
      store,
      logo,
      phase: Phase::Welcome,
      web: None,
      error: None,
      saving: false,
      connecting: false,
      _nav: None,
      _store,
    }
  }

  fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.saving || self.phase == Phase::SignIn {
      return;
    }
    self.error = None;
    self.phase = Phase::SignIn;
    self.open_login(window, cx);
    cx.notify();
  }

  fn cancel(&mut self, cx: &mut Context<Self>) {
    if self.saving {
      return;
    }
    self.web = None;
    self._nav = None;
    self.phase = Phase::Welcome;
    cx.notify();
  }

  fn open_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let (nav_tx, nav_rx) = flume::unbounded::<Nav>();
    let (r, g, b) = if is_light() {
      (0xf0, 0xf4, 0xf9)
    } else {
      (0x13, 0x13, 0x14)
    };
    let builder = WebViewBuilder::new()
      .with_user_agent(USER_AGENT)
      .with_background_color((r, g, b, 255))
      .with_navigation_handler(|url| url.starts_with("https://") || url.starts_with("about:"))
      .with_on_page_load_handler({
        let nav_tx = nav_tx.clone();
        move |event, url| {
          if matches!(event, PageLoadEvent::Finished) {
            let _ = nav_tx.send(Nav::Loaded(url));
          }
        }
      })
      .with_new_window_req_handler(move |url, _| {
        let _ = nav_tx.send(Nav::Open(url));
        NewWindowResponse::Deny
      })
      .with_url(LOGIN_URL);
    let web = match WebView::build(builder, window, cx) {
      Ok(web) => web,
      Err(error) => {
        tracing::error!("failed to open the sign-in page: {error}");
        self.phase = Phase::Welcome;
        self.error = Some("Couldn't open the sign-in page.".into());
        return;
      },
    };
    let web = cx.new(|_| web);
    self.web = Some(web);
    self._nav = Some(cx.spawn_in(window, async move |this, cx| {
      while let Ok(nav) = nav_rx.recv_async().await {
        let cont = this.update(cx, |this, cx| match nav {
          Nav::Loaded(url) => {
            if chat_signed_in(&url) {
              this.capture(cx);
            }
            true
          },
          Nav::Open(url) => {
            if let Some(web) = &this.web {
              web.read(cx).raw().load_url(&url).ok();
            }
            true
          },
        });
        if !cont.unwrap_or(false) {
          break;
        }
      }
    }));
  }

  fn capture(&mut self, cx: &mut Context<Self>) {
    if self.saving {
      return;
    }
    let Some(web) = &self.web else {
      return;
    };
    let read = session::read(web.read(cx).raw());
    self.saving = true;
    cx.notify();
    self._nav = Some(cx.spawn(async move |this, cx| {
      let Ok(cookies) = read.recv_async().await else {
        this
          .update(cx, |this, cx| {
            this.saving = false;
            this.error = Some("Couldn't read the Google session.".into());
            cx.notify();
          })
          .ok();
        return;
      };
      let result = cx.background_executor().spawn(async move { persist_session(cookies) }).await;
      this
        .update(cx, |this, cx| match result {
          Ok(()) => {
            this.web = None;
            this.phase = Phase::Welcome;
            this.connecting = true;
            this.store.read(cx).core.send(Command::Connect);
            cx.notify();
          },
          Err(message) => {
            this.saving = false;
            this.error = Some(message);
            cx.notify();
          },
        })
        .ok();
    }));
  }

  fn bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
    let signing = self.phase == Phase::SignIn;
    h_flex()
      .id("welcome-bar")
      .h(px(TOP_BAR_HEIGHT))
      .w_full()
      .flex_shrink_0()
      .pl(px(92.))
      .pr_4()
      .items_center()
      .on_mouse_down(MouseButton::Left, |event, window, _| {
        if event.click_count == 2 {
          window.titlebar_double_click();
        } else {
          window.start_window_move();
        }
      })
      .child(
        div()
          .text_size(px(15.))
          .font_weight(FontWeight::SEMIBOLD)
          .text_color(rgb(TEXT))
          .child(if signing { "Sign in" } else { "Working" }),
      )
      .child(div().flex_1())
      .when(signing, |bar| {
        bar.child(
          div()
            .id("welcome-cancel")
            .px_3()
            .h(px(28.))
            .flex()
            .items_center()
            .rounded(px(8.))
            .text_size(px(13.))
            .text_color(rgb(MUTED))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(FIELD)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
            .child("Cancel"),
        )
      })
  }

  fn welcome_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
    let [dark, light] = &self.logo;
    let logo = if is_light() { light } else { dark };
    let busy = self.saving;
    let status = self.status_line(cx);
    v_flex()
      .flex_1()
      .w_full()
      .items_center()
      .justify_center()
      .gap_4()
      .child(img(logo.clone()).size(px(64.)).flex_shrink_0())
      .child(
        div()
          .text_size(px(28.))
          .font_weight(FontWeight::SEMIBOLD)
          .text_color(rgb(TEXT))
          .child("Working"),
      )
      .child(div().text_size(px(14.)).text_color(rgb(MUTED)).child("Google Chat on your Mac"))
      .child(
        div()
          .id("sign-in")
          .mt_2()
          .h(px(32.))
          .px_4()
          .min_w(px(168.))
          .flex()
          .items_center()
          .justify_center()
          .rounded(px(8.))
          .bg(rgb(ACCENT_BG))
          .text_size(px(13.))
          .text_color(rgb(ACCENT))
          .when(!busy, |button| {
            button
              .cursor_pointer()
              .hover(|style| style.opacity(0.85))
              .on_click(cx.listener(|this, _, window, cx| this.start(window, cx)))
          })
          .child(if busy {
            "Signing in"
          } else {
            "Sign in with Google"
          }),
      )
      .child(
        div()
          .h(px(36.))
          .w(px(320.))
          .flex()
          .items_center()
          .justify_center()
          .text_center()
          .text_size(px(12.))
          .text_color(rgb(MUTED))
          .children(status),
      )
  }

  fn status_line(&self, cx: &App) -> Option<SharedString> {
    if let Some(error) = &self.error {
      return Some(error.clone());
    }
    self
      .store
      .read(cx)
      .signed_out
      .as_deref()
      .filter(|text| text.contains("expired"))
      .map(SharedString::from)
  }
}

fn persist_session(cookies: Vec<SessionCookie>) -> Result<(), SharedString> {
  if !session_looks_complete(&cookies) {
    return Err("Google Chat did not finish signing in.".into());
  }
  gchat_core::write_session_cookies(&cookies)
    .map_err(|_| SharedString::from("Couldn't save the session. Try signing in again."))
}

fn chat_signed_in(url: &str) -> bool {
  let Some(rest) = url.strip_prefix("https://chat.google.com") else {
    return false;
  };
  rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') || rest.starts_with('#')
}

fn session_looks_complete(cookies: &[SessionCookie]) -> bool {
  cookies
    .iter()
    .any(|cookie| cookie.name == "SID" || cookie.name == "SAPISID" || cookie.name.ends_with("PSID"))
}

impl Render for WelcomeView {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let signing = self.phase == Phase::SignIn;
    v_flex()
      .size_full()
      .bg(rgb(BG))
      .text_color(rgb(TEXT))
      .key_context("Welcome")
      .on_action(cx.listener(|this, _: &Escape, _, cx| this.cancel(cx)))
      .child(self.bar(cx))
      .when(signing, |root| {
        root.child(div().flex_1().w_full().min_h_0().children(self.web.clone()))
      })
      .when(!signing, |root| root.child(self.welcome_body(cx)))
  }
}

#[cfg(test)]
mod tests {
  use super::{chat_signed_in, session_looks_complete};
  use gchat_core::SessionCookie;

  fn cookie(name: &str, domain: &str) -> SessionCookie {
    SessionCookie {
      name: name.into(),
      value: "x".into(),
      domain: domain.into(),
      path: "/".into(),
    }
  }

  #[test]
  fn chat_home_counts_as_signed_in() {
    assert!(chat_signed_in("https://chat.google.com/"));
    assert!(chat_signed_in("https://chat.google.com/u/0/#chat/home"));
    assert!(chat_signed_in("https://chat.google.com?pli=1"));
    assert!(!chat_signed_in(
      "https://accounts.google.com/ServiceLogin?continue=https://chat.google.com/"
    ));
    assert!(!chat_signed_in("https://chat.google.com.example/"));
  }

  #[test]
  fn session_needs_a_google_sid() {
    assert!(!session_looks_complete(&[]));
    assert!(!session_looks_complete(&[cookie("NID", ".google.com")]));
    assert!(session_looks_complete(&[cookie("SAPISID", ".google.com")]));
    assert!(session_looks_complete(&[cookie("__Secure-1PSID", ".google.com")]));
  }
}
