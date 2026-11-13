//! The conversation list of the Messages and Rail layouts: chats with previews beside the open conversation. Messages
//! filters with chips under a header of buttons; Rail lists the pane its icon column picked.

use std::ops::Range;

use gchat_core::{GroupId, SearchHit, Shortcut};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::calendar::OpenCalendar;
use crate::home::{ROW_HEIGHT, RowMarks, conv_row, hit_row, unread_toggle};
use crate::meet::OpenMeet;
use crate::menu;
use crate::new_chat::OpenNewChat;
use crate::settings::{Layout, Settings};
use crate::store::{Kind, Pane, Store, View};
use crate::theme::{ACCENT, ACCENT_BG, BORDER, FIELD, MUTED, ROW_HOVER, TEXT, rgb};
use crate::ui::{Icon, icon};

/// A list row: the shared conversation row with a sliver of air above and below, so selected rows stand apart.
const LIST_ROW_HEIGHT: f32 = ROW_HEIGHT + 4.0;
/// Rows left below the visible ones when the next page of Mentions or Starred is asked for.
const PREFETCH_ROWS: usize = 8;

/// What the rows were filtered by. When any of it changes the list starts over at the top.
struct Filter {
  pane: Pane,
  unread_only: bool,
  query: String,
}

impl Filter {
  fn new(store: &Store) -> Self {
    Self {
      pane: store.pane.clone(),
      unread_only: store.unread_only,
      query: store.query.clone(),
    }
  }

  fn matches(&self, store: &Store) -> bool {
    self.pane == store.pane && self.unread_only == store.unread_only && self.query == store.query
  }
}

pub struct ConvList {
  store: Entity<Store>,
  scroll: UniformListScrollHandle,
  filter: Filter,
  /// The open conversation the list last scrolled to.
  followed: Option<GroupId>,
}

impl ConvList {
  pub fn new(store: Entity<Store>) -> Self {
    Self {
      store,
      scroll: UniformListScrollHandle::new(),
      // What a new store starts with.
      filter: Filter {
        pane: Pane::All,
        unread_only: false,
        query: String::new(),
      },
      followed: None,
    }
  }

  /// Lists another pane. The Unread chip and the Unread switch are one filter, so picking a pane ends the switch.
  fn pick(&mut self, pane: Pane, cx: &mut Context<Self>) {
    self.store.update(cx, |store, cx| {
      if store.unread_only {
        store.set_unread_only(false, cx);
      }
      store.set_pane(pane, cx);
    });
  }

  /// The Rail's Unread switch. Turning it off also ends the Unread pane a Messages chip may have left.
  fn set_only_unread(&mut self, on: bool, cx: &mut Context<Self>) {
    self.store.update(cx, |store, cx| {
      store.set_unread_only(on, cx);
      if !on && store.pane == Pane::Unread {
        store.set_pane(Pane::All, cx);
      }
    });
  }

  /// A new filter, whoever set it, lists from the first row.
  fn follow_filter(&mut self, store: &Store) {
    if self.filter.matches(store) {
      return;
    }
    self.filter = Filter::new(store);
    self.scroll.scroll_to_item_strict(0, ScrollStrategy::Top);
  }

  /// Scrolls to the open conversation when it changes, so stepping with the keys or the switcher keeps it in view.
  fn follow_open(&mut self, store: &Store) {
    let open = match &store.view {
      View::Conversation(id) => Some(id),
      View::Home | View::Shortcut(_) | View::Calendar | View::Meet => None,
    };
    if open == self.followed.as_ref() {
      return;
    }
    if let Some(ix) = open.and_then(|id| store.list.iter().position(|conv| conv.g.id == *id)) {
      self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
    }
    self.followed = open.cloned();
  }

  fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
    let store = self.store.read(cx);
    if !matches!(store.pane, Pane::Shortcut(_)) {
      return self.conversation_rows(range, store, cx);
    }
    let Some(hits) = shortcut_hits(store) else {
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

  fn conversation_rows(&self, range: Range<usize>, store: &Store, cx: &App) -> Vec<AnyElement> {
    let open = match &store.view {
      View::Conversation(id) => Some(id),
      View::Home | View::Shortcut(_) | View::Calendar | View::Meet => None,
    };
    let mut out = Vec::with_capacity(range.len());
    for ix in range {
      let Some(conv) = store.list.get(ix) else {
        break;
      };
      let marks = RowMarks {
        selected: open == Some(&conv.g.id),
        pin: true,
      };
      let row = conv_row(&self.store, store, ("conv-row", ix), conv, marks, cx)
        .px_3()
        .rounded(px(10.));
      out.push(
        div()
          .id(("conv", ix))
          .h(px(LIST_ROW_HEIGHT))
          .w_full()
          .px_2()
          .py_0p5()
          .child(row)
          .into_any_element(),
      );
    }
    out
  }

  /// Messages: the title with buttons for Mentions, Starred, Calendar, Meet, and New chat, then the filter chips.
  fn messages_header(&self, store: &Store, cx: &Context<Self>) -> Div {
    let shown = match &store.view {
      View::Shortcut(shortcut) => Some(*shortcut),
      View::Home | View::Calendar | View::Meet | View::Conversation(_) => None,
    };
    let shortcut_button = |id: &'static str, glyph: Icon, name: &'static str, shortcut: Shortcut| {
      let handle = self.store.clone();
      header_button(id, glyph, shown == Some(shortcut))
        .tooltip(hint(name))
        .on_click(move |_, _, cx| handle.update(cx, |s, cx| s.open_shortcut(shortcut, cx)))
    };
    let (show_mentions, show_starred, show_calendar, show_meet) = {
      let settings = cx.global::<Settings>();
      (
        settings.show_mentions,
        settings.show_starred,
        settings.show_calendar,
        settings.show_meet,
      )
    };
    let actions = h_flex()
      .gap_1()
      .when(show_mentions, |actions| {
        actions.child(shortcut_button("mentions", Icon::AtSign, "Mentions", Shortcut::Mentions))
      })
      .when(show_starred, |actions| {
        actions.child(shortcut_button("starred", Icon::Star, "Starred", Shortcut::Starred))
      })
      .when(show_calendar, |actions| {
        actions.child(
          header_button("calendar", Icon::Calendar, store.view == View::Calendar)
            .tooltip(calendar_tip)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenCalendar), cx)),
        )
      })
      .when(show_meet, |actions| {
        actions.child(
          header_button("meet", Icon::Video, store.view == View::Meet)
            .tooltip(meet_tip)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenMeet), cx)),
        )
      })
      .when(shown.is_none(), |actions| actions.child(sort_button("sort-list", cx)))
      .child(
        header_button("new-chat", Icon::Plus, false)
          .tooltip(new_chat_tip)
          .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenNewChat), cx)),
      );
    v_flex()
      .flex_shrink_0()
      .child(
        h_flex()
          .h(px(52.))
          .px_5()
          .items_center()
          .gap_2()
          .child(
            div()
              .flex_1()
              .min_w_0()
              .truncate()
              .text_size(px(22.))
              .text_color(rgb(TEXT))
              .child("Chats"),
          )
          .child(actions.flex_shrink_0().items_center()),
      )
      .child(self.chips(store, cx))
  }

  /// The filters: All and Unread, then the kinds of conversation the account has, on one line: they fit the
  /// default width, and scroll sideways when the list is dragged narrower.
  fn chips(&self, store: &Store, cx: &Context<Self>) -> Stateful<Div> {
    let mut chips = chip_row("messages-chips");
    for (ix, pane) in chip_panes(store).into_iter().enumerate() {
      // Short labels keep the chips on one line in the list's width.
      let label = match &pane {
        Pane::All => "All",
        _ => pane_title(&pane),
      };
      let chip = chip_button(ix, label, chip_on(store, &pane));
      let pick = cx.listener(move |this, _, _, cx| this.pick(pane.clone(), cx));
      chips = chips.child(chip.on_click(pick));
    }
    chips
  }

  /// Rail: the title of the pane and, for conversations, the Unread switch.
  fn rail_header(&self, store: &Store, cx: &Context<Self>) -> Div {
    let on = only_unread(store);
    h_flex()
      .h(px(56.))
      .flex_shrink_0()
      .px_5()
      .gap_3()
      .items_center()
      .justify_between()
      .child(
        div()
          .flex_1()
          .min_w_0()
          .truncate()
          .text_size(px(22.))
          .text_color(rgb(TEXT))
          .child(pane_title(&store.pane).to_string()),
      )
      .when(!matches!(store.pane, Pane::Shortcut(_)), |header| {
        let flip = cx.listener(move |this, _, _, cx| this.set_only_unread(!on, cx));
        header.child(
          h_flex()
            .gap_3()
            .items_center()
            .child(sort_button("sort-list", cx))
            .child(unread_toggle(on).on_click(flip)),
        )
      })
  }
}

