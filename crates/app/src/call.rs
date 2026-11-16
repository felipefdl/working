//! A Google Meet call in a window of its own.
//!
//! Audio and video are Google's: the Meet web app runs in a webview, signed in with the chat session. What the
//! user sees is ours: `call.js` strips the page down to the video tiles and joins by itself, with the camera
//! kept away from the page until the camera button here turns it on,
//! and this window draws the top bar and the controls, which click Meet's own buttons through that script. A
//! button in the top bar brings Google's interface back, for anything the stripped view hides.
//!
//! The call's chat is Meet's too: the script reads the messages out of Meet's chat panel and writes into its
//! field, and the panel beside the tiles here draws them.
//!
//! Closing the window drops the webview, which leaves the call.

use gpui_kit::component::input::{Escape, InputEvent, Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;
use gpui_webview::WebView;

use crate::call_chat::{Chat, ChatMessage};
use crate::chrome;
#[cfg(target_os = "macos")]
use crate::screen::{self, End, Frame, Share};
use crate::session;
use crate::settings::Settings;
use crate::theme::{self, ACCENT, ACCENT_BG, BG, BORDER, FIELD, GREEN, HANGUP, MUTED, OFF_BG, TEXT, Tone, WHITE, rgb};

use crate::ui::{Icon, icon};
use crate::webview;

/// Meet supports Safari; the default agent of macOS's `WKWebView` has no browser name and gets the "unsupported"
/// page. Windows' `WebView2` is Chromium and keeps its own agent. Linux has no call window: meetings open in the
/// browser there.
#[cfg(not(target_os = "windows"))]
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15";

const TOP_BAR_HEIGHT: f32 = 44.0;
const CONTROLS_HEIGHT: f32 = 68.0;
/// The chat panel takes this share of the window's width, within these limits.
const CHAT_SHARE: f32 = 0.3;
const CHAT_WIDTH: std::ops::RangeInclusive<f32> = 240.0..=360.0;
/// The widest the control buttons get, side by side: what is left of the bar beside them can show a message.
const CONTROLS_WIDTH: f32 = 460.0;
/// A message's preview beside the controls needs at least this much room.
const PREVIEW_MIN_WIDTH: f32 = 140.0;
/// What the notification of a chat message is filed under, before the meeting's code: a click on it comes back
/// with this key (`show_chat`).
const CHAT_KEY: &str = "meet-chat/";

/// The open call windows, where a clicked chat notification finds its call.
#[derive(Default)]
struct Calls(Vec<(AnyWindowHandle, WeakEntity<CallView>)>);

impl Global for Calls {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
  /// Before the call: Meet's "Ready to join?" page, which the script passes by itself.
  Lobby,
  Call,
  /// The call was left from inside the page.
  Ended,
}

/// What `call.js` last reported. A device is `None` when Meet has none to offer (no camera, no permission).
#[derive(Clone, Copy, PartialEq, Eq)]
struct CallState {
  phase: Phase,
  mic: Option<bool>,
  camera: Option<bool>,
  hand: bool,
  /// Our stripped view is showing (the default), rather than Meet's own web interface.
  custom: bool,
  /// The user is sharing their screen.
  presenting: bool,
  /// How loud the microphone is right now, 0 (silent or muted) to 4.
  voice: u8,
}

impl CallState {
  /// Parses the script's `phase|mic|camera|hand|custom|presenting|voice` report.
  fn parse(report: &str) -> Option<Self> {
    let mut parts = report.split('|');
    let phase = match parts.next()? {
      "lobby" => Phase::Lobby,
      "call" => Phase::Call,
      "ended" => Phase::Ended,
      _ => return None,
    };
    let device = |part: &str| match part {
      "1" => Some(true),
      "0" => Some(false),
      _ => None,
    };
    let mic = device(parts.next()?);
    let camera = device(parts.next()?);
    let hand = parts.next()? == "1";
    let custom = parts.next()? == "1";
    let presenting = parts.next()? == "1";
    let voice = parts.next()?.parse().ok()?;
    Some(Self {
      phase,
      mic,
      camera,
      hand,
      custom,
      presenting,
      voice,
    })
  }
}

struct CallView {
  /// Empty until the webview is built.
  web: Option<Entity<WebView>>,
  /// The meeting link as shared in the chat.
  url: String,
  code: SharedString,
  /// The link was just copied: the copy button says so for a moment.
  copied: bool,
  _copied_reset: Option<Task<()>>,
  state: CallState,
  chat: Chat,
  /// The chat panel is showing beside the tiles.
  chat_shown: bool,
  /// How many of the chat's messages the panel has shown: the ones after them count on the chat button.
  chat_seen: usize,
  chat_scroll: ScrollHandle,
  draft: Entity<TextareaState>,
  /// The text on its way to Meet: the field keeps it until the page says it went.
  sending: Option<String>,
  /// The last message did not go: the panel says so until the draft changes.
  send_failed: bool,
  /// A message that came while the panel was closed, shown beside the controls for a moment.
  preview: Option<usize>,
  _preview_reset: Option<Task<()>>,
  /// The notifications posted for messages that came while the window was in the background.
  notified: Vec<String>,
  /// The screen share, from the macOS picker to its end, with the tasks that pass its frames and its end to
  /// the page. Elsewhere the page shares through the webview's own `getDisplayMedia`.
  #[cfg(target_os = "macos")]
  share: Option<(Share, Task<()>, Task<()>)>,
  _reports: Task<()>,
  _session_sync: Task<()>,
  /// Turns the camera off when "Camera always off" is switched on during a call.
  _settings: Subscription,
  _draft: Subscription,
}

impl CallView {
  fn new(url: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let (report_tx, report_rx) = flume::unbounded::<String>();
    // `WORKING_TRACE=1`: the page reports what a shared screen is sent as, printed below.
    let trace = std::env::var("WORKING_TRACE").is_ok_and(|value| value == "1");
    let mut scripts = Vec::new();
    if trace {
      scripts.push("window.__workingTrace = true;");
    }
    // The page asks the app for a shared screen where the app can pick and capture one (the macOS picker, from
    // macOS 14); elsewhere it keeps the webview's own capture.
    #[cfg(target_os = "macos")]
    if screen::available() {
      scripts.push("window.__workingShare = true;");
    }
    scripts.push(include_str!("call.js"));
    // English labels: the script finds Meet's buttons by them.
    let separator = if url.contains('?') { '&' } else { '?' };
    let target = format!("{url}{separator}hl=en");
    webview::build(
      move |builder| {
        #[cfg(not(target_os = "windows"))]
        let builder = builder.with_user_agent(USER_AGENT);
        builder
          .with_autoplay(true)
          .with_background_color((0x13, 0x13, 0x14, 255))
          .with_ipc_handler(move |request| {
            let _ = report_tx.send(request.into_body());
          })
      },
      scripts,
      None,
      window,
      cx,
      move |this, web, _, cx| this.web_ready(web, target, cx),
    );

    let reports = cx.spawn_in(window, async move |this, cx| {
      while let Ok(report) = report_rx.recv_async().await {
        if let Some(stats) = report.strip_prefix("stats|") {
          tracing::debug!("call stats: {stats}");
          continue;
        }
        #[cfg(target_os = "macos")]
        {
          if let Some(capture) = report.strip_prefix("capture|") {
            this.update(cx, |this, cx| this.on_capture(capture, cx)).ok();
            continue;
          }
        }
        if let Some(chat) = report.strip_prefix("chat|") {
          if let Some(chat) = Chat::parse(chat) {
            cx.update(|window, cx| this.update(cx, |this, cx| this.on_chat(chat, window, cx)))
              .ok();
          }
          continue;
        }
        if let Some(sent) = report.strip_prefix("sent|") {
          cx.update(|window, cx| this.update(cx, |this, cx| this.on_sent(sent == "1", window, cx)))
            .ok();
          continue;
        }
        let Some(state) = CallState::parse(&report) else {
          continue;
        };
        cx.update(|window, cx| this.update(cx, |this, cx| this.on_report(state, window, cx)))
          .ok();
      }
    });

    // Google rotates the session's cookies while a call is open; the saved ones stop working soon after, for
    // the chat side and for the next call (each call starts from the saved ones alone). So the call's current
    // ones are saved, a little after joining and then regularly.
    let session_sync = cx.spawn(async move |this, cx| {
      let mut wait = std::time::Duration::from_secs(10);
      loop {
        cx.background_executor().timer(wait).await;
        wait = std::time::Duration::from_secs(30);
        let in_call = |this: &mut Self, cx: &mut Context<Self>| {
          let web = this.web.as_ref().filter(|_| this.state.phase == Phase::Call)?;
          Some(session::read(web.read(cx).raw()))
        };
        let Ok(read) = this.update(cx, in_call) else {
          break;
        };
        let Some(read) = read else { continue };
        let Ok(cookies) = read.recv_async().await else {
          continue;
        };
        if let Err(error) = gchat_core::save_session_cookies(cookies) {
          tracing::warn!("could not save the session's cookies: {error}");
        }
      }
    });

    let code = url
      .split(['?', '#'])
      .next()
      .unwrap_or_default()
      .rsplit('/')
      .next()
      .unwrap_or_default()
      .to_string();
    let state = CallState {
      phase: Phase::Lobby,
      mic: None,
      camera: None,
      hand: false,
      custom: true,
      presenting: false,
      voice: 0,
    };
    let draft = cx.new(|cx| {
      TextareaState::new(window, cx)
        .auto_grow(1, 5)
        .submit_on_enter(true)
        .placeholder("Send a message")
    });
    let draft_events = cx.subscribe_in(&draft, window, |this, _, event: &InputEvent, _, cx| match event {
      InputEvent::Change if this.send_failed => {
        this.send_failed = false;
        cx.notify();
      },
      InputEvent::PressEnter { shift: false, .. } => this.send_chat(cx),
      _ => {},
    });
    Self {
      web: None,
      url: url.to_string(),
      code: code.into(),
      copied: false,
      _copied_reset: None,
      state,
      chat: Chat::default(),
      chat_shown: false,
      chat_seen: 0,
      chat_scroll: ScrollHandle::new(),
      draft,
      sending: None,
      send_failed: false,
      preview: None,
      _preview_reset: None,
      notified: Vec::new(),
      #[cfg(target_os = "macos")]
      share: None,
      _reports: reports,
      _session_sync: session_sync,
      _settings: cx.observe_global::<Settings>(|this, cx| {
        if cx.global::<Settings>().camera_off {
          this.run("setCamera(false)", cx);
        }
        cx.notify();
      }),
      _draft: draft_events,
    }
  }

  /// The chat as the page has it now. While the panel shows, every message counts as seen and the newest one is
  /// scrolled to. A message from someone else that the user is not looking at is announced.
  fn on_chat(&mut self, chat: Chat, window: &Window, cx: &mut Context<Self>) {
    let known = self.chat.messages.len();
    self.chat = chat;
    let count = self.chat.messages.len();
    self.chat_seen = self.chat_seen.min(count);
    self.preview = self.preview.filter(|index| *index < count);
    let watched = self.chat_shown && window.is_window_active();
    if let Some(index) = self.chat.newest_unread(known).filter(|_| !watched) {
      self.announce(index, window, cx);
    }
    if self.chat_shown {
      self.see_chat();
    }
    cx.notify();
  }

  /// Tells the user about the message at `index`: beside the controls for a moment while the window is the
  /// active one, as a notification otherwise, unless do not disturb is on.
  fn announce(&mut self, index: usize, window: &Window, cx: &mut Context<Self>) {
    if window.is_window_active() {
      self.preview = Some(index);
      self._preview_reset = Some(cx.spawn(async move |this, cx| {
        cx.background_executor().timer(std::time::Duration::from_secs(6)).await;
        this
          .update(cx, |this, cx| {
            this.preview = None;
            cx.notify();
          })
          .ok();
      }));
      return;
    }
    if crate::status::quiet(cx) {
      return;
    }
    let message = &self.chat.messages[index];
    let thread = format!("{CHAT_KEY}{}", self.code);
    let id = format!("{thread}/{index}");
    let sound = cx.global::<Settings>().sound.resolved();
    crate::notify::post(&id, &thread, &message.sender, Some("Meeting"), &message.text, sound);
    self.notified.push(id);
  }

  /// The panel shows the chat up to its newest message: nothing is left to announce.
  fn see_chat(&mut self) {
    self.chat_seen = self.chat.messages.len();
    self.chat_scroll.scroll_to_bottom();
    self.preview = None;
    if !self.notified.is_empty() {
      crate::notify::clear(&self.notified);
      self.notified.clear();
    }
  }

  fn toggle_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.chat_shown.toggle();
    if self.chat_shown {
      self.see_chat();
      self.draft.update(cx, |state, cx| state.focus(window, cx));
    }
    cx.notify();
  }

  /// Sends what the field holds through Meet's own chat field. The text stays in the field until the page
  /// says it went (`on_sent`).
  fn send_chat(&mut self, cx: &mut Context<Self>) {
    if self.sending.is_some() {
      return;
    }
    let text = self.draft.read(cx).value().trim().to_string();
    if text.is_empty() {
      return;
    }
    // A JSON string is a JavaScript string literal.
    let Ok(literal) = serde_json::to_string(&text) else {
      return;
    };
    self.run(&format!("sendChat({literal})"), cx);
    self.sending = Some(text);
    self.send_failed = false;
    cx.notify();
  }

  /// The page's answer to `send_chat`: the message went, or Meet did not take it.
  fn on_sent(&mut self, sent: bool, window: &mut Window, cx: &mut Context<Self>) {
    let Some(text) = self.sending.take() else {
      return;
    };
    if !sent {
      self.send_failed = true;
    } else if self.draft.read(cx).value().trim() == text {
      self.draft.update(cx, |state, cx| state.set_value("", window, cx));
    }
    cx.notify();
  }

  /// The text of a message, with its links to click.
  fn chat_text(index: usize, message: &ChatMessage) -> AnyElement {
    if message.links.is_empty() {
      return message.text.clone().into_any_element();
    }
    let style = HighlightStyle {
      color: Some(rgb(ACCENT).into()),
      underline: Some(UnderlineStyle {
        thickness: px(1.),
        color: None,
        wavy: false,
      }),
      ..Default::default()
    };
    let (ranges, addresses): (Vec<_>, Vec<_>) = message.links.iter().cloned().unzip();
    let styled =
      StyledText::new(message.text.clone()).with_highlights(ranges.iter().map(|range| (range.clone(), style)));
    InteractiveText::new(("chat-text", index), styled)
      .on_click(ranges, move |link, _, cx| {
        if let Some(address) = addresses.get(link) {
          cx.open_url(address);
        }
      })
      .into_any_element()
  }

  /// The chat panel beside the tiles, `width` wide: the messages, then the field, or a line saying that nothing
  /// can be sent.
  fn chat_panel(&self, width: f32, cx: &mut Context<Self>) -> Div {
    let mut list = v_flex().w_full().px_3().pb_3().gap_1();
    // Messages in a row from one person at one time share a line with the name and the time, as in Meet.
    let mut head = None;
    for (index, message) in self.chat.messages.iter().enumerate() {
      if head != Some((&message.sender, &message.time)) {
        head = Some((&message.sender, &message.time));
        let sender = if message.sender.is_empty() {
          "You".into()
        } else {
          message.sender.clone()
        };
        list = list.child(
          h_flex()
            .pt_2()
            .gap_2()
            .items_baseline()
            .child(
              div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(TEXT))
                .child(sender),
            )
            .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(message.time.clone())),
        );
      }
      list = list.child(
        div()
          .w_full()
          .text_size(px(13.))
          .text_color(rgb(TEXT))
          .child(Self::chat_text(index, message)),
      );
    }
    let messages = if self.chat.messages.is_empty() {
      div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.))
        .text_color(rgb(MUTED))
        .child("No messages yet")
        .into_any_element()
    } else {
      div()
        .id("chat-messages")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(&self.chat_scroll)
        .child(list)
        .into_any_element()
    };
    let note = |text: &'static str| div().text_size(px(12.)).text_color(rgb(MUTED)).child(text);
    let field = if self.chat.writable() {
      h_flex()
        .min_h(px(36.))
        .pl_3()
        .pr_1()
        .items_end()
        .rounded(px(18.))
        .bg(rgb(FIELD))
        .child(
          div()
            .flex_1()
            .min_w_0()
            .child(Textarea::new(&self.draft).appearance(false).bordered(false)),
        )
        .child(
          div()
            .id("chat-send")
            .size(px(28.))
            .mb(px(4.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .hover(|s| s.opacity(0.85))
            // Dimmed while a message is on its way: the next one waits for it.
            .opacity(if self.sending.is_some() { 0.4 } else { 1. })
            .on_click(cx.listener(|this, _, _, cx| this.send_chat(cx)))
            .child(icon(Icon::Send, 15., ACCENT)),
        )
    } else {
      h_flex()
        .h(px(36.))
        .justify_center()
        .child(note("Messages can't be sent in this meeting right now"))
    };
    v_flex()
      .w(px(width))
      .h_full()
      .flex_shrink_0()
      .border_l_1()
      .border_color(rgb(BORDER))
      .on_action(cx.listener(|this, _: &Escape, window, cx| {
        this.toggle_chat(window, cx);
        cx.stop_propagation();
      }))
      .child(
        h_flex()
          .h(px(40.))
          .flex_shrink_0()
          .pl_3()
          .pr_2()
          .items_center()
          .child(
            div()
              .flex_1()
              .text_size(px(13.))
              .font_weight(FontWeight::SEMIBOLD)
              .text_color(rgb(TEXT))
              .child("In-call messages"),
          )
          .child(
            div()
              .id("chat-close")
              .size(px(26.))
              .flex()
              .items_center()
              .justify_center()
              .rounded_full()
              .cursor_pointer()
              .hover(|s| s.bg(rgb(FIELD)))
              .on_click(cx.listener(|this, _, window, cx| this.toggle_chat(window, cx)))
              .child(icon(Icon::X, 14., MUTED)),
          ),
      )
      .child(messages)
      .child(
        v_flex()
          .flex_shrink_0()
          .p_3()
          .gap_1p5()
          .children(self.send_failed.then(|| note("Not sent. Press Enter to try again.")))
          .child(field),
      )
  }

  /// The webview is there: the saved session goes into it, and the meeting at `target` loads once it is in, so
  /// Meet opens signed in.
  fn web_ready(&mut self, web: wry::Result<WebView>, target: String, cx: &mut Context<Self>) {
    let web = web.unwrap_or_else(|error| {
      tracing::error!("failed to create the call webview: {error}");
      std::process::exit(1);
    });
    let session_ready = session::install(web.raw());
    self.web = Some(cx.new(|_| web));
    cx.spawn(async move |this, cx| {
      let _ = session_ready.recv_async().await;
      this.update(cx, |this, cx| this.with_web(|web| web.load_url(&target), cx)).ok();
    })
    .detach();
    cx.notify();
  }

  /// The page wants a screen to share (`pick|<request number>`), or its share is over (`stop`). What goes
  /// back to the page carries that number.
  #[cfg(target_os = "macos")]
  fn on_capture(&mut self, request: &str, cx: &mut Context<Self>) {
    self.share = None;
    let Some(id) = request.strip_prefix("pick|").and_then(|id| id.parse::<u32>().ok()) else {
      return;
    };
    // One frame of slack: the capture waits for the page instead of piling pictures up.
    let (frame_tx, frame_rx) = flume::bounded::<Frame>(1);
    let (end_tx, end_rx) = flume::unbounded();
    let share = Share::pick(frame_tx, end_tx);
    let frames = cx.spawn(async move |this, cx| {
      while let Ok(frame) = frame_rx.recv_async().await {
        let sent = this.update(cx, |this, cx| this.run(&format!("frame({id},'{}')", frame.jpeg_base64), cx));
        if sent.is_err() {
          return;
        }
      }
    });
    let ended = cx.spawn(async move |this, cx| {
      let Ok(end) = end_rx.recv_async().await else {
        return;
      };
      let call = match end {
        End::Cancelled => format!("shareCancelled({id})"),
        End::Stopped => format!("shareStopped({id})"),
      };
      this
        .update(cx, |this, cx| {
          this.run(&call, cx);
          this.share = None;
        })
        .ok();
    });
    self.share = Some((share, frames, ended));
  }

  fn copy_link(&mut self, cx: &mut Context<Self>) {
    cx.write_to_clipboard(ClipboardItem::new_string(self.url.clone()));
    self.copied = true;
    self._copied_reset = Some(cx.spawn(async move |this, cx| {
      cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
      this
        .update(cx, |this, cx| {
          this.copied = false;
          cx.notify();
        })
        .ok();
    }));
    cx.notify();
  }

  fn on_report(&mut self, state: CallState, window: &mut Window, cx: &mut Context<Self>) {
    if state.phase == Phase::Ended {
      window.remove_window();
      return;
    }
    self.state = state;
    cx.notify();
  }

  /// Calls one of the functions `call.js` exposes as `window.__working`.
  fn run(&self, call: &str, cx: &App) {
    self.with_web(
      |web| web.evaluate_script(&format!("window.__working && window.__working.{call};")),
      cx,
    );
  }

  /// Calls `f` on the webview, once it is there.
  fn with_web(&self, f: impl FnOnce(&wry::WebView) -> wry::Result<()>, cx: &App) {
    if let Some(web) = &self.web {
      f(web.read(cx).raw()).ok();
    }
  }

  fn control(id: &'static str, name: Icon, background: Tone) -> Stateful<Div> {
    Self::tinted(id, name, background, TEXT)
  }

  fn tinted(id: &'static str, name: Icon, background: Tone, foreground: Tone) -> Stateful<Div> {
    div()
      .id(id)
      .size(px(44.))
      .flex()
      .items_center()
      .justify_center()
      .rounded_full()
      .bg(rgb(background))
      .cursor_pointer()
      .hover(|s| s.opacity(0.85))
      .child(icon(name, 20., foreground))
  }

  /// A microphone or camera button: neutral while on, red-tinted while off, dimmed when there is no device.
  fn device(id: &'static str, on: Option<bool>, on_icon: Icon, off_icon: Icon) -> Stateful<Div> {
    match on {
      Some(true) => Self::control(id, on_icon, FIELD),
      Some(false) => Self::control(id, off_icon, OFF_BG),
      None => Self::control(id, off_icon, FIELD).opacity(0.5),
    }
  }

  /// The microphone button. While the microphone is on it carries a small voice meter, which shows that sound
  /// is going out; it stands in for the self view Meet floats over the call.
  fn mic(state: CallState) -> Stateful<Div> {
    let button = Self::device("mic", state.mic, Icon::Mic, Icon::MicOff);
    if state.mic != Some(true) {
      return button;
    }
    let bar = |reach: f32| {
      div()
        .w(px(3.))
        .h(px((reach * f32::from(state.voice)).mul_add(3., 4.)))
        .rounded_full()
        .bg(rgb(if state.voice > 0 { GREEN } else { MUTED }))
    };
    button.w(px(76.)).gap_2().child(
      h_flex()
        .h(px(18.))
        .gap(px(2.))
        .items_center()
        .child(bar(0.6))
        .child(bar(1.))
        .child(bar(0.6)),
    )
  }
}

