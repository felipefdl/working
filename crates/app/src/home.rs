//! Home: every conversation by recency (or the search results) in a virtualized list.

use std::ops::Range;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use gchat_core::SearchHit;

use crate::store::Store;
use crate::theme::{ACCENT, BG, BORDER, MARK, MUTED, ROW_HOVER, TEXT, rgb};
use crate::ui::{avatar, group_avatar};
use crate::util;

const ROW_HEIGHT: f32 = 56.0;

pub struct HomeView {
  store: Entity<Store>,
  scroll: UniformListScrollHandle,
}

impl HomeView {
  pub fn new(store: Entity<Store>) -> Self {
    Self {
      store,
      scroll: UniformListScrollHandle::new(),
    }
  }

  fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
    let store = self.store.read(cx);
    let me = store.me.as_ref().map_or("", |u| u.id.as_str());
    let mut out = Vec::with_capacity(range.len());
    for ix in range {
      let Some(conv) = store.home.get(ix) else {
        // Past the conversation matches come the message hits of the search.
        let search = store.search.as_ref();
        let hit = search
          .and_then(|search| search.hits.as_ref())
          .and_then(|hits| hits.get(ix - store.home.len()));
        let (Some(hit), Some(search)) = (hit, search) else {
          break;
        };
        out.push(hit_row(&self.store, store, ix, hit, &search.query, cx));
        continue;
      };
      let g = &conv.g;
      let snippet = g.snippet.as_ref().map(|s| {
        let sender = if !me.is_empty() && s.sender_id == me {
          "You"
        } else {
          s.sender_name.as_str()
        };
        let text: String = s.text.chars().take(240).map(|c| if c == '\n' { ' ' } else { c }).collect();
        if sender.is_empty() {
          text
        } else {
          format!("{sender}: {text}")
        }
      });
      let time = g.snippet.as_ref().map_or(g.sort_time, |s| s.time);
      let id = g.id.clone();
      let store_handle = self.store.clone();
      out.push(
        h_flex()
          .id(("home", ix))
          .h(px(ROW_HEIGHT))
          .w_full()
          .px_4()
          .gap_3()
          .items_center()
          .cursor_pointer()
          .hover(|s| s.bg(rgb(ROW_HOVER)))
          .on_click(move |_, window, cx| {
            let id = id.clone();
            store_handle.update(cx, |s, cx| s.open(id, window, cx));
          })
          .child(group_avatar(&store.images, cx, g, 40.))
          .child(
            v_flex()
              .flex_1()
              .min_w_0()
              .gap_0p5()
              .child(
                div()
                  .truncate()
                  .text_size(px(14.))
                  .text_color(rgb(TEXT))
                  .when(g.unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                  .child(g.name.clone()),
              )
              .child(
                div()
                  .truncate()
                  .text_size(px(13.))
                  .text_color(rgb(MUTED))
                  .when(g.unread, |d| d.text_color(rgb(TEXT)))
                  .child(snippet.unwrap_or_default()),
              ),
          )
          .child(
            h_flex()
              .flex_shrink_0()
              .gap_2()
              .items_center()
              .child(
                div()
                  .text_size(px(12.))
                  .text_color(rgb(MUTED))
                  .when(g.unread, |d| d.font_weight(FontWeight::SEMIBOLD).text_color(rgb(TEXT)))
                  .child(util::rel_time(time)),
              )
              .child(div().size(px(8.)).rounded_full().when(g.unread, |d| d.bg(rgb(ACCENT)))),
          )
          .into_any_element(),
      );
    }
    out
  }
}

impl Render for HomeView {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let store = self.store.read(cx);
    let hits = store
      .search
      .as_ref()
      .and_then(|search| search.hits.as_ref())
      .map_or(0, |hits| hits.len());
    let count = store.home.len() + hits;
    let searching = !store.query.is_empty();
    let unread_only = store.unread_only;
    let title: SharedString = match &store.search {
      Some(search) if search.hits.is_none() => format!("Searching messages for \"{}\"…", search.query).into(),
      Some(search) => format!("Results for \"{}\"", search.query).into(),
      None if searching => "Conversations (Enter searches messages)".into(),
      None => "Home".into(),
    };
    let store_handle = self.store.clone();

