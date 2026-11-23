//! The webviews that show the HTML of the emails shown whole in the Mail view's reading pane: one each, built through
//! `webview::build`, serving the page `mail_page::page` makes under the `mailbody` protocol. Images come through the
//! core (`Core::fetch`), links go to the default browser, and the page's script reports the email's height, the
//! wheel, and Escape. A webview is a native view drawn over GPUI's, so they are hidden while a window overlay covers
//! the panel (`MailView::set_covered`) and dropped as soon as their email is not shown whole, another thread opens,
//! the theme or `Settings::mail_other_background` changes, or Mail leaves the screen.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use gchat_core::Core;
use gpui_kit::*;
use gpui_webview::WebView;
use wry::http::Response;
use wry::{NewWindowResponse, RequestAsyncResponder};

use crate::mail::MailView;
use crate::mail_page::{self, Colors, PAGE_PATH, PAGE_URL, Report, SCHEME, image_request, image_type};
use crate::settings::Settings;
use crate::theme::{ACCENT, PANEL, TEXT, hex};
use crate::webview;

/// How tall an email's card is until its page says.
pub const START_HEIGHT: f32 = 24.0;

/// An email shown by a webview.
pub struct Body {
  /// Which build this is: an answer for an older one is dropped.
  generation: u64,
  /// Empty until the webview is built.
  pub web: Option<Entity<WebView>>,
  /// How tall the email is, as its page last said.
  pub height: f32,
  /// The webview could not be built: the card shows the email's text instead.
  pub failed: bool,
}

/// What a page asks of the app, for the email `id`.
pub struct PageEvent {
  id: String,
  kind: PageEventKind,
}

enum PageEventKind {
  Report(Report),
  /// A link was followed: it opens in the browser, never in the page.
  Open(String),
}

/// The bodies of a Mail view and the ways their pages reach it.
pub struct Bodies {
  shown: HashMap<String, Body>,
  /// The colors the pages were made with: other colors make them again.
  colors: Option<Colors>,
  /// A window overlay covers the panel: the webviews are hidden.
  covered: bool,
  generation: u64,
  events: flume::Sender<PageEvent>,
  images: flume::Sender<(String, RequestAsyncResponder)>,
}

impl Bodies {
  /// Bodies whose pages report on `events` and ask for their images on `images`, which `MailView::new` serves.
  pub fn new(events: flume::Sender<PageEvent>, images: flume::Sender<(String, RequestAsyncResponder)>) -> Self {
    Self {
      shown: HashMap::new(),
      colors: None,
      covered: false,
      generation: 0,
      events,
      images,
    }
  }

  /// Drops every webview.
  pub fn clear(&mut self) {
    self.shown.clear();
  }

  /// The body of email `id`, when its webview is built and shows it.
  pub fn get(&self, id: &str) -> Option<&Body> {
    self.shown.get(id).filter(|body| !body.failed)
  }
}

/// The colors of the theme in use that a page follows.
fn theme_colors() -> Colors {
  Colors {
    background: hex(PANEL),
    text: hex(TEXT),
    link: hex(ACCENT),
  }
}

/// The colors the pages are made with: the theme's, or the other ones (`Colors::other`) while
/// `Settings::mail_other_background` is on.
fn page_colors(cx: &App) -> Colors {
  let colors = theme_colors();
  if cx.global::<Settings>().mail_other_background {
    colors.other()
  } else {
    colors
  }
}

/// The background the pages are made with, as `0xRRGGBB`, while it is not the theme's
/// (`Settings::mail_other_background`).
pub fn other_background(cx: &App) -> Option<u32> {
  cx.global::<Settings>()
    .mail_other_background
    .then(|| theme_colors().other().background)
}

impl MailView {
  /// Keeps one webview for each of `wanted`, the ids of the emails shown whole that have HTML, building the missing
  /// ones and dropping the others. A change of theme or of `Settings::mail_other_background` makes every page again.
  pub(crate) fn sync_bodies(&mut self, wanted: &[String], window: &Window, cx: &mut Context<Self>) {
    let colors = page_colors(cx);
    if self.bodies.colors != Some(colors) {
      self.bodies.clear();
      self.bodies.colors = Some(colors);
    }
    let keep: HashSet<&str> = wanted.iter().map(String::as_str).collect();
    self.bodies.shown.retain(|id, _| keep.contains(id.as_str()));
    for id in wanted {
      if !self.bodies.shown.contains_key(id) {
        self.build_body(id.clone(), colors, window, cx);
      }
    }
  }

