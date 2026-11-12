//! Mentions and Starred: messages from every conversation, newest first, in a virtualized list that loads older
//! pages as it nears the end.

use std::ops::Range;

use gchat_core::Shortcut;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;

use crate::home::hit_row;
use crate::store::Store;
use crate::theme::{MUTED, TEXT, rgb};

/// Rows left below the visible ones when the next page is asked for.
const PREFETCH_ROWS: usize = 8;

pub struct ShortcutView {
  store: Entity<Store>,
  scroll: UniformListScrollHandle,
}

impl ShortcutView {
  pub fn new(store: Entity<Store>) -> Self {
    Self {
      store,
      scroll: UniformListScrollHandle::new(),
    }
  }

  fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
    let store = self.store.read(cx);
    let Some(hits) = store.shortcut.as_ref().and_then(|list| list.hits.as_ref()) else {
      return Vec::new();
    };
    let near_end = range.end + PREFETCH_ROWS >= hits.len();
    let rows = range
      .filter_map(|ix| hits.get(ix).map(|hit| hit_row(&self.store, store, ix, hit, "", cx)))
      .collect();
    if near_end {
      let handle = self.store.clone();
      cx.defer(move |cx| handle.update(cx, |store, _| store.load_more_shortcut()));
    }
    rows
  }
}

impl Render for ShortcutView {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let store = self.store.read(cx);
    let Some(list) = store.shortcut.as_ref() else {
      return div().into_any_element();
    };
    let (title, empty) = match list.shortcut {
      Shortcut::Mentions => ("Mentions", "No one has mentioned you yet."),
      Shortcut::Starred => ("Starred", "Messages you star in Google Chat show here."),
    };
    let header = h_flex()
      .h(px(56.))
      .flex_shrink_0()
      .px_4()
      .items_center()
      .child(div().text_size(px(22.)).text_color(rgb(TEXT)).child(title));
    let count = list.hits.as_ref().map_or(0, Vec::len);
    let body = if count == 0 {
      div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(MUTED))
        .child(if list.hits.is_none() {
          "Loading…"
        } else {
          empty
        })
        .into_any_element()
    } else {
      div()
        .flex_1()
        .min_h_0()
        .child(
          uniform_list(
            "shortcut",
            count,
            cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
          )
          .track_scroll(&self.scroll)
          .size_full(),
        )
        .into_any_element()
    };
    v_flex().size_full().child(header).child(body).into_any_element()
  }
}
