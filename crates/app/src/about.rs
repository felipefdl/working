//! The About card: the app's mark, name, version, and where its source lives, over the main window.

use std::sync::Arc;

use gpui_kit::component::{FocusTrapElement as _, v_flex};
use gpui_kit::*;

use crate::theme::{ACCENT, BORDER, MUTED, POPOVER, TEXT, is_light, rgb};

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

pub enum AboutEvent {
  Close,
}

pub struct About {
  focus: FocusHandle,
  /// The app's mark, for the dark look and for the light one.
  logo: [Arc<Image>; 2],
}

impl EventEmitter<AboutEvent> for About {}

impl About {
  pub fn new(logo: [Arc<Image>; 2], window: &mut Window, cx: &mut Context<Self>) -> Self {
    let focus = cx.focus_handle();
    window.focus(&focus, cx);
    Self { focus, logo }
  }
}

impl Render for About {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    div()
      .absolute()
      .top_0()
      .left_0()
      .size_full()
      .flex()
      .justify_center()
      .items_start()
      .pt(px(90.))
      .occlude()
      .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(AboutEvent::Close)))
      .child(
        v_flex()
          .w(px(320.))
          .p_6()
          .gap_1()
          .items_center()
          .rounded(px(14.))
          .bg(rgb(POPOVER))
          .border_1()
          .border_color(rgb(BORDER))
          .shadow_lg()
          .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
            if matches!(event.keystroke.key.as_str(), "escape" | "enter") {
              cx.emit(AboutEvent::Close);
              cx.stop_propagation();
            }
          }))
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .child(img(self.logo[usize::from(is_light())].clone()).size(px(64.)).mb_2())
          .child(
            div()
              .text_size(px(18.))
              .font_weight(FontWeight::SEMIBOLD)
              .text_color(rgb(TEXT))
              .child("Working"),
          )
          .child(
            div()
              .text_size(px(12.))
              .text_color(rgb(MUTED))
              .child(concat!("Version ", env!("CARGO_PKG_VERSION"))),
          )
          .child(
            div()
              .mt_2()
              .text_size(px(13.))
              .text_center()
              .text_color(rgb(TEXT))
              .child(env!("CARGO_PKG_DESCRIPTION")),
          )
          .child(
            div()
              .id("about-repository")
              .mt_2()
              .text_size(px(13.))
              .text_color(rgb(ACCENT))
              .cursor_pointer()
              .on_click(|_, _, cx| cx.open_url(REPOSITORY))
              .child(REPOSITORY.trim_start_matches("https://")),
          )
          .focus_trap("about", &self.focus),
      )
  }
}