  fn build_body(&mut self, id: String, colors: Colors, window: &Window, cx: &mut Context<Self>) {
    let messages = self.store.read(cx).mail.messages.clone();
    let Some(message) = messages
      .as_deref()
      .and_then(|messages| messages.iter().find(|message| message.id == id))
    else {
      return;
    };
    let page = mail_page::page(&message.html, colors);
    self.bodies.generation += 1;
    let generation = self.bodies.generation;
    self.bodies.shown.insert(
      id.clone(),
      Body {
        generation,
        web: None,
        height: START_HEIGHT,
        failed: false,
      },
    );
    let images = self.bodies.images.clone();
    let serve = move |_: wry::WebViewId, request: wry::http::Request<Vec<u8>>, responder: RequestAsyncResponder| {
      let uri = request.uri();
      if uri.path() == PAGE_PATH {
        let response = Response::builder()
          .header("Content-Type", "text/html; charset=utf-8")
          .header("Content-Security-Policy", mail_page::CSP)
          .body(Cow::Owned(page.clone().into_bytes()));
        respond(responder, response);
      } else if let Some(url) = image_request(uri.path(), uri.query()) {
        // The core fetches it; `MailView::new` answers.
        let _ = images.send((url, responder));
      } else {
        respond(responder, Response::builder().status(404).body(Cow::Borrowed(&[][..])));
      }
    };
    let events = &self.bodies.events;
    let (reports, opens, windows) = (events.clone(), events.clone(), events.clone());
    let (report_id, open_id, window_id) = (id.clone(), id.clone(), id.clone());
    let ipc = move |request: wry::http::Request<String>| {
      if let Some(report) = Report::parse(request.body()) {
        let _ = reports.send(PageEvent {
          id: report_id.clone(),
          kind: PageEventKind::Report(report),
        });
      }
    };
    // The page itself, and the anchors inside it, stay; any other address leaves for the browser.
    let navigate = move |url: String| {
      if webview::is_under(&url, PAGE_URL) || url == "about:blank" {
        return true;
      }
      let _ = opens.send(PageEvent {
        id: open_id.clone(),
        kind: PageEventKind::Open(url),
      });
      false
    };
    let new_window = move |url: String, _| {
      let _ = windows.send(PageEvent {
        id: window_id.clone(),
        kind: PageEventKind::Open(url),
      });
      NewWindowResponse::Deny
    };
    let background = colors.background;
    webview::build(
      move |builder| {
        builder
          .with_focused(false)
          .with_background_color((channel(background, 16), channel(background, 8), channel(background, 0), 255))
          .with_asynchronous_custom_protocol(SCHEME.into(), serve)
          .with_ipc_handler(ipc)
          .with_navigation_handler(navigate)
          .with_new_window_req_handler(new_window)
          .with_initialization_script(mail_page::SCRIPT)
          .with_url(PAGE_URL)
      },
      window,
      cx,
      move |this, web, _, cx| this.body_ready(&id, generation, web, cx),
    );
  }

  fn body_ready(&mut self, id: &str, generation: u64, web: wry::Result<WebView>, cx: &mut Context<Self>) {
    let covered = self.hides_bodies();
    // An email no longer shown whole, or shown again since, drops the webview here.
    let Some(body) = self
      .bodies
      .shown
      .get_mut(id)
      .filter(|body| body.generation == generation && body.web.is_none())
    else {
      return;
    };
    match web {
      Ok(web) => {
        if covered {
          web.set_visible(false).ok();
        }
        body.web = Some(cx.new(|_| web));
      },
      Err(error) => {
        tracing::warn!("mail: the email's webview could not be built: {error}");
        body.failed = true;
      },
    }
    cx.notify();
  }

  /// Hides the webviews while a window overlay (settings, a picker, a menu, the media viewer) covers the panel, and
  /// shows them again after: they are native views, drawn over whatever GPUI draws there.
  pub fn set_covered(&mut self, covered: bool, cx: &App) {
    if covered == self.bodies.covered {
      return;
    }
    self.bodies.covered = covered;
    self.apply_cover(cx);
  }

  /// Whether a window overlay covers the panel, as the root last said.
  pub const fn covered(&self) -> bool {
    self.bodies.covered
  }

  /// Whether the webviews are hidden: a window overlay covers the panel, or the labels popover is open over them.
  const fn hides_bodies(&self) -> bool {
    self.bodies.covered || self.labels_open
  }

  /// Shows or hides every webview as `hides_bodies` says: after the covering or the labels popover changed.
  pub(crate) fn apply_cover(&self, cx: &App) {
    let hidden = self.hides_bodies();
    for web in self.bodies.shown.values().filter_map(|body| body.web.as_ref()) {
      web.read(cx).set_visible(!hidden).ok();
    }
  }

  /// What a page asked: its email's height, a turn of the wheel, Escape, or a link to open.
  pub(crate) fn on_page_event(&mut self, event: PageEvent, window: &mut Window, cx: &mut Context<Self>) {
    match event.kind {
      PageEventKind::Report(Report::Height(height)) => {
        if let Some(body) = self.bodies.shown.get_mut(&event.id) {
          body.height = height.max(START_HEIGHT);
          cx.notify();
        }
      },
      PageEventKind::Report(Report::Scroll(delta)) => self.scroll_reader(delta, cx),
      PageEventKind::Report(Report::Close) => {
        self.dismiss(window, cx);
      },
      PageEventKind::Open(url) => {
        if mail_page::opens_in_browser(&url) {
          cx.open_url(&url);
        }
      },
    }
  }
}

/// Answers a page's image from the core, or with 404 when it cannot be fetched.
pub async fn serve_image(core: Core, url: String, responder: RequestAsyncResponder) {
  let response = match core.fetch(url).await {
    Ok(bytes) => Response::builder()
      .header("Content-Type", image_type(&bytes))
      .body(Cow::Owned(bytes.to_vec())),
    Err(_) => Response::builder().status(404).body(Cow::Borrowed(&[][..])),
  };
  respond(responder, response);
}

/// One byte of a `0xRRGGBB` color, `shift` bits up.
fn channel(color: u32, shift: u32) -> u8 {
  u8::try_from((color >> shift) & 0xff).unwrap_or(u8::MAX)
}

/// Answers a request of a page. A response that could not be built answers empty.
fn respond(responder: RequestAsyncResponder, response: wry::http::Result<Response<Cow<'static, [u8]>>>) {
  responder.respond(response.unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..]))));
}
