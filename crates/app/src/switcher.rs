//! Cmd+K quick switcher: fzf-style search over conversation names, recent first when empty.

use std::sync::Arc;

use gchat_core::GroupId;
use gchat_core::fuzzy::Fuzzy;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::store::{Conv, Store};
use crate::theme::{BORDER, MUTED, POPOVER, ROW_HOVER, ROW_SELECTED, TEXT, rgb};
use crate::ui::group_avatar;

const MAX_RESULTS: usize = 12;

pub enum SwitcherEvent {
  Close,
  Pick(GroupId),
}

pub struct Switcher {
  store: Entity<Store>,
  input: Entity<InputState>,
  results: Vec<Arc<Conv>>,
  selected: usize,
  _sub: Subscription,
}

impl EventEmitter<SwitcherEvent> for Switcher {}

impl Switcher {
  pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("Jump to a conversation"));
    let sub = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
      if matches!(event, InputEvent::Change) {
        this.refresh(cx);
      }
    });
    input.update(cx, |state, cx| state.focus(window, cx));
    let mut this = Self {
      store,
      input,
      results: Vec::new(),
      selected: 0,
      _sub: sub,
    };
    this.refresh(cx);
    this
  }

  fn refresh(&mut self, cx: &mut Context<Self>) {
    let query = self.input.read(cx).value().trim().to_string();
    let store = self.store.read(cx);
    let mut results: Vec<Arc<Conv>> = Vec::new();
    if query.is_empty() {
      for id in &store.recent {
        if let Some(conv) = store.group(id) {
          results.push(conv.clone());
        }
      }
      for conv in &store.all {
        if results.len() >= MAX_RESULTS {
          break;
        }
        if !results.iter().any(|r| r.g.id == conv.g.id) {
          results.push(conv.clone());
        }
      }
    } else {
      // `all` is by recency, so equally good matches come most recent first.
      let ranked = Fuzzy::new(&query).rank(store.all.iter(), |conv| conv.g.name.as_str());
      results.extend(ranked.into_iter().map(|(_, conv)| conv.clone()));
    }
    results.truncate(MAX_RESULTS);
    self.results = results;
    self.selected = 0;
    cx.notify();
  }

  fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
    if let Some(conv) = self.results.get(ix) {
      cx.emit(SwitcherEvent::Pick(conv.g.id.clone()));
    }
  }

  fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
    match event.keystroke.key.as_str() {
      "escape" => cx.emit(SwitcherEvent::Close),
      "down" => self.selected = (self.selected + 1).min(self.results.len().saturating_sub(1)),
      "up" => self.selected = self.selected.saturating_sub(1),
      "enter" => self.pick(self.selected, cx),
      _ => return,
    }
    cx.stop_propagation();
    cx.notify();
  }
}

impl Render for Switcher {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let images = self.store.read(cx).images.clone();
    let mut list = v_flex().p_1();
    for (ix, conv) in self.results.iter().enumerate() {
      let selected = ix == self.selected;
      list = list.child(
        h_flex()
          .id(("sw", ix))
          .h(px(40.))
          .px_3()
          .gap_3()
          .items_center()
          .rounded(px(8.))
          .cursor_pointer()
          .when(selected, |d| d.bg(rgb(ROW_SELECTED)))
          .when(!selected, |d| d.hover(|s| s.bg(rgb(ROW_HOVER))))
          .on_click(cx.listener(move |this, _, _, cx| this.pick(ix, cx)))
          .child(group_avatar(&images, cx, &conv.g, 24.))
          .child(
            div()
              .min_w_0()
              .truncate()
              .text_size(px(14.))
              .text_color(rgb(TEXT))
              .when(conv.g.unread, |d| d.font_weight(FontWeight::SEMIBOLD))
              .child(conv.g.name.clone()),
          ),
      );
    }
    if self.results.is_empty() {
      list = list.child(
        div()
          .h(px(40.))
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
      .bg(rgba(0x00000073))
      .occlude()
      .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(SwitcherEvent::Close)))
      .child(
        v_flex()
          .w(px(560.))
          .rounded(px(14.))
          .bg(rgb(POPOVER))
          .border_1()
          .border_color(rgb(BORDER))
          .overflow_hidden()
          .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_key(event, cx)))
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .child(
            div()
              .px_4()
              .h(px(48.))
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
