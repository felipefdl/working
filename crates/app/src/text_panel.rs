//! A message's raw text in a small floating panel at the pointer, where it can be selected and copied. A click on a
//! message's text bubble opens it (`message.rs`). Message text in the conversation is drawn with `StyledText`, which
//! cannot be selected: this stands in until the kit's selectable text can carry links.

use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;

use crate::calendar::{pill, round_button};
use crate::conv_list::hint;
use crate::store::{Store, View};
use crate::theme::{BORDER, MUTED, POPOVER, TEXT, rgb};
use crate::ui::{Icon, icon};

const WIDTH: f32 = 380.0;
/// The tallest the panel gets before its text scrolls.
const MAX_HEIGHT: f32 = 320.0;
/// Rows of text before the field scrolls: the header and the field's padding fit in `MAX_HEIGHT` with them.
const MAX_ROWS: usize = 11;
const HEADER_HEIGHT: f32 = 36.0;
/// Gap kept between the panel and the window's edges.
const MARGIN: f32 = 12.0;
/// How far the panel sits from the pointer, so the pointer is not over it.
const OFFSET: f32 = 8.0;

struct Shown {
  at: Point<Pixels>,
  text: SharedString,
  /// What had the keyboard before the panel took it.
  previous_focus: Option<FocusHandle>,
  /// The view and conversation on screen when it opened: leaving them closes the panel.
  view: View,
  conv: Option<EntityId>,
}

pub struct TextPanel {
  store: Entity<Store>,
  field: Entity<TextareaState>,
  shown: Option<Shown>,
  focus: FocusHandle,
  _store: Subscription,
}

/// The window's text panel, reachable from any click handler.
pub struct TextPanelHandle(pub Entity<TextPanel>);

impl Global for TextPanelHandle {}

/// Opens the text panel at `at` (window coordinates) holding `text`, or moves it there when it is already open.
pub fn show(at: Point<Pixels>, text: &str, window: &mut Window, cx: &mut App) {
  let panel = cx.global::<TextPanelHandle>().0.clone();
  panel.update(cx, |panel, cx| panel.open(at, text, window, cx));
}

impl TextPanel {
  pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let field = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, MAX_ROWS));
    // Another conversation or view on screen closes the panel.
    let observed = cx.observe_in(&store, window, |this, _, window, cx| this.close_if_left(window, cx));
    Self {
      store,
      field,
      shown: None,
      focus: cx.focus_handle(),
      _store: observed,
    }
  }

  pub const fn is_open(&self) -> bool {
    self.shown.is_some()
  }

  fn open(&mut self, at: Point<Pixels>, text: &str, window: &mut Window, cx: &mut Context<Self>) {
    let previous_focus = match self.shown.take() {
      Some(shown) => shown.previous_focus,
      None => window.focused(cx),
    };
    let store = self.store.read(cx);
    self.shown = Some(Shown {
      at,
      text: text.to_owned().into(),
      previous_focus,
      view: store.view.clone(),
      conv: store.conv.as_ref().map(Entity::entity_id),
    });
    self.field.update(cx, |field, cx| {
      field.set_value(text.to_owned(), window, cx);
      field.focus(window, cx);
    });
    // Again once the click is over: a mouse press moves focus to what was pressed after this runs.
    let field = self.field.clone();
    window.defer(cx, move |window, cx| field.update(cx, |field, cx| field.focus(window, cx)));
    cx.notify();
  }

  /// Closes the panel and gives the keyboard back, unless it has already moved on (a conversation that opened took it).
  pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(shown) = self.shown.take() else {
      return;
    };
    if self.focus.contains_focused(window, cx)
      && let Some(handle) = shown.previous_focus
    {
      window.focus(&handle, cx);
    }
    cx.notify();
  }

  fn close_if_left(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(shown) = &self.shown else {
      return;
    };
    let store = self.store.read(cx);
    if store.view != shown.view || store.conv.as_ref().map(Entity::entity_id) != shown.conv {
      self.close(window, cx);
    }
  }

  fn header(&self, cx: &Context<Self>) -> Div {
    h_flex()
      .h(px(HEADER_HEIGHT))
      .flex_shrink_0()
      .pl_3()
      .pr_1()
      .gap_1()
      .items_center()
      .child(div().flex_1().text_size(px(12.)).text_color(rgb(MUTED)).child("Message text"))
      .child(
        pill("text-panel-copy")
          .h(px(26.))
          .px_2()
          .gap_1()
          .text_size(px(12.))
          .child(icon(Icon::Copy, 14., TEXT))
          .child("Copy all")
          .on_click(cx.listener(|this, _, window, cx| {
            if let Some(shown) = &this.shown {
              cx.write_to_clipboard(ClipboardItem::new_string(shown.text.to_string()));
            }
            this.close(window, cx);
          })),
      )
      .child(
        round_button("text-panel-close", Icon::X)
          .size(px(26.))
          .tooltip(hint("Close"))
          .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
      )
  }
}

impl Render for TextPanel {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let Some(shown) = &self.shown else {
      return div().into_any_element();
    };
    let max_height = px(MAX_HEIGHT).min((window.viewport_size().height - px(2. * MARGIN)).max(px(120.)));
    // Beside the pointer; once its height is known, the panel is moved to stay inside the window.
    let at = point(shown.at.x + px(OFFSET), shown.at.y + px(OFFSET));

    // No backdrop: a press anywhere else closes the panel and still reaches what is there, so clicking another bubble
    // closes this panel on the press and opens it on that message on the release, and clicking the same bubble again
    // moves it to the new pointer.
    div()
      .absolute()
      .top_0()
      .left_0()
      .size_full()
      .child(
        anchored().position(at).snap_to_window_with_margin(px(MARGIN)).child(
          v_flex()
            .w(px(WIDTH))
            .max_h(max_height)
            .rounded(px(14.))
            .bg(rgb(POPOVER))
            .border_1()
            .border_color(rgb(BORDER))
            .shadow_lg()
            .overflow_hidden()
            .occlude()
            .text_color(rgb(TEXT))
            .track_focus(&self.focus)
            .key_context("TextPanel")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
              if event.keystroke.key == "escape" {
                this.close(window, cx);
                cx.stop_propagation();
              }
            }))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close(window, cx)))
            .child(self.header(cx))
            .child(
              div()
                .px_3()
                .pb_3()
                .min_h_0()
                .child(Textarea::new(&self.field).appearance(false).bordered(false).readonly(true)),
            ),
        ),
      )
      .into_any_element()
  }
}
