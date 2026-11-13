//! Home: every conversation by recency (or the search results) in a virtualized list. Beside the conversation list of
//! the Messages and Rail layouts it shows only the message search. Also the rows the lists share.

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use gchat_core::SearchHit;

use crate::conv_list::{chip_button, chip_row, kinds_present, sort_button};
use crate::settings::Settings;
use crate::store::{Conv, Pane, Store};
use crate::theme::{ACCENT, FIELD, MARK, MUTED, ROW_HOVER, ROW_SELECTED, TEXT, rgb};
use crate::ui::{Icon, avatar, group_avatar, icon, switch};
use crate::util;

/// Height of a conversation or message row.
pub const ROW_HEIGHT: f32 = 56.0;

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
    let kept = cx.global::<Settings>().layout.keeps_list();
    let store = self.store.read(cx);
    let convs = conversations(store, kept);
    let mut out = Vec::with_capacity(range.len());
    for ix in range {
      if let Some(conv) = convs.get(ix) {
        let row = conv_row(&self.store, store, ("home", ix), conv, RowMarks::default(), cx);
        out.push(row.into_any_element());
        continue;
      }
      // Past the conversation matches come the message hits of the search.
      let search = store.search.as_ref();
      let hit = search
        .and_then(|search| search.hits.as_ref())
        .and_then(|hits| hits.get(ix - convs.len()));
      let (Some(hit), Some(search)) = (hit, search) else {
        break;
      };
      out.push(hit_row(&self.store, store, ix, hit, &search.query, cx));
    }
    out
  }
}

/// The conversations Home lists. Beside a conversation list (`kept`) they are already there, so Home has none.
fn conversations(store: &Store, kept: bool) -> &[Arc<Conv>] {
  if kept { &[] } else { &store.home }
}

/// What the header says: the search under way or done, or the plain Home.
fn heading(store: &Store) -> SharedString {
  match &store.search {
    Some(search) if search.hits.is_none() => format!("Searching messages for \"{}\"…", search.query).into(),
    Some(search) => format!("Results for \"{}\"", search.query).into(),
    None if !store.query.is_empty() => "Conversations (Enter searches messages)".into(),
    None => "Home".into(),
  }
}

/// What Home says when it has no rows.
fn empty_text(store: &Store) -> &'static str {
  if store.search.as_ref().is_some_and(|search| search.hits.is_none()) {
    "Searching…"
  } else if store.search.is_some() {
    "No results"
  } else if !store.query.is_empty() {
    "No conversations match. Press Enter to search messages."
  } else if store.unread_only {
    "No unread conversations"
  } else {
    "No conversations yet"
  }
}

impl Render for HomeView {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let kept = cx.global::<Settings>().layout.keeps_list();
    let store = self.store.read(cx);
    // Beside a conversation list the conversation matches are already there: Home is the message search or, with
    // none running, a hint about picking a conversation.
    if kept && store.search.is_none() {
      return select_conversation(!store.query.is_empty()).into_any_element();
    }
    let hits = store
      .search
      .as_ref()
      .and_then(|search| search.hits.as_ref())
      .map_or(0, |hits| hits.len());
    let count = conversations(store, kept).len() + hits;
    let unread_only = store.unread_only;
    let store_handle = self.store.clone();

    // The filters under the title: All and the kinds of conversation the account has.
    let mut chips = vec![("All", Pane::All)];
    chips.extend(kinds_present(store).map(|kind| (kind.title(), Pane::Kind(kind))));
    let chips = (chips.len() > 1).then(|| {
      let mut row = chip_row("home-chips");
      for (ix, (name, pane)) in chips.into_iter().enumerate() {
        let on = store.pane == pane || (pane == Pane::All && store.pane == Pane::Unread);
        let handle = self.store.clone();
        row = row.child(
          chip_button(ix, name, on).on_click(move |_, _, cx| handle.update(cx, |s, cx| s.set_pane(pane.clone(), cx))),
        );
      }
      row
    });

    let header = v_flex()
      .flex_shrink_0()
      .child(
        h_flex()
          .h(px(56.))
          .px_4()
          .items_center()
          .justify_between()
          .child(div().text_size(px(22.)).text_color(rgb(TEXT)).child(heading(store)))
          .when(!kept, |header| {
            header.child(h_flex().gap_3().items_center().child(sort_button("sort-home", cx)).child(
              unread_toggle(unread_only).on_click(move |_, _, cx| {
                store_handle.update(cx, |s, cx| s.set_unread_only(!s.unread_only, cx));
              }),
            ))
          }),
      )
      .when_some(chips.filter(|_| !kept), ParentElement::child);

