//! One context menu for the whole window: a transparent catcher plus a small popup at the pointer.

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::{BORDER, POPOVER, ROW_HOVER, TEXT, rgb};

const WIDTH: f32 = 200.0;
const ITEM_HEIGHT: f32 = 30.0;
/// Side of one cell of a strip.
const CELL: f32 = 36.0;

pub struct MenuItem {
  label: SharedString,
  run: Rc<dyn Fn(&mut Window, &mut App)>,
}

impl MenuItem {
  pub fn new(label: impl Into<SharedString>, run: impl Fn(&mut Window, &mut App) + 'static) -> Self {
    Self { label: label.into(), run: Rc::new(run) }
  }
}

struct Shown {
  at: Point<Pixels>,
  items: Vec<MenuItem>,
  /// Items side by side as square cells (an emoji picker) instead of a list.
  strip: bool,
  previous_focus: Option<FocusHandle>,
}

pub struct ContextMenu {
  shown: Option<Shown>,
  focus: FocusHandle,
}

/// The window's context menu, reachable from any click handler.
pub struct MenuHandle(pub Entity<ContextMenu>);

impl Global for MenuHandle {}

/// Opens the context menu at `at` (window coordinates).
pub fn show(at: Point<Pixels>, items: Vec<MenuItem>, window: &mut Window, cx: &mut App) {
  let menu = cx.global::<MenuHandle>().0.clone();
  menu.update(cx, |menu, cx| menu.open(at, items, false, window, cx));
}

/// Opens a strip of short choices (emoji) at `at`.
pub fn show_strip(at: Point<Pixels>, items: Vec<MenuItem>, window: &mut Window, cx: &mut App) {
  let menu = cx.global::<MenuHandle>().0.clone();
  menu.update(cx, |menu, cx| menu.open(at, items, true, window, cx));
}

impl ContextMenu {
  pub fn new(cx: &mut Context<Self>) -> Self {
    Self { shown: None, focus: cx.focus_handle() }
  }

  pub const fn is_open(&self) -> bool {
    self.shown.is_some()
  }

  fn open(
    &mut self,
    at: Point<Pixels>,
    items: Vec<MenuItem>,
    strip: bool,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if items.is_empty() {
      return;
    }
    let previous_focus = match self.shown.take() {
      Some(shown) => shown.previous_focus,
      None => window.focused(cx),
    };
    self.shown = Some(Shown { at, items, strip, previous_focus });
    window.focus(&self.focus, cx);
    // Again once the click is over: a left mouse press moves focus to what was pressed after this runs.
    let focus = self.focus.clone();
    window.defer(cx, move |window, cx| window.focus(&focus, cx));
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
    let Some(run) = self
      .shown
      .as_ref()
      .and_then(|shown| shown.items.get(ix))
      .map(|item| item.run.clone())
    else {
      return;
    };
    self.close(window, cx);
    // After this update ends: an item that opens another menu (Do not disturb's durations) updates this entity.
    window.defer(cx, move |window, cx| run(window, cx));
  }
}

impl Render for ContextMenu {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let Some(shown) = &self.shown else {
      return div().into_any_element();
    };
    let window_size = window.viewport_size();
    let count = shown.items.len() as f32;
    let (width, height) = if shown.strip {
      (count.mul_add(CELL, 8.), CELL + 8.)
    } else {
      (WIDTH, count.mul_add(ITEM_HEIGHT, 8.))
    };
    let left = shown.at.x.min(window_size.width - px(width + 8.)).max(px(8.));
    let top = shown.at.y.min(window_size.height - px(height + 8.)).max(px(8.));

    let mut panel = div()
      .absolute()
      .left(left)
      .top(top)
      .flex()
      .when(shown.strip, gpui_kit::Styled::px_1)
      .when(!shown.strip, |panel| panel.flex_col().w(px(WIDTH)))
      .py_1()
      .rounded(px(10.))
      .bg(rgb(POPOVER))
      .border_1()
      .border_color(rgb(BORDER))
      .shadow_lg()
      .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
      .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
    for (ix, item) in shown.items.iter().enumerate() {
      let cell = div()
        .id(("menu-item", ix))
        .flex()
        .items_center()
        .rounded(px(6.))
        .text_color(rgb(TEXT))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(ROW_HOVER)))
        .on_click(cx.listener(move |this, _, window, cx| this.pick(ix, window, cx)))
        .child(item.label.clone());
      panel = panel.child(if shown.strip {
        cell.size(px(CELL)).justify_center().text_size(px(19.))
      } else {
        cell.h(px(ITEM_HEIGHT)).mx_1().px_3().text_size(px(13.))
      });
    }

    div()
      .absolute()
      .top_0()
      .left_0()
      .size_full()
      .occlude()
      .track_focus(&self.focus)
      .key_context("ContextMenu")
      .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
        if event.keystroke.key == "escape" {
          this.close(window, cx);
          cx.stop_propagation();
        }
      }))
      .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close(window, cx)))
      .on_mouse_down(MouseButton::Right, cx.listener(|this, _, window, cx| this.close(window, cx)))
      .child(panel)
      .into_any_element()
  }
}
