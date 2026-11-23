//! The dialog that adds the MCP server to an agent: pick the harness, read what to do with its setup, and copy it.
//! Settings, Agents opens it from the Server card's "Add to an agent" row and draws it over itself.

use std::net::SocketAddr;
use std::time::Duration;

use gpui_kit::component::IndexPath;
use gpui_kit::component::input::Escape;
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::{FocusTrapElement as _, h_flex, v_flex};
use gpui_kit::*;

use crate::theme::{BORDER, MUTED, POPOVER, ROW_HOVER, TEXT, control_fill, mono, rgb};
use crate::ui::{Icon, icon, plain_button, primary_button};

const WIDTH: f32 = 560.0;

/// An agent harness the server can be added to, in the order the select lists them. Each setup is user-wide: the
/// server reaches every project.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Harness {
  ClaudeCode,
  Codex,
  Grok,
  Pi,
  Omp,
  Hermes,
  OpenClaw,
  Antigravity,
}

impl Harness {
  const ALL: [Self; 8] = [
    Self::ClaudeCode,
    Self::Codex,
    Self::Grok,
    Self::Pi,
    Self::Omp,
    Self::Hermes,
    Self::OpenClaw,
    Self::Antigravity,
  ];

  const fn label(self) -> &'static str {
    match self {
      Self::ClaudeCode => "Claude Code",
      Self::Codex => "Codex",
      Self::Grok => "Grok",
      Self::Pi => "Pi",
      Self::Omp => "omp",
      Self::Hermes => "Hermes",
      Self::OpenClaw => "OpenClaw",
      Self::Antigravity => "Antigravity",
    }
  }

  /// What to do with the setup.
  const fn note(self) -> &'static str {
    match self {
      Self::ClaudeCode | Self::Codex | Self::Grok | Self::Pi | Self::OpenClaw | Self::Antigravity => {
        "Run it in a terminal. It adds the server for every project."
      },
      Self::Hermes => "Run it in a terminal. Hermes then asks for the token: paste it from Copy token.",
      Self::Omp => "Type it in omp. It adds the server for every project.",
    }
  }

  /// The command that adds the server at `reach`, taking `token`.
  fn setup(self, reach: SocketAddr, token: &str) -> String {
    let url = format!("http://{reach}/mcp");
    match self {
      Self::ClaudeCode => {
        format!("claude mcp add --scope user --transport http working {url} --header \"Authorization: Bearer {token}\"")
      },
      // Neither `codex mcp add` nor `agy mcp add` takes a header: the token goes in the address.
      Self::Codex => format!("codex mcp add working --url {url}/{token}"),
      Self::Grok => {
        format!("grok mcp add --scope user --transport http working {url} --header \"Authorization: Bearer {token}\"")
      },
      Self::Pi => format!("pi mcp add working --url {url} --header \"Authorization=Bearer {token}\""),
      Self::Omp => format!("/mcp add working --scope user --url {url} --transport http --token {token}"),
      Self::Hermes => format!("hermes mcp add working --url {url} --auth header"),
      Self::OpenClaw => format!(
        "openclaw mcp set working '{{\"url\":\"{url}\",\"transport\":\"streamable-http\",\"headers\":{{\"Authorization\":\"Bearer \
         {token}\"}}}}'"
      ),
      Self::Antigravity => format!("agy mcp add working {url}/{token}"),
    }
  }
}

impl SelectItem for Harness {
  type Value = Self;

  fn title(&self) -> SharedString {
    self.label().into()
  }

  fn value(&self) -> &Self {
    self
  }
}

pub enum AgentEvent {
  Close,
}

pub struct AgentDialog {
  /// Where agents reach the server (`Settings::mcp_reach`).
  reach: SocketAddr,
  harness: Harness,
  select: Entity<SelectState<Vec<Harness>>>,
  /// The setup was just copied: Copy says so for a moment.
  copied: bool,
  _copied_reset: Option<Task<()>>,
  focus: FocusHandle,
  _select_sub: Subscription,
}

impl EventEmitter<AgentEvent> for AgentDialog {}

impl AgentDialog {
  /// Opens on Claude Code, for the server at `reach`.
  pub fn new(reach: SocketAddr, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let select = cx.new(|cx| SelectState::new(Harness::ALL.to_vec(), Some(IndexPath::new(0)), window, cx));
    let select_sub = cx.subscribe(&select, |this: &mut Self, _, event: &SelectEvent<Vec<Harness>>, cx| {
      let SelectEvent::Confirm(Some(harness)) = event else {
        return;
      };
      this.harness = *harness;
      this.copied = false;
      this._copied_reset = None;
      cx.notify();
    });
    let focus = cx.focus_handle();
    window.focus(&focus, cx);
    Self {
      reach,
      harness: Harness::ClaudeCode,
      select,
      copied: false,
      _copied_reset: None,
      focus,
      _select_sub: select_sub,
    }
  }