    let body = if count == 0 {
      div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(MUTED))
        .child(empty_text(store))
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

    v_flex().size_full().child(header).child(body).into_any_element()
  }
}

/// The Unread switch of a list header, with its label. The caller sets what a click does.
pub fn unread_toggle(on: bool) -> Stateful<Div> {
  h_flex()
    .id("unread-toggle")
    .gap_2()
    .items_center()
    .cursor_pointer()
    .child(div().text_size(px(13.)).text_color(rgb(TEXT)).child("Unread"))
    .child(switch(on, 18.))
}

/// What the pane beside the conversation list says with nothing open and no search: how to pick a conversation.
/// `typing` adds that Enter searches messages.
fn select_conversation(typing: bool) -> Div {
  let hint = |keys: &'static str, what: &'static str| {
    h_flex()
      .gap_3()
      .items_center()
      .child(
        div().w(px(64.)).flex().justify_end().child(
          div()
            .px_1p5()
            .py_0p5()
            .rounded(px(6.))
            .bg(rgb(FIELD))
            .text_size(px(12.))
            .text_color(rgb(TEXT))
            .child(keys),
        ),
      )
      .child(div().w(px(150.)).text_size(px(13.)).text_color(rgb(MUTED)).child(what))
  };
  v_flex()
    .size_full()
    .items_center()
    .justify_center()
    .gap_4()
    .child(icon(Icon::Thread, 40., MUTED).opacity(0.5))
    .child(div().text_size(px(18.)).text_color(rgb(TEXT)).child("Select a conversation"))
    .child(
      v_flex()
        .gap_2()
        .when(typing, |hints| hints.child(hint("⏎", "Search messages")))
        .child(hint("⌘N", "New chat"))
        .child(hint("⌥⌘↓", "Next conversation")),
    )
}

/// How a `conv_row` differs from the row Home draws.
#[derive(Clone, Copy, Default)]
pub struct RowMarks {
  /// The conversation is open: its row is painted selected instead of lighting up under the pointer.
  pub selected: bool,
  /// A pinned conversation shows a pin beside its time.
  pub pin: bool,
}

/// One conversation: avatar, name, a preview line led by "You:" or the sender's name, the time of the last message,
/// and a dot while it is unread. A click opens it. The caller sets the id and may restyle the frame.
pub fn conv_row(
  handle: &Entity<Store>,
  store: &Store,
  id: impl Into<ElementId>,
  conv: &Conv,
  marks: RowMarks,
  cx: &App,
) -> Stateful<Div> {
  let g = &conv.g;
  let me = store.me.as_ref().map_or("", |u| u.id.as_str());
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
  let opened = g.id.clone();
  let handle = handle.clone();
  h_flex()
    .id(id)
    .h(px(ROW_HEIGHT))
    .w_full()
    .px_4()
    .gap_3()
    .items_center()
    .cursor_pointer()
    .when(marks.selected, |row| row.bg(rgb(ROW_SELECTED)))
    .when(!marks.selected, |row| row.hover(|s| s.bg(rgb(ROW_HOVER))))
    .on_click(move |_, window, cx| {
      let group = opened.clone();
      handle.update(cx, |s, cx| s.open(group, window, cx));
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
        .when(marks.pin && g.pinned, |d| d.child(icon(Icon::Pin, 12., MUTED)))
        .child(
          div()
            .text_size(px(12.))
            .text_color(rgb(MUTED))
            .when(g.unread, |d| d.font_weight(FontWeight::SEMIBOLD).text_color(rgb(TEXT)))
            .child(util::rel_time(time)),
        )
        .child(div().size(px(8.)).rounded_full().when(g.unread, |d| d.bg(rgb(ACCENT)))),
    )
}

/// One message found by the search or listed by a shortcut: sender, conversation, time, and the text with the
/// query's words marked.
pub fn hit_row(handle: &Entity<Store>, store: &Store, ix: usize, hit: &SearchHit, query: &str, cx: &App) -> AnyElement {
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
