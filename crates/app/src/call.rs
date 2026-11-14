//! A Google Meet call in a window of its own.
//!
//! Audio and video are Google's: the Meet web app runs in a webview, signed in with the chat session. What the
//! user sees is ours: `call.js` strips the page down to the video tiles and joins by itself, with the camera
//! kept away from the page until the camera button here turns it on,
//! and this window draws the top bar and the controls, which click Meet's own buttons through that script. A
//! button in the top bar brings Google's interface back, for anything the stripped view hides.
//!
//! Closing the window drops the webview, which leaves the call.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;
use gpui_webview::WebView;
use wry::WebViewBuilder;

use crate::screen::{self, End, Frame, Share};
use crate::session;
use crate::settings::Settings;
use crate::theme::{self, ACCENT, ACCENT_BG, BG, FIELD, GREEN, HANGUP, MUTED, OFF_BG, TEXT, Tone, WHITE, rgb};

use crate::ui::{Icon, icon};

/// Meet supports Safari; the default `WKWebView` agent has no browser name and gets the "unsupported" page.
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15";

const TOP_BAR_HEIGHT: f32 = 44.0;
const CONTROLS_HEIGHT: f32 = 68.0;

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
  web: Entity<WebView>,
  /// The meeting link as shared in the chat.
  url: String,
  code: SharedString,
  /// The link was just copied: the copy button says so for a moment.
  copied: bool,
  _copied_reset: Option<Task<()>>,
  state: CallState,
  /// The screen share, from the macOS picker to its end, with the tasks that pass its frames and its end to
  /// the page.
  share: Option<(Share, Task<()>, Task<()>)>,
  _reports: Task<()>,
  _session_sync: Task<()>,
  /// Turns the camera off when "Camera always off" is switched on during a call.
  _settings: Subscription,
}

impl CallView {
  fn new(url: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let (report_tx, report_rx) = flume::unbounded::<String>();
    // `WORKING_TRACE=1`: the page reports what a shared screen is sent as, printed below.
    let trace = std::env::var("WORKING_TRACE").is_ok_and(|value| value == "1");
    let builder = WebViewBuilder::new()
      .with_user_agent(USER_AGENT)
      .with_autoplay(true)
      .with_background_color((0x13, 0x13, 0x14, 255))
      .with_initialization_script(if trace {
        "window.__workingTrace = true;"
      } else {
        ""
      })
      // The page asks the app for a shared screen where the app can pick and capture one.
      .with_initialization_script(if screen::available() {
        "window.__workingShare = true;"
      } else {
        ""
      })
      .with_initialization_script(include_str!("call.js"))
      .with_ipc_handler(move |request| {
        let _ = report_tx.send(request.into_body());
      });
    let web = WebView::build(builder, window, cx).unwrap_or_else(|error| {
      tracing::error!("failed to create the call webview: {error}");
      std::process::exit(1);
    });
    let session_ready = session::install(web.raw());
    // English labels: the script finds Meet's buttons by them.
    let separator = if url.contains('?') { '&' } else { '?' };
    let target = format!("{url}{separator}hl=en");
    let web = cx.new(|_| web);

    let reports = cx.spawn_in(window, async move |this, cx| {
      // The first page load waits for the session, so Meet opens signed in.
      let _ = session_ready.recv_async().await;
      cx.update(|_, cx| this.update(cx, |this, cx| this.web.read(cx).raw().load_url(&target).ok()))
        .ok();
      while let Ok(report) = report_rx.recv_async().await {
        if let Some(stats) = report.strip_prefix("stats|") {
          tracing::debug!("call stats: {stats}");
          continue;
        }
        if let Some(capture) = report.strip_prefix("capture|") {
          this.update(cx, |this, cx| this.on_capture(capture, cx)).ok();
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
    // the chat side and for the next call (the webview keeps none between launches). So the call's current
    // ones are saved, a little after joining and then regularly.
    let session_sync = cx.spawn(async move |this, cx| {
      let mut wait = std::time::Duration::from_secs(10);
      loop {
        cx.background_executor().timer(wait).await;
        wait = std::time::Duration::from_secs(30);
        let in_call = |this: &mut Self, cx: &mut Context<Self>| {
          (this.state.phase == Phase::Call).then(|| session::read(this.web.read(cx).raw()))
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
    Self {
      web,
      url: url.to_string(),
      code: code.into(),
      copied: false,
      _copied_reset: None,
      state,
      share: None,
      _reports: reports,
      _session_sync: session_sync,
      _settings: cx.observe_global::<Settings>(|this, cx| {
        if cx.global::<Settings>().camera_off {
          this.run("setCamera(false)", cx);
        }
        cx.notify();
      }),
    }
  }

  /// The page wants a screen to share (`pick|<request number>`), or its share is over (`stop`). What goes
  /// back to the page carries that number.
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
    self
      .web
      .read(cx)
      .raw()
      .evaluate_script(&format!("window.__working && window.__working.{call};"))
      .ok();
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
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let state = self.state;
    // "Camera always off": no camera button anywhere, so the page never gets the real camera.
    let camera_off = cx.global::<Settings>().camera_off;
    let custom = state.custom;

    let bar = h_flex()
      .h(px(TOP_BAR_HEIGHT))
      .w_full()
      .flex_shrink_0()
      // Clear of the traffic lights.
      .pl(px(84.))
      .pr_3()
      .gap_2()
      .items_center()
      .on_mouse_down(MouseButton::Left, |event, window, _| {
        if event.click_count == 2 {
          window.titlebar_double_click();
        } else {
          window.start_window_move();
        }
      })
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

    // Google's interface has its own controls, and the lobby passes by itself.
    let controls = (custom && state.phase == Phase::Call).then(|| {
      h_flex()
        .h(px(CONTROLS_HEIGHT))
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
          Self::tinted("leave", Icon::PhoneOff, HANGUP, WHITE)
            .w(px(64.))
            .on_click(cx.listener(|this, _, _, cx| this.run("leave()", cx))),
        )
    });

    v_flex()
      .size_full()
      .bg(rgb(BG))
      .font_family(theme::sans())
      .child(bar)
      .child(div().flex_1().min_h_0().w_full().child(self.web.clone()))
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
  let options = WindowOptions {
    titlebar: Some(TitlebarOptions {
      title: Some("Meeting".into()),
      appears_transparent: true,
      // The three 12px buttons, centered vertically in the top bar.
      traffic_light_position: Some(point(px(16.), px((TOP_BAR_HEIGHT - 12.) / 2.))),
    }),
    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1100.), px(720.)), cx))),
    window_min_size: Some(size(px(640.), px(420.))),
    ..Default::default()
  };
  let opened = cx.open_window(options, move |window, cx| cx.new(|cx| CallView::new(&url, window, cx)));
  if opened.is_err() {
    cx.open_url(&fallback);
  }
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
