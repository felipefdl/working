//! The dialog that changes where the MCP server listens: its address and its port, saved together so the server
//! restarts once. Settings, Agents opens it from the Server card's Address row and draws it over itself.

use std::net::IpAddr;

use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::component::{FocusTrapElement as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::settings::Settings;
use crate::theme::{BORDER, MUTED, POPOVER, RED, ROW_HOVER, TEXT, control_fill, rgb};
use crate::ui::{Icon, icon, plain_button, primary_button};

const WIDTH: f32 = 400.0;

pub enum ListenEvent {
  Close,
}

pub struct ListenDialog {
  address: Entity<InputState>,
  port: Entity<InputState>,
  focus: FocusHandle,
  _subs: [Subscription; 2],
}

impl EventEmitter<ListenEvent> for ListenDialog {}

/// The address in `text`, when it is an IP address.
fn address(text: &str) -> Option<IpAddr> {
  text.trim().parse().ok()
}

/// The lowest port the dialog takes: those below are system ports, which need administrator rights on Linux.
const MIN_PORT: u16 = 1024;

/// The port in `text`, when it is one from [`MIN_PORT`] to 65535.
fn port(text: &str) -> Option<u16> {
  text.trim().parse().ok().filter(|port| *port >= MIN_PORT)
}

impl ListenDialog {
  /// Opens on the address and port in use, with the keyboard in the address.
  pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
    let settings = cx.global::<Settings>();
    let shown = [settings.mcp_address.to_string(), settings.mcp_port.to_string()];
    let [address, port] = shown.map(|value| {
      let field = cx.new(|cx| InputState::new(window, cx));
      field.update(cx, |state, cx| state.set_value(value, window, cx));
      field
    });
    // Each keystroke shows whether the text is a value; Return saves.
    let subs = [&address, &port].map(|field| {
      cx.subscribe_in(field, window, |this, _, event: &InputEvent, _, cx| match event {
        InputEvent::Change => cx.notify(),
        InputEvent::PressEnter { .. } => this.save(cx),
        InputEvent::Focus | InputEvent::Blur => {},
      })
    });
    address.update(cx, |state, cx| state.focus(window, cx));
    Self {
      address,
      port,
      focus: cx.focus_handle(),
      _subs: subs,
    }
  }

  /// Both values, while both fields hold one.
  fn values(&self, cx: &App) -> (Option<IpAddr>, Option<u16>) {
    (address(&self.address.read(cx).value()), port(&self.port.read(cx).value()))
  }

  /// Writes both settings in one change and closes. Refused while either field holds text that is not a value.
  fn save(&mut self, cx: &mut Context<Self>) {
    let (Some(address), Some(port)) = self.values(cx) else {
      return;
    };
    Settings::update(cx, |settings| {
      settings.mcp_address = address;
      settings.mcp_port = port;
    });
    cx.emit(ListenEvent::Close);
  }

  /// A labeled field at `width`, with `hint` under it, or `invalid` in red while it holds text that is not a value.
  fn field(
    label: &'static str,
    state: &Entity<InputState>,
    width: f32,
    valid: bool,
    hint: &'static str,
    invalid: &'static str,
  ) -> Div {
    v_flex()
      .gap_1()
      .child(div().text_size(px(13.)).text_color(rgb(TEXT)).child(label))
      .child(
        div()
          .w(px(width))
          .px_2()
          .py_1()
          .rounded(px(9.))
          .bg(control_fill())
          .child(Input::new(state).appearance(false).bordered(false)),
      )
      .child(
        div()
          .text_size(px(12.))
          .text_color(rgb(if valid { MUTED } else { RED }))
          .child(if valid { hint } else { invalid }),
      )
  }
}

impl Render for ListenDialog {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let (address, port) = self.values(cx);
    let valid = address.is_some() && port.is_some();
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
      .key_context("ListenDialog")
      .on_action(cx.listener(|_, _: &Escape, _, cx| {
        cx.emit(ListenEvent::Close);
        cx.stop_propagation();
      }))
      // A press outside cancels, and stays out of the settings under it, which would close on it.
      .on_mouse_down(
        MouseButton::Left,
        cx.listener(|_, _, _, cx| {
          cx.emit(ListenEvent::Close);
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
                  .child("Server address"),
              )
              .child(
                div()
                  .id("listen-close")
                  .size(px(30.))
                  .flex()
                  .items_center()
                  .justify_center()
                  .rounded_full()
                  .cursor_pointer()
                  .hover(|style| style.bg(rgb(ROW_HOVER)))
                  .on_click(cx.listener(|_, _, _, cx| cx.emit(ListenEvent::Close)))
                  .child(icon(Icon::X, 16., TEXT)),
              ),
          )
          .child(
            v_flex()
              .px_4()
              .pb_4()
              .gap_3()
              .child(Self::field(
                "Address",
                &self.address,
                220.,
                address.is_some(),
                "127.0.0.1 is this computer only; 0.0.0.0 is every network this computer is on.",
                "Not an IP address, such as 127.0.0.1.",
              ))
              .child(Self::field(
                "Port",
                &self.port,
                100.,
                port.is_some(),
                "A number from 1024 to 65535.",
                "Not a port: a number from 1024 to 65535.",
              )),
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
              .child(plain_button("listen-cancel", "Cancel").on_click(cx.listener(|_, _, _, cx| cx.emit(ListenEvent::Close))))
              .child(
                primary_button("listen-save", "Save", valid)
                  .when(valid, |save| save.on_click(cx.listener(|this, _, _, cx| this.save(cx)))),
              ),
          ),
      )
      .focus_trap("listen-dialog", &self.focus)
  }
}

#[cfg(test)]
mod tests {
  use super::{address, port};

  #[test]
  fn mcp_address_takes_an_ip_address() {
    assert_eq!(address(" 0.0.0.0 "), Some([0, 0, 0, 0].into()));
    assert!(address("::1").is_some());
    assert_eq!(address("localhost"), None);
    assert_eq!(address("127.0.0"), None);
  }

  #[test]
  fn mcp_port_is_1024_to_65535() {
    assert_eq!(port("4000"), Some(4000));
    assert_eq!(port("1024"), Some(1024));
    assert_eq!(port("65535"), Some(65535));
    assert_eq!(port("1023"), None);
    assert_eq!(port("0"), None);
    assert_eq!(port("65536"), None);
    assert_eq!(port("9a"), None);
  }
}