  /// Copies the setup with the token in it.
  fn copy(&mut self, cx: &mut Context<Self>) {
    let token = match gchat_core::mcp::token() {
      Ok(token) => token,
      Err(error) => {
        tracing::warn!("could not read the MCP token: {error}");
        return;
      },
    };
    cx.write_to_clipboard(ClipboardItem::new_string(self.harness.setup(self.reach, &token)));
    self.copied = true;
    self._copied_reset = Some(cx.spawn(async move |this, cx| {
      cx.background_executor().timer(Duration::from_millis(1500)).await;
      this
        .update(cx, |this, cx| {
          this.copied = false;
          cx.notify();
        })
        .ok();
    }));
    cx.notify();
  }
}

impl Render for AgentDialog {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    div()
      .absolute()
      .top_0()
      .left_0()
      .size_full()
      .flex()
      .items_center()
      .justify_center()
      .bg(rgba(0x0000_0073))
      .occlude()
      .key_context("AgentDialog")
      .on_action(cx.listener(|_, _: &Escape, _, cx| {
        cx.emit(AgentEvent::Close);
        cx.stop_propagation();
      }))
      // A press outside closes, and stays out of the settings under it, which would close on it.
      .on_mouse_down(
        MouseButton::Left,
        cx.listener(|_, _, _, cx| {
          cx.emit(AgentEvent::Close);
          cx.stop_propagation();
        }),
      )
      .child(
        v_flex()
          .w(px(WIDTH))
          .rounded(px(14.))
          .bg(rgb(POPOVER))
          .border_1()
          .border_color(rgb(BORDER))
          .shadow_lg()
          .overflow_hidden()
          .text_color(rgb(TEXT))
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .child(
            h_flex()
              .h(px(48.))
              .pl_4()
              .pr_2()
              .flex_shrink_0()
              .items_center()
              .child(
                div()
                  .flex_1()
                  .text_size(px(13.))
                  .font_weight(FontWeight::MEDIUM)
                  .text_color(rgb(MUTED))
                  .child("Add to an agent"),
              )
              .child(
                div()
                  .id("agent-close")
                  .size(px(30.))
                  .flex()
                  .items_center()
                  .justify_center()
                  .rounded_full()
                  .cursor_pointer()
                  .hover(|style| style.bg(rgb(ROW_HOVER)))
                  .on_click(cx.listener(|_, _, _, cx| cx.emit(AgentEvent::Close)))
                  .child(icon(Icon::X, 16., TEXT)),
              ),
          )
          .child(
            v_flex()
              .px_4()
              .pb_4()
              .gap_3()
              .child(
                div()
                  .w(px(220.))
                  .child(Select::new(&self.select).id("agent-harness").w_full().menu_max_h(px(320.))),
              )
              .child(
                div()
                  .text_size(px(12.))
                  .text_color(rgb(MUTED))
                  .child(self.harness.note()),
              )
              .child(
                div()
                  .p_3()
                  .rounded(px(9.))
                  .bg(control_fill())
                  .font_family(mono())
                  .text_size(px(12.))
                  .child(self.harness.setup(self.reach, "<token>")),
              ),
          )
          .child(
            h_flex()
              .h(px(56.))
              .px_4()
              .gap_2()
              .flex_shrink_0()
              .items_center()
              .justify_end()
              .border_t_1()
              .border_color(rgb(BORDER))
              .child(plain_button("agent-dismiss", "Close").on_click(cx.listener(|_, _, _, cx| cx.emit(AgentEvent::Close))))
              .child(
                primary_button("agent-copy", if self.copied { "Copied" } else { "Copy" }, true)
                  .on_click(cx.listener(|this, _, _, cx| this.copy(cx))),
              ),
          ),
      )
      .focus_trap("agent-dialog", &self.focus)
  }
}

#[cfg(test)]
mod tests {
  use super::Harness;
  use std::net::SocketAddr;

  #[test]
  fn mcp_agent_setups_keep_each_harness_shape() {
    let reach: SocketAddr = "127.0.0.1:3021".parse().unwrap();
    assert_eq!(
      Harness::Codex.setup(reach, "wk_test"),
      "codex mcp add working --url http://127.0.0.1:3021/mcp/wk_test"
    );
    assert_eq!(
      Harness::Pi.setup(reach, "wk_test"),
      "pi mcp add working --url http://127.0.0.1:3021/mcp --header \"Authorization=Bearer wk_test\""
    );
    assert_eq!(
      Harness::Omp.setup(reach, "wk_test"),
      "/mcp add working --scope user --url http://127.0.0.1:3021/mcp --transport http --token wk_test"
    );
    assert_eq!(
      Harness::OpenClaw.setup(reach, "wk_test"),
      r#"openclaw mcp set working '{"url":"http://127.0.0.1:3021/mcp","transport":"streamable-http","headers":{"Authorization":"Bearer wk_test"}}'"#
    );
    assert_eq!(
      Harness::Antigravity.setup(reach, "wk_test"),
      "agy mcp add working http://127.0.0.1:3021/mcp/wk_test"
    );
  }
}