    let header = h_flex()
      .h(px(56.))
      .flex_shrink_0()
      .px_4()
      .items_center()
      .justify_between()
      .child(div().text_size(px(22.)).text_color(rgb(TEXT)).child(title))
      .child(
        h_flex()
          .id("unread-toggle")
          .gap_2()
          .items_center()
          .cursor_pointer()
          .on_click(move |_, _, cx| store_handle.update(cx, |s, cx| s.set_unread_only(!s.unread_only, cx)))
          .child(div().text_size(px(13.)).text_color(rgb(TEXT)).child("Unread"))
          .child(
            div()
              .w(px(34.))
              .h(px(18.))
              .rounded_full()
              .p(px(2.))
              .flex()
              .when(unread_only, |d| d.bg(rgb(ACCENT)).justify_end())
              .when(!unread_only, |d| d.bg(rgb(BORDER)).justify_start())
              .child(div().size(px(14.)).rounded_full().bg(rgb(if unread_only { BG } else { MUTED }))),
          ),
      );

    let body = if count == 0 {
      div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(MUTED))
        .child(if store.search.as_ref().is_some_and(|search| search.hits.is_none()) {
          "Searching…"
        } else if store.search.is_some() {
          "No results"
        } else if searching {
          "No conversations match. Press Enter to search messages."
        } else if unread_only {
          "No unread conversations"
        } else {
          "No conversations yet"
        })
        .into_any_element()
    } else {
      div()
        .flex_1()
        .min_h_0()
        .child(
          uniform_list(
            "home",
            count,
            cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
          )
          .track_scroll(&self.scroll)
          .size_full(),
        )
        .into_any_element()
    };

    v_flex().size_full().child(header).child(body)
  }
}

/// One message found by the search: sender, conversation, time, and the text with the query's words marked.
fn hit_row(handle: &Entity<Store>, store: &Store, ix: usize, hit: &SearchHit, query: &str, cx: &App) -> AnyElement {
  let conversation = store.group(&hit.group).map(|conv| conv.g.name.clone()).unwrap_or_default();
  let title = if conversation.is_empty() || conversation == hit.sender_name {
    hit.sender_name.clone()
  } else {
    format!("{} · {}", hit.sender_name, conversation)
  };
  // One line; newlines become spaces without moving byte offsets.
  let text: String = hit.text.chars().take(400).map(|c| if c == '\n' { ' ' } else { c }).collect();
  let lower = text.to_ascii_lowercase();
  let mark = HighlightStyle {
    background_color: Some(rgb(MARK).into()),
    color: Some(rgb(TEXT).into()),
    ..Default::default()
  };
  let mut marks: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
  for word in query
    .to_ascii_lowercase()
    .split(|c: char| !c.is_alphanumeric())
    .filter(|word| word.len() > 1)
  {
    marks.extend(
      lower
        .match_indices(word)
        .map(|(start, found)| (start..start + found.len(), mark)),
    );
  }
  marks.sort_by_key(|(range, _)| range.start);
  // Overlapping marks would be rejected by the text layout: keep the first of each overlap.
  let mut end = 0;
  marks.retain(|(range, _)| {
    let keep = range.start >= end && text.is_char_boundary(range.start) && text.is_char_boundary(range.end);
    if keep {
      end = range.end;
    }
    keep
  });
  let opened = hit.clone();
  let handle = handle.clone();
  h_flex()
    .id(("hit", ix))
    .h(px(ROW_HEIGHT))
    .w_full()
    .px_4()
    .gap_3()
    .items_center()
    .cursor_pointer()
    .hover(|s| s.bg(rgb(ROW_HOVER)))
    .on_click(move |_, window, cx| handle.update(cx, |store, cx| store.open_hit(&opened, window, cx)))
    .child(avatar(
      &store.images,
      cx,
      hit.sender_avatar.as_deref(),
      None,
      &hit.sender_name,
      40.,
    ))
    .child(
      v_flex()
        .flex_1()
        .min_w_0()
        .gap_0p5()
        .child(div().truncate().text_size(px(14.)).text_color(rgb(TEXT)).child(title))
        .child(
          div()
            .truncate()
            .text_size(px(13.))
            .text_color(rgb(MUTED))
            .child(StyledText::new(text).with_highlights(marks)),
        ),
    )
    .child(
      div()
        .flex_shrink_0()
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .child(util::rel_time(hit.time)),
    )
    .into_any_element()
}
