//! The conversation list of the Messages, Rail and IRC layouts beside the open conversation. Messages lists chats
//! with previews, filtered with chips under a header of buttons; Rail lists the pane its icon column picked; IRC has
//! the Messages header and chips over one line of the monospaced name a conversation, as WeeChat's buffer list.

use std::ops::Range;

use gchat_core::{Group, GroupId, GroupKind, SearchHit, Shortcut};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::calendar::OpenCalendar;
use crate::emoji_font;
use crate::home::{ROW_HEIGHT, RowMarks, conv_row, conversation_menu, hit_row, minute_clock, unread_toggle};
use crate::keys;
use crate::mail::OpenMail;
use crate::meet::OpenMeet;
use crate::menu;
use crate::new_chat::OpenNewChat;
use crate::settings::{Layout, Place, Settings};
use crate::store::{Conv, Kind, ListRow, Pane, Store, View};
use crate::theme::{ACCENT, ACCENT_BG, BORDER, FIELD, MUTED, ROW_HOVER, ROW_SELECTED, TEXT, mono, rgb};
use crate::ui::{Icon, count_badge, icon, person_status, status_dot};

/// A list row: the shared conversation row with a sliver of air above and below, so selected rows stand apart.
const LIST_ROW_HEIGHT: f32 = ROW_HEIGHT + 4.0;
/// Rows left below the visible ones when the next page of Mentions or Starred is asked for.
const PREFETCH_ROWS: usize = 8;
/// An IRC buffer row: one line of the monospaced name.
const BUFFER_ROW_HEIGHT: f32 = 24.0;
/// Narrowest the Messages list shows its title beside the header buttons.
const TITLE_MIN_WIDTH: f32 = 300.0;

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

/// The window root draws it as a cached view, so it redraws itself whenever what it draws changes: the store, the
/// settings, an image arriving, or the minute its rows' times count.
pub struct ConvList {
  store: Entity<Store>,
  scroll: UniformListScrollHandle,
  filter: Filter,
  /// The open conversation the list last scrolled to.
  followed: Option<GroupId>,
  _subs: [Subscription; 3],
  _clock: Task<()>,
}

