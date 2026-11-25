//! Forward message: the dialog that picks the conversation a message goes to. The first entry of a message's more
//! menu opens it (`show`, from `message.rs`); Next hands the message and the conversation picked to `Store::forward`,
//! which opens that conversation with the forward's chip over its main composer. The window root draws the dialog.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use gchat_core::fuzzy::Fuzzy;
use gchat_core::{GroupId, GroupKind, Message};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::component::{FocusTrapElement as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::conversation::modal_size;
use crate::emoji_font;
use crate::mail_logic::step;
use crate::store::{Conv, Forwarding, Store};
use crate::theme::{ACCENT, BORDER, MUTED, POPOVER, ROW_HOVER, ROW_SELECTED, TEXT, control_fill, rgb};
use crate::ui::{Icon, group_avatar, icon, plain_button, primary_button};

/// A conversation's row: its avatar, its name, and for a one-to-one chat the person's email under it.
const ROW: f32 = 52.0;
/// The smallest and the largest the dialog gets as the window's size changes (`modal_size`).
const SIZE: (Size<f32>, Size<f32>) = (Size { width: 400.0, height: 360.0 }, Size { width: 460.0, height: 600.0 });

struct Shown {
  message: Arc<Message>,
  /// What had the keyboard before the dialog took it, given back when it closes without forwarding.
  previous_focus: Option<FocusHandle>,
}

/// A conversation the message can go to.
struct Target {
  conv: Arc<Conv>,
  /// The person's email, for a one-to-one chat with someone the directory knows.
  email: Option<SharedString>,
}

pub struct ForwardDialog {
  store: Entity<Store>,
  input: Entity<InputState>,
  /// Holds the panel's focus trap, so Tab stays in the dialog.
  focus: FocusHandle,
  scroll: UniformListScrollHandle,
  shown: Option<Shown>,
  /// The conversations the search leaves, in the sidebar's recency order.
  targets: Vec<Target>,
  /// The conversation the message goes to on Next: one of `targets`, else none.
  picked: Option<GroupId>,
  _sub: Subscription,
}

/// The window's Forward message dialog, reachable from any click handler.
pub struct ForwardHandle(pub Entity<ForwardDialog>);

impl Global for ForwardHandle {}

/// Opens the dialog to forward `message`.
pub fn show(message: Arc<Message>, window: &mut Window, cx: &mut App) {
  let dialog = cx.global::<ForwardHandle>().0.clone();
  dialog.update(cx, |dialog, cx| dialog.open(message, window, cx));
}

impl ForwardDialog {
  pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search in Chat"));
    let sub = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
      if matches!(event, InputEvent::Change) {
        this.refresh(cx);
      }
    });
    Self {
      store,
      input,
      focus: cx.focus_handle(),
      scroll: UniformListScrollHandle::new(),
      shown: None,
      targets: Vec::new(),
      picked: None,
      _sub: sub,
    }
  }

  pub const fn is_open(&self) -> bool {
    self.shown.is_some()
  }

  fn open(&mut self, message: Arc<Message>, window: &mut Window, cx: &mut Context<Self>) {
    let previous_focus = match self.shown.take() {
      Some(shown) => shown.previous_focus,
      None => window.focused(cx),
    };
    self.shown = Some(Shown { message, previous_focus });
    self.picked = None;
    self.input.update(cx, |state, cx| {
      state.set_value("", window, cx);
      state.focus(window, cx);
    });
    // Again once the click is over: the menu closing moves focus after this runs.
    let input = self.input.clone();
    window.defer(cx, move |window, cx| input.update(cx, |state, cx| state.focus(window, cx)));
    self.refresh(cx);
    self.scroll.scroll_to_item_strict(0, ScrollStrategy::Top);
  }

  /// Closes without forwarding, and gives the keyboard back.
  pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(shown) = self.shown.take() else {
      return;
    };
    if self.focus.contains_focused(window, cx)
      && let Some(handle) = shown.previous_focus
    {
      window.focus(&handle, cx);
    }
    self.targets.clear();
    cx.notify();
  }

  /// The conversations for the text in the search field: every one the browsing lists show, most recent first, those
  /// whose name (or a one-to-one chat's email) matches the text when there is some.
  fn refresh(&mut self, cx: &mut Context<Self>) {
    let query = self.input.read(cx).value().trim().to_string();
    let store = self.store.read(cx);
    let emails: HashMap<&str, &str> = store
      .people
      .iter()
      .filter(|user| !user.email.is_empty())
      .map(|user| (user.id.as_str(), user.email.as_str()))
      .collect();
    let mut fuzzy = (!query.is_empty()).then(|| Fuzzy::new(&query));
    self.targets = store
      .all
      .iter()
      .filter_map(|conv| {
        let email = (conv.g.kind == GroupKind::Dm)
          .then(|| conv.g.member_ids.first())
          .flatten()
          .and_then(|id| emails.get(id.as_str()).copied());
        if let Some(fuzzy) = &mut fuzzy
          && fuzzy.score(&conv.g.name).is_none()
          && email.is_none_or(|email| fuzzy.score(email).is_none())
        {
          return None;
        }
        Some(Target {
          conv: conv.clone(),
          email: email.map(|email| SharedString::from(email.to_owned())),
        })
      })
      .collect();
    if self.picked_ix().is_none() {
      self.picked = None;
    }
    cx.notify();
  }

  fn picked_ix(&self) -> Option<usize> {
    let picked = self.picked.as_ref()?;
    self.targets.iter().position(|target| &target.conv.g.id == picked)
  }

  fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
    if let Some(target) = self.targets.get(ix) {
      self.picked = Some(target.conv.g.id.clone());
      self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
      cx.notify();
    }
  }

  /// Holds the forward for the conversation picked and opens it there. Nothing happens with none picked.
  fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(to) = self.picked.clone() else {
      return;
    };
    let Some(shown) = self.shown.take() else {
      return;
    };
    self.targets.clear();
    let from = shown.message.group.clone();
    let store = self.store.clone();
    let from_name = store.read(cx).group(&from).map_or_default(|conv| conv.name.clone());
    let forwarding = Forwarding {
      to,
      from,
      from_name,
      message: shown.message,
    };
    store.update(cx, |store, cx| store.forward(forwarding, window, cx));
    cx.notify();
  }

  fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
    match event.keystroke.key.as_str() {
      "escape" => self.close(window, cx),
      key @ ("up" | "down") => {
        if let Some(ix) = step(self.picked_ix(), self.targets.len(), key == "down") {
          self.pick(ix, cx);
        }
      },
      "enter" => self.next(window, cx),
      _ => return,
    }
    cx.stop_propagation();
  }

  fn rows(&self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
    let images = self.store.read(cx).images.clone();
    let picked = self.picked_ix();
    self
      .targets
      .iter()
      .enumerate()
      .skip(range.start)
      .take(range.len())
      .map(|(ix, target)| {
        let selected = picked == Some(ix);
        let row = h_flex()
          .id(("forward-to", ix))
          .w_full()
          .h(px(ROW))
          .px_3()
          .gap_3()
          .items_center()
          .rounded(px(8.))
          .cursor_pointer()
          .when(selected, |row| row.bg(rgb(ROW_SELECTED)))
          .when(!selected, |row| row.hover(|style| style.bg(rgb(ROW_HOVER))))
          .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            this.pick(ix, cx);
            if event.click_count() >= 2 {
              this.next(window, cx);
            }
          }))
          .child(group_avatar(&images, cx, &target.conv.g, 32.))
          .child(
            v_flex()
              .flex_1()
              .min_w_0()
              .child(
                div()
                  .truncate()
                  .text_size(px(14.))
                  .text_color(rgb(TEXT))
                  .child(emoji_font::text(target.conv.name.clone())),
              )
              .children(
                target
                  .email
                  .clone()
                  .map(|email| div().truncate().text_size(px(12.)).text_color(rgb(MUTED)).child(email)),
              ),
          )
          .when(selected, |row| row.child(icon(Icon::Check, 16., ACCENT)));
        // The list sizes its items to their content: the row takes the list's width less a margin each side.
        div().w_full().px_2().child(row).into_any_element()
      })
      .collect()
  }
}

