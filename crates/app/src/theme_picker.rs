//! Color theme picker: the dark and the light themes in a filterable list, as VS Code's. The highlighted theme shows
//! on every window until Enter keeps it for its appearance or Escape puts the saved one back.

use gchat_core::fuzzy::Fuzzy;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::settings::{self, Settings};
use crate::theme::{self, BORDER, MUTED, POPOVER, ROW_HOVER, ROW_SELECTED, TEXT, THEMES, rgb};

pub enum ThemePickerEvent {
  Close,
  /// An index in `THEMES`.
  Pick(usize),
}

pub struct ThemePicker {
  input: Entity<InputState>,
  /// Indexes in `THEMES`: the themes of the appearance on screen first, then the others.
  results: Vec<usize>,
  selected: usize,
  scroll: ScrollHandle,
  /// The saved theme of each appearance, marked in the list.
  saved: [usize; 2],
  _sub: Subscription,
}

impl EventEmitter<ThemePickerEvent> for ThemePicker {}

/// Themes whose name matches `query`, those with `dark_first`'s appearance first.
fn ordered(query: &str, dark_first: bool) -> Vec<usize> {
  let matching: Vec<usize> = if query.is_empty() {
    (0..THEMES.len()).collect()
  } else {
    Fuzzy::new(query)
      .rank(0..THEMES.len(), |ix| THEMES[*ix].name)
      .into_iter()
      .map(|(_, ix)| ix)
      .collect()
  };
  let (mut first, second): (Vec<usize>, Vec<usize>) =
    matching.into_iter().partition(|ix| THEMES[*ix].dark == dark_first);
  first.extend(second);
  first
}

impl ThemePicker {
  pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("Select a color theme"));
    let sub = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
      if matches!(event, InputEvent::Change) {
        this.refresh(cx);
      }
    });
    input.update(cx, |state, cx| state.focus(window, cx));
    let settings = cx.global::<Settings>();
    let saved = [
      theme::find(&settings.dark_theme, true),
      theme::find(&settings.light_theme, false),
    ];
    let shown = settings.shown_theme(cx);
    let results = ordered("", THEMES[shown].dark);
    let this = Self {
      selected: results.iter().position(|ix| *ix == shown).unwrap_or_default(),
      scroll: ScrollHandle::new(),
      input,
      results,
      saved,
      _sub: sub,
    };
    this.reveal();
    this
  }

  /// Scrolls the highlighted row into view. The list's children are the rows plus a heading before each appearance.
  fn reveal(&self) {
    let (Some(first), Some(row)) = (self.results.first(), self.results.get(self.selected)) else {
      return;
    };
    let headings = 1 + usize::from(THEMES[*row].dark != THEMES[*first].dark);
    self.scroll.scroll_to_item(self.selected + headings);
  }

  fn refresh(&mut self, cx: &mut Context<Self>) {
    let query = self.input.read(cx).value().trim().to_string();
    self.results = ordered(&query, !theme::is_light());
    self.select(0, cx);
  }

  /// Highlights a row and shows its theme.
  fn select(&mut self, row: usize, cx: &mut Context<Self>) {
    self.selected = row;
    self.reveal();
    if let Some(ix) = self.results.get(row) {
      settings::show_theme(*ix, cx);
    }
    cx.notify();
  }

  fn pick(&mut self, row: usize, cx: &mut Context<Self>) {
    if let Some(ix) = self.results.get(row) {
      cx.emit(ThemePickerEvent::Pick(*ix));
    }
  }

  fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
    match event.keystroke.key.as_str() {
      "escape" => cx.emit(ThemePickerEvent::Close),
      "down" => self.select((self.selected + 1).min(self.results.len().saturating_sub(1)), cx),
      "up" => self.select(self.selected.saturating_sub(1), cx),
      "enter" => self.pick(self.selected, cx),
      _ => return,
    }
    cx.stop_propagation();
  }
}

impl Render for ThemePicker {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let mut list = div()
      .id("themes")
      .flex()
      .flex_col()
      .flex_1()
      .min_h_0()
      .p_1()
      .overflow_y_scroll()
      .track_scroll(&self.scroll);
    let mut section = None;
    for (row, ix) in self.results.iter().copied().enumerate() {
      let item = &THEMES[ix];
      if section != Some(item.dark) {
        section = Some(item.dark);
        list = list.child(
          div()
            .px_3()
            .pt_2()
            .pb_1()
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(MUTED))
            .child(if item.dark { "Dark" } else { "Light" }),
        );
      }
      let selected = row == self.selected;
      list = list.child(
        h_flex()
          .id(("theme", ix))
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
          .child(div().text_size(px(14.)).text_color(rgb(TEXT)).child(item.name))
          .when(self.saved.contains(&ix), |row| {
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
      .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(ThemePickerEvent::Close)))
      .child(
        v_flex()
          .w(px(420.))
          .max_h(px(560.))
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
  use super::ordered;
  use crate::theme::THEMES;

  #[test]
  fn the_shown_appearance_comes_first() {
    for dark_first in [true, false] {
      let rows = ordered("", dark_first);
      assert_eq!(rows.len(), THEMES.len());
      let switch = rows.iter().position(|ix| THEMES[*ix].dark != dark_first).unwrap();
      assert!(rows[switch..].iter().all(|ix| THEMES[*ix].dark != dark_first));
    }
  }

  #[test]
  fn a_query_keeps_matching_names_in_appearance_order() {
    let names: Vec<&str> = ordered("dracula", false).into_iter().map(|ix| THEMES[ix].name).collect();
    assert_eq!(names, ["Dracula Light (Alucard)", "Dracula"]);
  }
}
