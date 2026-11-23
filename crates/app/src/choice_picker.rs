//! Picker of one setting's choices (the window layout, the message style) in a filterable list, as the color theme
//! picker. The highlighted choice shows in the window until Enter keeps it or Escape puts the saved one back.

use gchat_core::fuzzy::Fuzzy;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::settings::{Layout, MessageStyle, Settings};
use crate::theme::{BORDER, MUTED, POPOVER, ROW_HOVER, ROW_SELECTED, TEXT, rgb};

/// A setting the picker lists: its values, and where the settings hold the one in use.
pub trait Choice: Copy + PartialEq + 'static {
  /// Every value, in the order the settings window shows them.
  const ALL: &'static [Self];
  const PLACEHOLDER: &'static str;

  fn label(self) -> &'static str;
  fn get(settings: &Settings) -> Self;
  fn set(self, settings: &mut Settings);
}

impl Choice for Layout {
  const ALL: &'static [Self] = &Self::ALL;
  const PLACEHOLDER: &'static str = "Select a layout";

  fn label(self) -> &'static str {
    Self::label(self)
  }

  fn get(settings: &Settings) -> Self {
    settings.layout
  }

  fn set(self, settings: &mut Settings) {
    settings.layout = self;
  }
}

impl Choice for MessageStyle {
  const ALL: &'static [Self] = &[Self::Comfortable, Self::Compact, Self::Irc];
  const PLACEHOLDER: &'static str = "Select a message style";

  fn label(self) -> &'static str {
    match self {
      Self::Comfortable => "Comfortable",
      Self::Compact => "Compact",
      Self::Irc => "IRC",
    }
  }

  fn get(settings: &Settings) -> Self {
    settings.message_style
  }

  fn set(self, settings: &mut Settings) {
    settings.message_style = self;
  }
}

pub enum ChoicePickerEvent<T> {
  Close,
  Pick(T),
}

pub struct ChoicePicker<T: Choice> {
  input: Entity<InputState>,
  results: Vec<T>,
  selected: usize,
  /// The value the settings had when the picker opened: marked in the list, and back on screen when it closes
  /// without a pick.
  pub saved: T,
  _sub: Subscription,
}

impl<T: Choice> EventEmitter<ChoicePickerEvent<T>> for ChoicePicker<T> {}

/// Values whose name matches `query`, best match first; every value in its own order without one.
fn matching<T: Choice>(query: &str) -> Vec<T> {
  if query.is_empty() {
    return T::ALL.to_vec();
  }
  Fuzzy::new(query)
    .rank(T::ALL.iter().copied(), |value| value.label())
    .into_iter()
    .map(|(_, value)| value)
    .collect()
}

impl<T: Choice> ChoicePicker<T> {
  pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder(T::PLACEHOLDER));
    let sub = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
      if matches!(event, InputEvent::Change) {
        this.refresh(cx);
      }
    });
    input.update(cx, |state, cx| state.focus(window, cx));
    let saved = T::get(cx.global::<Settings>());
    let results = matching::<T>("");
    Self {
      selected: results.iter().position(|value| *value == saved).unwrap_or_default(),
      input,
      results,
      saved,
      _sub: sub,
    }
  }

  fn refresh(&mut self, cx: &mut Context<Self>) {
    let query = self.input.read(cx).value().trim().to_string();
    self.results = matching(&query);
    self.select(0, cx);
  }

  /// Highlights a row and shows its value in the window, without saving it.
  fn select(&mut self, row: usize, cx: &mut Context<Self>) {
    self.selected = row;
    if let Some(value) = self.results.get(row).copied() {
      Settings::preview(cx, |settings| value.set(settings));
    }
    cx.notify();
  }

  fn pick(&mut self, row: usize, cx: &mut Context<Self>) {
    if let Some(value) = self.results.get(row) {
      cx.emit(ChoicePickerEvent::Pick(*value));
    }
  }

  fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
    match event.keystroke.key.as_str() {
      "escape" => cx.emit(ChoicePickerEvent::Close),
      "down" => self.select((self.selected + 1).min(self.results.len().saturating_sub(1)), cx),
      "up" => self.select(self.selected.saturating_sub(1), cx),
      "enter" => self.pick(self.selected, cx),
      _ => return,
    }
    cx.stop_propagation();
  }
}

impl<T: Choice> Render for ChoicePicker<T> {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let mut list = v_flex().p_1();
    for (row, value) in self.results.iter().copied().enumerate() {
      let selected = row == self.selected;
      list = list.child(
        h_flex()
          .id(("choice", row))
          .h(px(32.))
          .flex_shrink_0()
          .px_3()
          .gap_3()
          .items_center()
          .justify_between()
          .rounded(px(8.))
          .cursor_pointer()
          .when(selected, |d| d.bg(rgb(ROW_SELECTED)))
          .when(!selected, |d| d.hover(|s| s.bg(rgb(ROW_HOVER))))
          .on_click(cx.listener(move |this, _, _, cx| this.pick(row, cx)))
          .child(div().text_size(px(14.)).text_color(rgb(TEXT)).child(value.label()))
          .when(value == self.saved, |row| {
            row.child(div().text_size(px(12.)).text_color(rgb(MUTED)).child("Current"))
          }),
      );
    }
    if self.results.is_empty() {
      list = list.child(
        div()
          .h(px(32.))
          .px_3()
          .flex()
          .items_center()
          .text_color(rgb(MUTED))
          .child("No matches"),
      );
    }

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
      .on_mouse_down(
        MouseButton::Left,
        cx.listener(|_, _, _, cx| cx.emit(ChoicePickerEvent::<T>::Close)),
      )
      .child(
        v_flex()
          .w(px(420.))
          .rounded(px(14.))
          .bg(rgb(POPOVER))
          .border_1()
          .border_color(rgb(BORDER))
          .shadow_lg()
          .overflow_hidden()
          .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_key(event, cx)))
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .child(
            div()
              .px_4()
              .h(px(48.))
              .flex_shrink_0()
              .flex()
              .items_center()
              .border_b_1()
              .border_color(rgb(BORDER))
              .child(Input::new(&self.input).appearance(false).bordered(false)),
          )
          .child(list),
      )
  }
}

#[cfg(test)]
mod tests {
  use super::matching;
  use crate::settings::{Layout, MessageStyle};

  #[test]
  fn a_query_narrows_the_choices_by_name() {
    assert_eq!(matching::<Layout>(""), Layout::ALL);
    assert_eq!(matching::<Layout>("irc"), [Layout::Irc]);
    assert_eq!(matching::<MessageStyle>("compact"), [MessageStyle::Compact]);
  }
}
