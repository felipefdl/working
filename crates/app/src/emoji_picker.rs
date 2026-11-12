//! The full emoji picker: every emoji in a searchable grid, opened at a point of the window.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;

use crate::theme::{BORDER, FIELD, MUTED, POPOVER, ROW_HOVER, TEXT, rgb};

const COLUMNS: usize = 8;
const CELL: f32 = 36.0;
const WIDTH: f32 = COLUMNS as f32 * CELL + 16.0;
const HEIGHT: f32 = 340.0;

/// What to do with the emoji the user picked.
pub type Pick = Rc<dyn Fn(&str, &mut Window, &mut App)>;

struct Shown {
  at: Point<Pixels>,
  pick: Pick,
  previous_focus: Option<FocusHandle>,
}

pub struct EmojiPicker {
  shown: Option<Shown>,
  input: Entity<InputState>,
  scroll: UniformListScrollHandle,
  /// The emoji matching the search text, in Unicode's order.
  results: Vec<&'static emojis::Emoji>,
  _sub: Subscription,
}

/// The window's emoji picker, reachable from any click handler.
pub struct EmojiHandle(pub Entity<EmojiPicker>);

impl Global for EmojiHandle {}

/// Opens the picker at `at` (window coordinates); `pick` gets the chosen emoji.
pub fn show(at: Point<Pixels>, pick: Pick, window: &mut Window, cx: &mut App) {
  let picker = cx.global::<EmojiHandle>().0.clone();
  picker.update(cx, |picker, cx| picker.open(at, pick, window, cx));
}

/// The newest Unicode version whose emoji the system font is known to draw; later ones show as a box.
const NEWEST_UNICODE: u32 = 16;

fn drawable() -> impl Iterator<Item = &'static emojis::Emoji> {
  emojis::iter().filter(|emoji| emoji.unicode_version().major() <= NEWEST_UNICODE)
}

impl EmojiPicker {
  pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search emoji"));
    let sub = cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
      if matches!(event, InputEvent::Change) {
        let query = input.read(cx).value().trim().to_lowercase();
        this.search(&query);
        cx.notify();
      }
    });
    Self {
      shown: None,
      input,
      scroll: UniformListScrollHandle::new(),
      results: drawable().collect(),
      _sub: sub,
    }
  }

  pub const fn is_open(&self) -> bool {
    self.shown.is_some()
  }

  fn search(&mut self, query: &str) {
    self.results = drawable()
      .filter(|emoji| {
        query.is_empty() || emoji.name().contains(query) || emoji.shortcodes().any(|code| code.contains(query))
      })
      .collect();
  }

  fn open(&mut self, at: Point<Pixels>, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
    let previous_focus = match self.shown.take() {
      Some(shown) => shown.previous_focus,
      None => window.focused(cx),
    };
    self.shown = Some(Shown { at, pick, previous_focus });
    self.input.update(cx, |state, cx| {
      state.set_value("", window, cx);
      state.focus(window, cx);
    });
    self.search("");
    cx.notify();
  }

  pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(shown) = self.shown.take() else {
      return;
    };
    if let Some(handle) = shown.previous_focus {
      window.focus(&handle, cx);
    }
    cx.notify();
  }

  fn pick(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
    let Some(emoji) = self.results.get(ix).copied() else {
      return;
    };
    let Some(pick) = self.shown.as_ref().map(|shown| shown.pick.clone()) else {
      return;
    };
    self.close(window, cx);
    pick(emoji.as_str(), window, cx);
  }

  fn rows(&self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
    range
      .map(|row| {
        let mut line = h_flex().h(px(CELL));
        for ix in row * COLUMNS..((row + 1) * COLUMNS).min(self.results.len()) {
          line = line.child(
            div()
              .id(("emoji", ix))
              .size(px(CELL))
              .flex()
              .items_center()
              .justify_center()
              .rounded(px(6.))
              .text_size(px(20.))
              .cursor_pointer()
              .hover(|s| s.bg(rgb(ROW_HOVER)))
              .on_click(cx.listener(move |this, _, window, cx| this.pick(ix, window, cx)))
              .child(self.results[ix].as_str()),
          );
        }
        line.into_any_element()
      })
      .collect()
  }
}

impl Render for EmojiPicker {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let Some(shown) = &self.shown else {
      return div().into_any_element();
    };
    let window_size = window.viewport_size();
    let left = shown.at.x.min(window_size.width - px(WIDTH + 8.)).max(px(8.));
    let top = shown.at.y.min(window_size.height - px(HEIGHT + 8.)).max(px(8.));
    let rows = self.results.len().div_ceil(COLUMNS);

    let grid: AnyElement = if rows == 0 {
      div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(MUTED))
        .child("No emoji found")
        .into_any_element()
    } else {
      div()
        .flex_1()
        .min_h_0()
        .child(
          uniform_list(
            "emoji-rows",
            rows,
            cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
          )
          .track_scroll(&self.scroll)
          .size_full(),
        )
        .into_any_element()
    };

    div()
      .absolute()
      .top_0()
      .left_0()
      .size_full()
      .occlude()
      .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close(window, cx)))
      .on_mouse_down(MouseButton::Right, cx.listener(|this, _, window, cx| this.close(window, cx)))
      .child(
        v_flex()
          .absolute()
          .left(left)
          .top(top)
          .w(px(WIDTH))
          .h(px(HEIGHT))
          .p_2()
          .gap_2()
          .rounded(px(12.))
          .bg(rgb(POPOVER))
          .border_1()
          .border_color(rgb(BORDER))
          .shadow_lg()
          .text_color(rgb(TEXT))
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
          .child(
            div()
              .h(px(34.))
              .flex_shrink_0()
              .px_3()
              .flex()
              .items_center()
              .rounded_full()
              .bg(rgb(FIELD))
              .child(Input::new(&self.input).appearance(false).bordered(false)),
          )
          .child(grid),
      )
      .into_any_element()
  }
}