impl Render for CallView {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let width = window.viewport_size().width.as_f32();
    let state = self.state;
    // "Camera always off": no camera button anywhere, so the page never gets the real camera.
    let camera_off = cx.global::<Settings>().camera_off;
    let custom = state.custom;

    let bar = h_flex()
      .w_full()
      .flex_shrink_0()
      // On macOS clear of the traffic lights.
      .pl(px(chrome::inset(84.)))
      .pr_3()
      .gap_2()
      .items_center()
      .child(icon(Icon::Video, 16., ACCENT))
      .child(
        div()
          .text_size(px(14.))
          .font_weight(FontWeight::SEMIBOLD)
          .text_color(rgb(TEXT))
          .child("Meeting"),
      )
      .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(self.code.clone()))
      // A fallback for when the stripped view breaks or hides something: Meet's own web interface.
      .child(
        div()
          .id("toggle-interface")
          .occlude()
          .px_2()
          .rounded(px(6.))
          .bg(rgb(if custom { FIELD } else { ACCENT_BG }))
          .text_size(px(11.))
          .text_color(rgb(if custom { MUTED } else { TEXT }))
          .cursor_pointer()
          .hover(|s| s.opacity(0.85))
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .on_click(cx.listener(move |this, _, _, cx| {
            this.run(if custom { "setCustom(false)" } else { "setCustom(true)" }, cx);
          }))
          .child("WebUI"),
      )
      .child(div().flex_1())
      .child(
        h_flex()
          .id("copy-link")
          .occlude()
          .gap_1p5()
          .px_3()
          .py_1()
          .items_center()
          .rounded_full()
          .bg(rgb(FIELD))
          .text_size(px(12.))
          .text_color(rgb(TEXT))
          .cursor_pointer()
          .hover(|s| s.opacity(0.85))
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .on_click(cx.listener(|this, _, _, cx| this.copy_link(cx)))
          .child(icon(Icon::Copy, 13., TEXT))
          .child(if self.copied { "Copied" } else { "Copy link" }),
      )
      // The web interface hides our controls, but its camera button only works once this switch is on.
      .children((!custom && !camera_off).then(|| {
        let on = state.camera == Some(true);
        h_flex()
          .id("camera-switch")
          .occlude()
          .gap_1p5()
          .px_3()
          .py_1()
          .items_center()
          .rounded_full()
          .bg(rgb(if on { FIELD } else { OFF_BG }))
          .text_size(px(12.))
          .text_color(rgb(TEXT))
          .cursor_pointer()
          .hover(|s| s.opacity(0.85))
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .on_click(cx.listener(move |this, _, _, cx| {
            this.run(if on { "setCamera(false)" } else { "setCamera(true)" }, cx);
          }))
          .child(icon(if on { Icon::Video } else { Icon::VideoOff }, 13., TEXT))
          .child(if on { "Camera on" } else { "Camera off" })
      }));

    let chat_shown = self.chat_shown;
    // Messages from the others that came while the panel was closed.
    let unread = if chat_shown {
      0
    } else {
      self.chat.unread(self.chat_seen)
    };
    // The newest of them shows for a moment in the room the bar has left of its buttons.
    let room = (width - CONTROLS_WIDTH) / 2. - 24.;
    let preview = self
      .preview
      .and_then(|index| self.chat.messages.get(index))
      .filter(|_| !chat_shown && room >= PREVIEW_MIN_WIDTH)
      .map(|message| {
        let line = message.text.lines().next().unwrap_or_default().to_string();
        div().absolute().left_3().top_0().bottom_0().flex().items_center().child(
          h_flex()
            .id("chat-preview")
            .max_w(px(room))
            .px_3()
            .py_1p5()
            .gap_1p5()
            .rounded_full()
            .bg(rgb(FIELD))
            .text_size(px(12.))
            .text_color(rgb(TEXT))
            .cursor_pointer()
            .hover(|s| s.opacity(0.85))
            .on_click(cx.listener(|this, _, window, cx| this.toggle_chat(window, cx)))
            .child(
              div()
                .flex_shrink_0()
                .font_weight(FontWeight::SEMIBOLD)
                .child(message.sender.clone()),
            )
            .child(div().min_w_0().truncate().child(line)),
        )
      });
    // Google's interface has its own controls, and the lobby passes by itself.
    let controls = (custom && state.phase == Phase::Call).then(|| {
      h_flex()
        .h(px(CONTROLS_HEIGHT))
        .relative()
        .children(preview)
        .w_full()
        .flex_shrink_0()
        .gap_3()
        .items_center()
        .justify_center()
        .child(Self::mic(state).on_click(cx.listener(|this, _, _, cx| this.run("toggleMic()", cx))))
        .children((!camera_off).then(|| {
          Self::device("camera", state.camera, Icon::Video, Icon::VideoOff).on_click(cx.listener(
            move |this, _, _, cx| {
              this.run(
                if state.camera == Some(true) {
                  "setCamera(false)"
                } else {
                  "setCamera(true)"
                },
                cx,
              );
            },
          ))
        }))
        .child(
          Self::control("share", Icon::ScreenShare, if state.presenting { ACCENT_BG } else { FIELD })
            .on_click(cx.listener(|this, _, _, cx| this.run("share()", cx))),
        )
        .child(
          Self::control("hand", Icon::Hand, if state.hand { ACCENT_BG } else { FIELD })
            .on_click(cx.listener(|this, _, _, cx| this.run("toggleHand()", cx))),
        )
        .child(
          Self::control("chat", Icon::MessageSquare, if chat_shown { ACCENT_BG } else { FIELD })
            .relative()
            .children((unread > 0).then(|| {
              div()
                .absolute()
                .top(px(-3.))
                .right(px(-3.))
                .min_w(px(18.))
                .h(px(18.))
                .px_1()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(rgb(ACCENT))
                .text_size(px(11.))
                .text_color(rgb(BG))
                .child(unread.to_string())
            }))
            .on_click(cx.listener(|this, _, window, cx| this.toggle_chat(window, cx))),
        )
        .child(
          Self::tinted("leave", Icon::PhoneOff, HANGUP, WHITE)
            .w(px(64.))
            .on_click(cx.listener(|this, _, _, cx| this.run("leave()", cx))),
        )
    });

    // The chat belongs to the stripped view: Google's interface has its own panel.
    let chat = (custom && state.phase == Phase::Call && chat_shown).then(|| {
      let panel = (width * CHAT_SHARE).clamp(*CHAT_WIDTH.start(), *CHAT_WIDTH.end());
      self.chat_panel(panel, cx)
    });
    v_flex()
      .size_full()
      .bg(rgb(BG))
      .font_family(theme::sans())
      .child(chrome::top_bar(bar, TOP_BAR_HEIGHT))
      .child(
        h_flex()
          .flex_1()
          .min_h_0()
          .w_full()
          .child(div().flex_1().min_w_0().h_full().children(self.web.clone()))
          .children(chat),
      )
      .children(controls)
  }
}