/// The name of a pane: the title of the Rail list, and the tooltip of its button.
pub fn pane_title(pane: &Pane) -> &str {
  match pane {
    Pane::All => "All chats",
    Pane::Unread => "Unread",
    Pane::Kind(kind) => kind.title(),
    Pane::Section(section) => section.title(),
    Pane::Shortcut(Shortcut::Mentions) => "Mentions",
    Pane::Shortcut(Shortcut::Starred) => "Starred",
  }
}

/// The tooltip of a New chat button: its name and keyboard shortcut.
pub fn new_chat_tip(window: &mut Window, cx: &mut App) -> AnyView {
  Tooltip::new("New chat").action(&OpenNewChat, None).build(window, cx)
}

/// The tooltip of a Calendar button: its name and keyboard shortcut.
pub fn calendar_tip(window: &mut Window, cx: &mut App) -> AnyView {
  Tooltip::new("Calendar").action(&OpenCalendar, None).build(window, cx)
}

/// The tooltip of a Meet button: its name and keyboard shortcut.
pub fn meet_tip(window: &mut Window, cx: &mut App) -> AnyView {
  Tooltip::new("Meet").action(&OpenMeet, None).build(window, cx)
}

/// A tooltip naming a button.
pub fn hint(text: &'static str) -> impl Fn(&mut Window, &mut App) -> AnyView {
  move |window, cx| Tooltip::new(text).build(window, cx)
}

/// Whether the list holds only unread conversations: the Unread switch is on, or the Unread pane is picked.
fn only_unread(store: &Store) -> bool {
  store.unread_only || store.pane == Pane::Unread
}

/// The loaded messages of the Mentions or Starred pane; `None` until the core answers.
fn shortcut_hits(store: &Store) -> Option<&[SearchHit]> {
  let Pane::Shortcut(shortcut) = &store.pane else {
    return None;
  };
  let list = store.shortcut.as_ref().filter(|list| list.shortcut == *shortcut)?;
  list.hits.as_deref()
}

/// How many rows the list has: conversations, or in the Rail's Mentions and Starred the messages loaded so far.
fn row_count(store: &Store, rail: bool) -> usize {
  if rail && matches!(store.pane, Pane::Shortcut(_)) {
    shortcut_hits(store).map_or(0, <[SearchHit]>::len)
  } else {
    store.list.len()
  }
}

/// What the list says when it has no rows.
fn empty_text(store: &Store, rail: bool) -> &'static str {
  match &store.pane {
    // The Messages layout opens Mentions and Starred from its buttons; a pane left over from the Rail lists nothing.
    Pane::Shortcut(_) if !rail => "Pick All or another filter to list your conversations.",
    Pane::Shortcut(_) if shortcut_hits(store).is_none() => "Loading…",
    Pane::Shortcut(Shortcut::Mentions) => "No one has mentioned you yet.",
    Pane::Shortcut(Shortcut::Starred) => "Messages you star in Google Chat show here.",
    _ if !store.query.is_empty() => "No conversations match. Press Enter to search messages.",
    _ if only_unread(store) => "No unread conversations",
    Pane::Section(_) | Pane::Kind(_) => "No conversations here",
    Pane::All | Pane::Unread => "No conversations yet",
  }
}

/// The kinds of conversation the account has, in the order of the chips.
pub fn kinds_present(store: &Store) -> impl Iterator<Item = Kind> + '_ {
  Kind::ALL
    .into_iter()
    .filter(|kind| store.tabs.iter().any(|tab| tab.pane == Pane::Kind(*kind)))
}

