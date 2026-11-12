//! New chat: pick people, open their DM, or create a group DM when that set does not exist.

use std::sync::Arc;

use gchat_core::fuzzy::Fuzzy;
use gchat_core::{Command, GroupId, User};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::store::Store;
use crate::theme::{ACCENT, BORDER, FIELD, MUTED, POPOVER, ROW_HOVER, ROW_SELECTED, TEXT, WHITE, rgb};
use crate::ui::{Icon, avatar, icon};

actions!(working, [OpenNewChat]);

const WIDTH: f32 = 420.0;
const HEIGHT: f32 = 520.0;
const ROW: f32 = 44.0;
const CHIP: f32 = 28.0;

pub enum NewChatEvent {
  Close,
  Open(GroupId),
}

pub struct NewChat {
  store: Entity<Store>,
  input: Entity<InputState>,
  selected: Vec<User>,
  results: Vec<Arc<User>>,
  selected_row: usize,
  _sub: Subscription,
}

impl EventEmitter<NewChatEvent> for NewChat {}

impl NewChat {
  pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("Add 1 or more people"));
    let sub = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
      InputEvent::Change => this.refresh(cx),
      InputEvent::PressEnter { .. } => this.on_enter(window, cx),
      _ => {},
    });
    input.update(cx, |state, cx| state.focus(window, cx));
    let mut view = Self {
      store,
      input,
      selected: Vec::new(),
      results: Vec::new(),
      selected_row: 0,
      _sub: sub,
    };
    view.refresh(cx);
    view
  }

  fn query(&self, cx: &App) -> String {
    self.input.read(cx).value().trim().to_string()
  }

  fn refresh(&mut self, cx: &mut Context<Self>) {
    let store = self.store.read(cx);
    let picked: std::collections::HashSet<&str> = self.selected.iter().map(|user| user.id.as_str()).collect();
    let people: Vec<Arc<User>> = store
      .people
      .iter()
      .filter(|user| !picked.contains(user.id.as_str()))
      .cloned()
      .map(Arc::new)
      .collect();
    let query = self.query(cx);
    self.results = if query.is_empty() {
      let dms: std::collections::HashSet<&str> = store
        .all
        .iter()
        .filter(|conv| conv.g.kind == gchat_core::GroupKind::Dm)
        .flat_map(|conv| conv.g.member_ids.iter().map(String::as_str))
        .collect();
      let mut people = people;
      people.sort_by_key(|user| (!dms.contains(user.id.as_str()), user.name.to_lowercase()));
      people
    } else {
      let mut fuzzy = Fuzzy::new(&query);
      let mut hits: Vec<(u32, Arc<User>)> = people
        .into_iter()
        .filter_map(|user| {
          let score = fuzzy.score(&user.name).into_iter().chain(fuzzy.score(&user.email)).max();
          score.map(|score| (score, user))
        })
        .collect();
      hits.sort_by(|a, b| b.0.cmp(&a.0));
      hits.into_iter().map(|(_, user)| user).collect()
    };
    if self.selected_row >= self.results.len() {
      self.selected_row = self.results.len().saturating_sub(1);
    }
    cx.notify();
  }

  fn toggle(&mut self, user: User, cx: &mut Context<Self>) {
    if let Some(ix) = self.selected.iter().position(|held| held.id == user.id) {
      self.selected.remove(ix);
    } else {
      self.selected.push(user);
    }
    self.refresh(cx);
  }

  fn add_highlighted(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    let Some(user) = self.results.get(self.selected_row).map(|user| (**user).clone()) else {
      return false;
    };
    if !self.selected.iter().any(|held| held.id == user.id) {
      self.selected.push(user);
    }
    self.input.update(cx, |state, cx| state.set_value("", window, cx));
    self.selected_row = 0;
    self.refresh(cx);
    true
  }

  fn on_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if !self.query(cx).is_empty() {
      self.add_highlighted(window, cx);
      return;
    }
    if !self.selected.is_empty() {
      self.start(cx);
      return;
    }
    self.add_highlighted(window, cx);
  }

  fn start(&mut self, cx: &mut Context<Self>) {
    if self.selected.is_empty() {
      return;
    }
    let ids: Vec<String> = self.selected.iter().map(|user| user.id.clone()).collect();
    let store = self.store.read(cx);
    if let Some(id) = store.existing_chat(&ids) {
      cx.emit(NewChatEvent::Open(id));
      return;
    }
    store.core.send(Command::CreateChat { user_ids: ids });
    cx.emit(NewChatEvent::Close);
  }

  fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
    match event.keystroke.key.as_str() {
      "escape" => cx.emit(NewChatEvent::Close),
      "up" => {
        if self.selected_row > 0 {
          self.selected_row -= 1;
          cx.notify();
        }
      },
      "down" => {
        if self.selected_row + 1 < self.results.len() {
          self.selected_row += 1;
          cx.notify();
        }
      },
      "backspace" if self.query(cx).is_empty() => {
        self.selected.pop();
        self.refresh(cx);
      },
      _ => return,
    }
    cx.stop_propagation();
    let _ = window;
  }
}