/// Opens the meeting at `url`. Built-in Meet uses a call window (the browser if that window cannot be made);
/// otherwise the link opens in the browser like any other.
pub fn open(url: &str, cx: &mut App) {
  if !cx.global::<Settings>().meet_builtin {
    cx.open_url(url);
    return;
  }
  let url = url.to_string();
  let fallback = url.clone();
  let mut options = WindowOptions {
    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1100.), px(720.)), cx))),
    window_min_size: Some(size(px(640.), px(420.))),
    ..Default::default()
  };
  chrome::set_title_bar(&mut options, "Meeting", TOP_BAR_HEIGHT);
  match open_window(options, cx, move |window, cx| cx.new(|cx| CallView::new(&url, window, cx))) {
    Ok((window, view)) => {
      let calls = cx.default_global::<Calls>();
      calls.0.retain(|(_, call)| call.upgrade().is_some());
      calls.0.push((window, view.downgrade()));
    },
    Err(_) => cx.open_url(&fallback),
  }
}

/// A notification was clicked: when `key` is the one of a call's chat, that call's window comes forward with its
/// chat panel showing. Says whether the key was a call's.
pub fn show_chat(key: &str, cx: &mut App) -> bool {
  let Some(code) = key.strip_prefix(CHAT_KEY) else {
    return false;
  };
  let call = cx.try_global::<Calls>().and_then(|calls| {
    calls
      .0
      .iter()
      .filter_map(|(window, view)| Some((*window, view.upgrade()?)))
      .find(|(_, view)| view.read(cx).code.as_ref() == code)
  });
  if let Some((window, view)) = call {
    window
      .update(cx, |_, window, cx| {
        cx.activate(true);
        window.activate_window();
        view.update(cx, |this, cx| {
          if !this.chat_shown {
            this.toggle_chat(window, cx);
          }
        });
      })
      .ok();
  }
  true
}

#[cfg(test)]
mod tests {
  use super::{CallState, Phase};

  #[test]
  fn parse_report() {
    let state = CallState::parse("call|1|0|1|1|0|3").expect("shape");
    assert_eq!(state.phase, Phase::Call);
    assert_eq!(state.mic, Some(true));
    assert_eq!(state.camera, Some(false));
    assert!(state.hand);
    assert!(state.custom);
    assert!(!state.presenting);
    assert_eq!(state.voice, 3);
    assert!(CallState::parse("call|1|0").is_none());
    assert!(CallState::parse("nope|1|0|0|0|0|0").is_none());
    let lobby = CallState::parse("lobby|-|-|0|1|0|0").expect("none devices");
    assert_eq!(lobby.phase, Phase::Lobby);
    assert_eq!(lobby.mic, None);
    assert_eq!(lobby.camera, None);
  }
}