impl ConvList {
  pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
    let tick = store.read(cx).images.tick().clone();
    let subs = [
      cx.observe(&store, |_, _, cx| cx.notify()),
      cx.observe_global::<Settings>(|_, cx| cx.notify()),
      cx.observe(&tick, |_, _, cx| cx.notify()),
    ];
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
      _subs: subs,
      _clock: minute_clock(cx),
    }
  }

  /// Lists another pane. The Unread chip and the Unread switch are one filter, so picking a pane ends the switch.
  fn pick(store: &Entity<Store>, pane: Pane, cx: &mut App) {
    store.update(cx, |store, cx| {
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
      View::Home | View::Shortcut(_) | View::Calendar | View::Meet | View::Mail => None,
    };
    if open == self.followed.as_ref() {
      return;
    }
    if let Some(ix) = open.and_then(|id| store.list.position(id)) {
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
      View::Home | View::Shortcut(_) | View::Calendar | View::Meet | View::Mail => None,
    };
    let irc = cx.global::<Settings>().layout == Layout::Irc;
    let mut out = Vec::with_capacity(range.len());
    for ix in range {
      let conv = match store.list.row(ix) {
        Some(ListRow::Conv(conv)) => conv,
        Some(ListRow::Older) => {
          out.push(self.older_row(store, irc).into_any_element());
          continue;
        },
        None => break,
      };
      let selected = open == Some(&conv.g.id);
      if irc {
        out.push(self.buffer_row(ix, conv, selected, cx).into_any_element());
        continue;
      }
      let marks = RowMarks { selected, pin: true };
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

  /// The row at the end of the list that shows or hides the spaces and group chats older than
  /// `Settings::list_window`: a muted centered line, in IRC a monospaced one between rules, as WeeChat's.
  fn older_row(&self, store: &Store, irc: bool) -> Stateful<Div> {
    let label = if store.list.show_older {
      "Hide older".to_string()
    } else {
      format!("Show {} older", store.list.older.len())
    };
    let handle = self.store.clone();
    let line = h_flex()
      .size_full()
      .justify_center()
      .items_center()
      .text_size(px(13.))
      .text_color(rgb(MUTED))
      .hover(|s| s.bg(rgb(ROW_HOVER)));
    let row = div()
      .id("older")
      .w_full()
      .cursor_pointer()
      .on_click(move |_, _, cx| handle.update(cx, Store::toggle_older));
    if irc {
      row
        .h(px(BUFFER_ROW_HEIGHT))
        .child(line.px_3().font_family(mono()).child(format!("── {label} ──")))
    } else {
      row
        .h(px(LIST_ROW_HEIGHT))
        .px_2()
        .py_0p5()
        .child(line.rounded(px(10.)).child(label))
    }
  }

  /// IRC: a conversation as one line of its buffer name, after `@` while pinned, bold in the accent color while
  /// unread. A person's row ends with their status. A click opens it and a right-click shows its menu, as on the
  /// Messages row.
  fn buffer_row(&self, ix: usize, conv: &Conv, selected: bool, cx: &App) -> Stateful<Div> {
    let g = &conv.g;
    let opened = g.id.clone();
    let right_click = conversation_menu(&self.store, g.id.clone());
    let handle = self.store.clone();
    h_flex()
      .id(("conv", ix))
      .h(px(BUFFER_ROW_HEIGHT))
      .w_full()
      .px_3()
      .gap_2()
      .items_center()
      .cursor_pointer()
      .font_family(mono())
      .text_size(px(13.))
      .when(selected, |row| row.bg(rgb(ROW_SELECTED)))
      .when(!selected, |row| row.hover(|s| s.bg(rgb(ROW_HOVER))))
      .on_click(move |_, window, cx| {
        let group = opened.clone();
        handle.update(cx, |s, cx| s.open(group, window, cx));
      })
      .on_mouse_down(MouseButton::Right, right_click)
      .child(
        div()
          .flex_1()
          .min_w_0()
          .truncate()
          .text_color(rgb(if g.unread {
            ACCENT
          } else if selected {
            TEXT
          } else {
            MUTED
          }))
          .when(g.unread, |d| d.font_weight(FontWeight::BOLD))
          .child(emoji_font::text(if g.pinned {
            format!("@{}", buffer_name(g))
          } else {
            buffer_name(g)
          })),
      )
      .when_some(person_status(g, cx), |row, status| row.child(status_dot(status)))
  }

  /// Messages: the title with buttons for Mentions, Starred, Calendar, Meet, Mail (with its unread count), and New
  /// chat, then the filter chips. A column too narrow for both drops the title and makes the buttons smaller.
  /// IRC: one line, the filter in use (its menu also holds the two orders) and then the buttons, smaller.
  fn messages_header(&self, store: &Store, cx: &Context<Self>) -> Div {
    let shown = match &store.view {
      View::Shortcut(shortcut) => Some(*shortcut),
      View::Home | View::Calendar | View::Meet | View::Mail | View::Conversation(_) => None,
    };
    let shortcut_button = |id: &'static str, glyph: Icon, name: &'static str, shortcut: Shortcut| {
      let handle = self.store.clone();
      header_button(id, glyph, shown == Some(shortcut))
        .tooltip(place_tip(name, shortcut.into()))
        .on_mouse_down(MouseButton::Right, menu::place_menu(&self.store, shortcut.into()))
        .on_click(move |_, _, cx| handle.update(cx, |s, cx| s.open_shortcut(shortcut, cx)))
    };
    let (show_mentions, show_starred, show_calendar, show_meet, show_mail) = {
      let settings = cx.global::<Settings>();
      (
        settings.show_mentions,
        settings.show_starred,
        settings.calendar,
        settings.meet,
        settings.mail,
      )
    };
    let irc = cx.global::<Settings>().layout == Layout::Irc;
    let narrow = !irc && cx.global::<Settings>().column_width(Layout::Messages) < TITLE_MIN_WIDTH;
    let small = irc || narrow;
    let fit = move |button: Stateful<Div>| if small { button.size(px(24.)) } else { button };
    let actions = h_flex()
      .when(small, gpui_kit::Styled::gap_0p5)
      .when(!small, gpui_kit::Styled::gap_1)
      .when(show_mentions, |actions| {
        actions.child(fit(shortcut_button("mentions", Icon::AtSign, "Mentions", Shortcut::Mentions)))
      })
      .when(show_starred, |actions| {
        actions.child(fit(shortcut_button("starred", Icon::Star, "Starred", Shortcut::Starred)))
      })
      .when(show_calendar, |actions| {
        actions.child(fit(
          header_button("calendar", Icon::Calendar, store.view == View::Calendar)
            .tooltip(place_tip("Calendar", Place::Calendar))
            .on_mouse_down(MouseButton::Right, menu::place_menu(&self.store, Place::Calendar))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenCalendar), cx)),
        ))
      })
      .when(show_meet, |actions| {
        actions.child(fit(
          header_button("meet", Icon::Video, store.view == View::Meet)
            .tooltip(place_tip("Meet", Place::Meet))
            .on_mouse_down(MouseButton::Right, menu::place_menu(&self.store, Place::Meet))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenMeet), cx)),
        ))
      })
      .when(show_mail, |actions| {
        let unread = store.mail.unread as usize;
        actions.child(fit(
          header_button("mail", Icon::Mail, store.view == View::Mail)
            .relative()
            .tooltip(place_tip("Mail", Place::Mail))
            .on_mouse_down(MouseButton::Right, menu::place_menu(&self.store, Place::Mail))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenMail), cx))
            .when(unread > 0, |button| {
              button.child(count_badge(unread).absolute().top(px(-4.)).right(px(-6.)))
            }),
        ))
      })
      .when(!irc, |actions| actions.child(fit(sort_button("sort-list", cx))))
      .child(fit(
        header_button("new-chat", Icon::Plus, false)
          .tooltip(new_chat_tip)
          .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenNewChat), cx)),
      ));
    if irc {
      return h_flex()
        .h(px(32.))
        .flex_shrink_0()
        .pl_2()
        .pr_1()
        .gap_1()
        .items_center()
        .child(self.filter_line(store, cx))
        .child(actions.flex_shrink_0());
    }
    let title = h_flex()
      .h(px(52.))
      .items_center()
      .gap_2()
      .when(narrow, gpui_kit::Styled::px_3)
      .when(!narrow, |title| {
        title.px_5().child(
          div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_size(px(22.))
            .text_color(rgb(TEXT))
            .child("Chats"),
        )
      });
    v_flex()
      .flex_shrink_0()
      .child(title.child(actions.flex_shrink_0().items_center()))
      .child(self.chips(store))
  }

  /// The filters: All and Unread, then the kinds of conversation the account has. They fit one line at the default
  /// width, and wrap onto more when the list is dragged narrower.
  fn chips(&self, store: &Store) -> Div {
    let mut chips = chip_row();
    for (ix, pane) in chip_panes(store).into_iter().enumerate() {
      let chip = chip_button(ix, chip_label(&pane), chip_on(store, &pane));
      let handle = self.store.clone();
      chips = chips.child(chip.on_click(move |_, _, cx| Self::pick(&handle, pane.clone(), cx)));
    }
    chips
  }

  /// IRC: the filter in use on one line over the buffers. A click opens the menu of the filters the chips offer, the
  /// one in use checked, and of the two orders.
  fn filter_line(&self, store: &Store, cx: &App) -> Stateful<Div> {
    let choices: Vec<(Pane, bool)> = chip_panes(store)
      .into_iter()
      .map(|pane| {
        let on = chip_on(store, &pane);
        (pane, on)
      })
      .collect();
    let current = choices
      .iter()
      .find(|(_, on)| *on)
      .map_or("All", |(pane, _)| chip_label(pane))
      .to_string();
    let sort = cx.global::<Settings>().list_sort;
    let handle = self.store.clone();
    h_flex()
      .id("buffer-filter")
      .h(px(BUFFER_ROW_HEIGHT))
      .flex_1()
      .min_w_0()
      .px_1()
      .gap_1()
      .items_center()
      .rounded(px(4.))
      .cursor_pointer()
      .font_family(mono())
      .text_size(px(12.))
      .text_color(rgb(MUTED))
      .hover(|s| s.bg(rgb(ROW_HOVER)))
      .child(div().min_w_0().truncate().child(current))
      .child(icon(Icon::ChevronDown, 12., MUTED))
      .on_mouse_down(MouseButton::Left, move |event, window, cx| {
        cx.stop_propagation();
        let mut items: Vec<menu::MenuItem> = choices
          .iter()
          .map(|(pane, on)| {
            let (handle, picked) = (handle.clone(), pane.clone());
            menu::MenuItem::new(chip_label(pane).to_string(), move |_, cx| {
              Self::pick(&handle, picked.clone(), cx);
            })
            .checked(*on)
          })
          .collect();
        let mut orders =
          menu::sort_items(sort, |sort, cx| Settings::update(cx, |settings| settings.list_sort = sort)).into_iter();
        if let Some(first) = orders.next() {
          items.push(first.separated());
        }
        items.extend(orders);
        menu::show(event.position, items, window, cx);
      })
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

/// A conversation's name as IRC writes it: a space as `#name`, a group DM as `&name`, an app as `+name`, a DM as its
/// plain name.
pub fn buffer_name(g: &Group) -> String {
  match g.kind {
    GroupKind::Space => format!("#{}", g.name),
    GroupKind::GroupDm => format!("&{}", g.name),
    GroupKind::App => format!("+{}", g.name),
    GroupKind::Dm => g.name.clone(),
  }
}

/// The tooltip of a New chat button: its name and keyboard shortcut.
pub fn new_chat_tip(window: &mut Window, cx: &mut App) -> AnyView {
  Tooltip::new("New chat").action(&OpenNewChat, None).build(window, cx)
}

/// The tooltip of a view's button: its name, and its number key while it has one.
pub fn place_tip(name: &'static str, place: Place) -> impl Fn(&mut Window, &mut App) -> AnyView {
  move |window, cx| {
    let action = cx.global::<Settings>().hotkey_of(&place).and_then(keys::hotkey_action);
    let tip = Tooltip::new(name);
    match action {
      Some(action) => tip.action(&*action, None),
      None => tip,
    }
    .build(window, cx)
  }
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
    store.list.rows()
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

/// The name on a filter's chip. Short, so the chips keep to one line at the list's default width.
fn chip_label(pane: &Pane) -> &str {
  match pane {
    Pane::All => "All",
    _ => pane_title(pane),
  }
}

/// The filter chips: on one line while they fit, wrapped onto more when they do not.
pub fn chip_row() -> Div {
  h_flex().w_full().flex_wrap().gap_1().px_4().pb_2()
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
      .w(px(cx.global::<Settings>().column_width(match cx.global::<Settings>().layout {
        Layout::Rail => Layout::Rail,
        Layout::Irc => Layout::Irc,
        Layout::GoogleChat | Layout::Messages => Layout::Messages,
      })))
      .h_full()
      .flex_shrink_0()
      .child(header)
      .child(body)
  }
}