impl Render for ForwardDialog {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    if self.shown.is_none() {
      return div().into_any_element();
    }
    let (width, height) = modal_size(window.viewport_size(), SIZE, (0.5, 0.8));
    let can_next = self.picked.is_some();
    let list = if self.targets.is_empty() {
      div()
        .flex_1()
        .px_5()
        .pt_3()
        .text_size(px(13.))
        .text_color(rgb(MUTED))
        .child("No conversations match")
        .into_any_element()
    } else {
      uniform_list(
        "forward-targets",
        self.targets.len(),
        cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
      )
      .track_scroll(&self.scroll)
      .flex_1()
      .min_h_0()
      .into_any_element()
    };

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
      .on_action(cx.listener(|this, _: &Escape, window, cx| {
        this.close(window, cx);
        cx.stop_propagation();
      }))
      .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close(window, cx)))
      .child(
        v_flex()
          .w(width)
          .h(height)
          .rounded(px(14.))
          .bg(rgb(POPOVER))
          .border_1()
          .border_color(rgb(BORDER))
          .shadow_lg()
          .overflow_hidden()
          .text_color(rgb(TEXT))
          .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| this.on_key(event, window, cx)))
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .child(
            div()
              .px_5()
              .pt_4()
              .pb_3()
              .flex_shrink_0()
              .text_size(px(16.))
              .font_weight(FontWeight::SEMIBOLD)
              .child("Forward message"),
          )
          .child(
            h_flex()
              .mx_4()
              .mb_2()
              .h(px(36.))
              .px_3()
              .gap_2()
              .flex_shrink_0()
              .items_center()
              .rounded(px(18.))
              .bg(control_fill())
              .child(icon(Icon::Search, 15., MUTED))
              .child(
                div()
                  .flex_1()
                  .min_w_0()
                  .child(Input::new(&self.input).appearance(false).bordered(false)),
              ),
          )
          .child(list)
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
              .child(
                plain_button("forward-cancel", "Cancel")
                  .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
              )
              .child(primary_button("forward-next", "Next", can_next).when(can_next, |next| {
                next.on_click(cx.listener(|this, _, window, cx| this.next(window, cx)))
              })),
          )
          .focus_trap("forward-dialog", &self.focus),
      )
      .into_any_element()
  }
}