/// The Messages chips: All and Unread, then People, Groups, Spaces, and Apps when the account has them, then the pane
/// in use when it is none of those (the Rail can leave a section or Mentions picked).
fn chip_panes(store: &Store) -> Vec<Pane> {
  let mut panes = vec![Pane::All, Pane::Unread];
  panes.extend(kinds_present(store).map(Pane::Kind));
  if !panes.contains(&store.pane) {
    panes.push(store.pane.clone());
  }
  panes
}

/// Whether a Messages chip is the filter in use. Unread also reads as on when the Unread switch was left on by the
/// other layouts, and then All is not.
fn chip_on(store: &Store, pane: &Pane) -> bool {
  match pane {
    Pane::Unread => only_unread(store),
    Pane::All => store.pane == Pane::All && !store.unread_only,
    Pane::Kind(_) | Pane::Section(_) | Pane::Shortcut(_) => store.pane == *pane,
  }
}

/// The line the filter chips sit on: never wrapped, scrolled sideways when they do not fit.
pub fn chip_row(id: &'static str) -> Stateful<Div> {
  h_flex().id(id).w_full().gap_1().px_4().pb_2().overflow_x_scroll()
}

pub fn chip_button(ix: usize, label: &str, on: bool) -> Stateful<Div> {
  div()
    .id(("chip", ix))
    .flex_shrink_0()
    .px_2()
    .py_0p5()
    .rounded_full()
    .text_size(px(11.))
    .cursor_pointer()
    .bg(rgb(if on { ACCENT_BG } else { FIELD }))
    .text_color(rgb(if on { ACCENT } else { TEXT }))
    .when(!on, |chip| chip.hover(|s| s.bg(rgb(BORDER))))
    .child(label.to_string())
}

/// A round icon button of the Messages header. `on` paints the one whose view is open.
fn header_button(id: &'static str, glyph: Icon, on: bool) -> Stateful<Div> {
  div()
    .id(id)
    .size(px(30.))
    .flex()
    .items_center()
    .justify_center()
    .rounded_full()
    .cursor_pointer()
    .when(on, |button| button.bg(rgb(ACCENT_BG)))
    .when(!on, |button| button.hover(|s| s.bg(rgb(ROW_HOVER))))
    .child(icon(glyph, 16., if on { ACCENT } else { MUTED }))
}

/// The sort button of a list header: opens the menu of the two orders, the one in `Settings::list_sort` checked, and
/// picking one writes it.
pub fn sort_button(id: &'static str, cx: &App) -> Stateful<Div> {
  let current = cx.global::<Settings>().list_sort;
  header_button(id, Icon::ArrowDownAZ, false).tooltip(hint("Sort")).on_mouse_down(
    MouseButton::Left,
    move |event, window, cx| {
      cx.stop_propagation();
      let items = menu::sort_items(current, |sort, cx| Settings::update(cx, |settings| settings.list_sort = sort));
      menu::show(event.position, items, window, cx);
    },
  )
}

impl Render for ConvList {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let rail = cx.global::<Settings>().layout == Layout::Rail;
    let store = self.store.read(cx);
    self.follow_filter(store);
    self.follow_open(store);
    let header = if rail {
      self.rail_header(store, cx)
    } else {
      self.messages_header(store, cx)
    };
    let count = row_count(store, rail);
    let body = if count == 0 {
      div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .px_6()
        .text_center()
        .text_color(rgb(MUTED))
        .child(empty_text(store, rail))
        .into_any_element()
    } else {
      div()
        .flex_1()
        .min_h_0()
        .child(
          uniform_list(
            "conv-list",
            count,
            cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
          )
          .track_scroll(&self.scroll)
          .size_full(),
        )
        .into_any_element()
    };
    v_flex()
      .w(px(cx.global::<Settings>().column_width(if rail {
        Layout::Rail
      } else {
        Layout::Messages
      })))
      .h_full()
      .flex_shrink_0()
      .child(header)
      .child(body)
  }
}