impl Render for NewChat {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let images = self.store.read(cx).images.clone();
    let can_start = !self.selected.is_empty();
    // Enter on an empty field starts the chat once someone is picked, so don't tint a row as if it would be added.
    let arm_row = self.query(cx).is_empty() && can_start;

    let mut chips = h_flex().gap_1().flex_wrap().min_h(px(CHIP)).w_full();
    for (ix, user) in self.selected.iter().enumerate() {
      let id = user.id.clone();
      chips = chips.child(
        h_flex()
          .id(("chip", ix))
          .h(px(CHIP))
          .pl_1()
          .pr_2()
          .gap_1()
          .items_center()
          .rounded_full()
          .bg(rgb(FIELD))
          .child(avatar(&images, cx, user.avatar_url.as_deref(), None, &user.name, 20.))
          .child(div().text_size(px(12.)).child(user.name.clone()))
          .child(
            div()
              .id(("chip-x", ix))
              .cursor_pointer()
              .on_click(cx.listener(move |this, _, _, cx| {
                this.selected.retain(|user| user.id != id);
                this.refresh(cx);
              }))
              .child(icon(Icon::X, 12., MUTED)),
          ),
      );
    }

    let mut list = v_flex().p_1();
    if self.results.is_empty() {
      list = list.child(
        div()
          .h(px(ROW))
          .px_3()
          .flex()
          .items_center()
          .text_color(rgb(MUTED))
          .child("No people match"),
      );
    }
    for (ix, user) in self.results.iter().enumerate() {
      let picked = !arm_row && ix == self.selected_row;
      let person = (**user).clone();
      let direct = person.clone();
      list = list.child(
        h_flex()
          .id(("person", ix))
          .h(px(ROW))
          .px_3()
          .gap_3()
          .items_center()
          .rounded(px(8.))
          .cursor_pointer()
          .when(picked, |d| d.bg(rgb(ROW_SELECTED)))
          .when(!picked, |d| d.hover(|s| s.bg(rgb(ROW_HOVER))))
          .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
            this.selected_row = ix;
            if event.click_count() >= 2 {
              this.selected.clear();
              this.selected.push(direct.clone());
              this.start(cx);
            } else {
              this.toggle(person.clone(), cx);
            }
          }))
          .child(avatar(&images, cx, user.avatar_url.as_deref(), None, &user.name, 28.))
          .child(
            v_flex()
              .min_w_0()
              .child(
                div()
                  .text_size(px(13.))
                  .text_color(rgb(TEXT))
                  .truncate()
                  .child(user.name.clone()),
              )
              .when(!user.email.is_empty(), |d| {
                d.child(
                  div()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .truncate()
                    .child(user.email.clone()),
                )
              }),
          ),
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
      .pt(px(80.))
      .bg(rgba(0x00000073))
      .occlude()
      .on_action(cx.listener(|_, _: &Escape, _, cx| {
        cx.emit(NewChatEvent::Close);
        cx.stop_propagation();
      }))
      .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(NewChatEvent::Close)))
      .child(
        v_flex()
          .w(px(WIDTH))
          .h(px(HEIGHT))
          .rounded(px(14.))
          .bg(rgb(POPOVER))
          .border_1()
          .border_color(rgb(BORDER))
          .overflow_hidden()
          .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| this.on_key(event, window, cx)))
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .child(
            v_flex()
              .px_3()
              .py_2()
              .gap_2()
              .border_b_1()
              .border_color(rgb(BORDER))
              .child(chips)
              .child(
                div()
                  .h(px(32.))
                  .flex()
                  .items_center()
                  .child(Input::new(&self.input).appearance(false).bordered(false)),
              ),
          )
          .child(div().id("people-list").flex_1().min_h_0().overflow_y_scroll().child(list))
          .child(
            h_flex()
              .h(px(48.))
              .px_3()
              .justify_end()
              .items_center()
              .border_t_1()
              .border_color(rgb(BORDER))
              .child(
                div()
                  .id("start-chat")
                  .h(px(30.))
                  .px_3()
                  .flex()
                  .items_center()
                  .rounded(px(8.))
                  .text_size(px(13.))
                  .when(can_start, |d| {
                    d.bg(rgb(ACCENT))
                      .text_color(rgb(WHITE))
                      .cursor_pointer()
                      .on_click(cx.listener(|this, _, _, cx| this.start(cx)))
                  })
                  .when(!can_start, |d| d.bg(rgb(FIELD)).text_color(rgb(MUTED)))
                  .child("Start chat"),
              ),
          ),
      )
  }
}
