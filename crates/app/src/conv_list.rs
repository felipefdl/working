//! The conversation list of the Messages, Rail and IRC layouts beside the open conversation. Messages lists chats
//! with previews under a header of view buttons, filtered with chips beside the sort button and the Unread toggle, or
//! in a column narrower than `TITLE_MIN_WIDTH` with IRC's one-line filter; Rail lists the pane its icon column picked;
//! IRC lists one line of the monospaced name a conversation, as WeeChat's buffer list, under a filter line, after rows
//! that open the views. Every layout shares one Unread filter (`Store::unread_only`), combined with the pane in use.
//! Each list scrolls to the open conversation when it changes, and IRC's also to the row of the open view.

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
use crate::store::{Conv, Kind, ListRow, Pane, Store, View, shows_mention, shows_unread};
use crate::theme::{ACCENT, ACCENT_BG, BORDER, FIELD, MUTED, ROW_HOVER, ROW_SELECTED, TEXT, mono, rgb};
use crate::ui::{Icon, count_badge, icon, muted_mark, person_status, status_dot, unread_mark};

/// A list row: the shared conversation row with a sliver of air above and below, so selected rows stand apart.
const LIST_ROW_HEIGHT: f32 = ROW_HEIGHT + 4.0;
/// Rows left below the visible ones when the next page of Mentions or Starred is asked for.
const PREFETCH_ROWS: usize = 8;
/// An IRC buffer row: one line of the monospaced name.
const BUFFER_ROW_HEIGHT: f32 = 24.0;
/// Narrowest the Messages list shows its title beside the header buttons.
const TITLE_MIN_WIDTH: f32 = 300.0;
/// Height of a filter chip, and of the sort and Unread buttons beside the Messages chips.
const CHIP_HEIGHT: f32 = 22.;

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

/// A view the IRC buffer list opens from its first rows, as the Messages header buttons do.
#[derive(Clone, Copy)]
enum ViewRow {
  Mentions,
  Starred,
  Calendar,
  Meet,
  Mail,
}

impl ViewRow {
  const ALL: [Self; 5] = [Self::Mentions, Self::Starred, Self::Calendar, Self::Meet, Self::Mail];

  /// The rows the settings show, in order: each under the switch of its Messages header button.
  fn shown(settings: &Settings) -> impl Iterator<Item = Self> + use<> {
    let on = [
      settings.show_mentions,
      settings.show_starred,
      settings.calendar,
      settings.meet,
      settings.mail,
    ];
    Self::ALL.into_iter().zip(on).filter_map(|(row, on)| on.then_some(row))
  }

  const fn name(self) -> &'static str {
    match self {
      Self::Mentions => "Mentions",
      Self::Starred => "Starred",
      Self::Calendar => "Calendar",
      Self::Meet => "Meet",
      Self::Mail => "Mail",
    }
  }

  const fn place(self) -> Place {
    match self {
      Self::Mentions => Place::Mentions,
      Self::Starred => Place::Starred,
      Self::Calendar => Place::Calendar,
      Self::Meet => Place::Meet,
      Self::Mail => Place::Mail,
    }
  }

  /// Whether `view` is this row's view.
  const fn is_open(self, view: &View) -> bool {
    match self {
      Self::Mentions => matches!(view, View::Shortcut(Shortcut::Mentions)),
      Self::Starred => matches!(view, View::Shortcut(Shortcut::Starred)),
      Self::Calendar => matches!(view, View::Calendar),
      Self::Meet => matches!(view, View::Meet),
      Self::Mail => matches!(view, View::Mail),
    }
  }

  /// Opens the view, as its Messages header button does.
  fn open(self, store: &Entity<Store>, window: &mut Window, cx: &mut App) {
    match self {
      Self::Mentions => store.update(cx, |s, cx| s.open_shortcut(Shortcut::Mentions, cx)),
      Self::Starred => store.update(cx, |s, cx| s.open_shortcut(Shortcut::Starred, cx)),
      Self::Calendar => window.dispatch_action(Box::new(OpenCalendar), cx),
      Self::Meet => window.dispatch_action(Box::new(OpenMeet), cx),
      Self::Mail => window.dispatch_action(Box::new(OpenMail), cx),
    }
  }
}

/// The window root draws it as a cached view, so it redraws itself whenever what it draws changes: the store, the
/// settings, an image arriving, or the minute its rows' times count.
pub struct ConvList {
  store: Entity<Store>,
  scroll: UniformListScrollHandle,
  filter: Filter,
  /// The view the list last scrolled to.
  followed: Option<View>,
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

  /// Lists another pane. The Unread filter stays as it is.
  fn pick(store: &Entity<Store>, pane: Pane, cx: &mut App) {
    store.update(cx, |store, cx| store.set_pane(pane, cx));
  }

  /// Turns the Unread filter on or off. The pane stays as it is.
  fn set_only_unread(&mut self, on: bool, cx: &mut Context<Self>) {
    self.store.update(cx, |store, cx| store.set_unread_only(on, cx));
  }

  /// A new filter, whoever set it, lists from the first row.
  fn follow_filter(&mut self, store: &Store) {
    if self.filter.matches(store) {
      return;
    }
    self.filter = Filter::new(store);
    self.scroll.scroll_to_item_strict(0, ScrollStrategy::Top);
  }

  /// Scrolls to the open view's row when the view changes, so a number key, stepping with the keys, or the switcher
  /// keeps it in view. `views` are the view rows before the conversations (IRC; none in the other layouts).
  fn follow_open(&mut self, store: &Store, views: impl Iterator<Item = ViewRow>) {
    if self.followed.as_ref() == Some(&store.view) {
      return;
    }
    if let Some(ix) = open_row(&store.view, views, |id| store.list.position(id)) {
      self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
    }
    self.followed = Some(store.view.clone());
  }

  fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
    let store = self.store.read(cx);
    if !matches!(store.pane, Pane::Shortcut(_)) {
      let settings = cx.global::<Settings>();
      if settings.layout != Layout::Irc {
        return self.conversation_rows(range, store, cx);
      }
      // IRC: the view rows come first, then the conversations.
      let views = ViewRow::shown(settings).count();
      let mut out = Vec::with_capacity(range.len());
      for (ix, view) in ViewRow::shown(settings).enumerate().take(range.end).skip(range.start) {
        out.push(self.view_row(ix, view, ix + 1 == views, store, cx).into_any_element());
      }
      out.extend(self.conversation_rows(range.start.saturating_sub(views)..range.end.saturating_sub(views), store, cx));
      return out;
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

  /// The rows `range` of `Store::list`, by their index there.
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

  /// IRC: a view as one line of its name, with its number key and, for Mail, the unread count (the name then bold in
  /// the accent color, as an unread buffer). The open view's row is painted selected. A click opens the view and a
  /// right-click shows its place menu, as its Messages header button. The `last` view row ends with a rule, so the
  /// views read apart from the buffers.
  fn view_row(&self, ix: usize, view: ViewRow, last: bool, store: &Store, cx: &App) -> Stateful<Div> {
    let open = view.is_open(&store.view);
    let unread = if matches!(view, ViewRow::Mail) {
      store.mail.unread as usize
    } else {
      0
    };
    let handle = self.store.clone();
    h_flex()
      .id(("view", ix))
      .h(px(BUFFER_ROW_HEIGHT))
      .w_full()
      .px_3()
      .gap_2()
      .items_center()
      .cursor_pointer()
      .font_family(mono())
      .text_size(px(13.))
      .when(last, |row| row.border_b_1().border_color(rgb(BORDER)))
      .when(open, |row| row.bg(rgb(ROW_SELECTED)))
      .when(!open, |row| row.hover(|s| s.bg(rgb(ROW_HOVER))))
      .tooltip(place_tip(view.name(), view.place()))
      .on_mouse_down(MouseButton::Right, menu::place_menu(&self.store, view.place()))
      .on_click(move |_, window, cx| view.open(&handle, window, cx))
      .child(
        div()
          .flex_1()
          .min_w_0()
          .truncate()
          .text_color(rgb(if unread > 0 {
            ACCENT
          } else if open {
            TEXT
          } else {
            MUTED
          }))
          .when(unread > 0, |d| d.font_weight(FontWeight::BOLD))
          .child(view.name()),
      )
      .children(key_hint(&view.place(), cx))
      .when(unread > 0, |row| row.child(count_badge(unread)))
  }

  /// IRC: a conversation as one line of its buffer name, after `@` while pinned, bold in the accent color while it
  /// shows as unread, with its whole name as the tooltip. After the name: the muted mark while muted, the mention mark
  /// while its unread activity mentions the user, and a person's status. A click opens it and a right-click shows its
  /// menu, as on the Messages row.
  fn buffer_row(&self, ix: usize, conv: &Conv, selected: bool, cx: &App) -> Stateful<Div> {
    let g = &conv.g;
    let unread = shows_unread(g);
    let name = SharedString::from(buffer_name(g));
    let label = if g.pinned {
      SharedString::from(format!("@{name}"))
    } else {
      name.clone()
    };
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
      .tooltip(move |window, cx| Tooltip::new(name.clone()).build(window, cx))
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
          .text_color(rgb(if unread {
            ACCENT
          } else if selected {
            TEXT
          } else {
            MUTED
          }))
          .when(unread, |d| d.font_weight(FontWeight::BOLD))
          .child(emoji_font::text(label)),
      )
      .when(g.muted, |row| row.child(muted_mark()))
      .when(shows_mention(g), |row| row.child(unread_mark(g)))
      .when_some(person_status(g, cx), |row, status| row.child(status_dot(status)))
  }

  /// Messages: the title with buttons for Mentions, Starred, Calendar, Meet, Mail (with its unread count), and New
  /// chat, then the filter chips with the sort button and the Unread toggle at their right. A column narrower than
  /// `TITLE_MIN_WIDTH` drops the title, makes the buttons smaller, and puts the one-line filter of IRC across the
  /// column in the place of the chips and the two buttons, in the interface font, so the header never wraps.
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
    let narrow = cx.global::<Settings>().column_width(Layout::Messages) < TITLE_MIN_WIDTH;
    let fit = move |button: Stateful<Div>| if narrow { button.size(px(24.)) } else { button };
    let actions = h_flex()
      .when(narrow, gpui_kit::Styled::gap_0p5)
      .when(!narrow, gpui_kit::Styled::gap_1)
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
      .child(fit(new_chat_button()));
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
    // Narrow: one line under the buttons in the place of the chips, so the header never wraps.
    let filters = if narrow {
      h_flex().w_full().px_3().pb_2().child(self.filter_line(store))
    } else {
      self.chip_row(store, cx)
    };
    v_flex()
      .flex_shrink_0()
      .child(title.child(actions.flex_shrink_0().items_center()))
      .child(filters)
  }

  /// Messages: the chips, wrapping onto more lines when the list is dragged narrower, and at their right, level with
  /// their first line, the sort button and the Unread toggle with the count of every unread conversation.
  fn chip_row(&self, store: &Store, cx: &Context<Self>) -> Div {
    let on = store.unread_only;
    let unread = store.pane_unread(&Pane::All);
    h_flex()
      .w_full()
      .px_4()
      .pb_2()
      .gap_2()
      .items_start()
      .child(chips(&self.store, store).flex_1().min_w_0())
      .child(
        h_flex()
          .flex_shrink_0()
          .gap_1()
          .items_center()
          .child(
            icon_button("sort-list", Icon::ArrowDownAZ, false, CHIP_HEIGHT, 14.)
              .tooltip(hint("Sort"))
              .on_mouse_down(MouseButton::Left, sort_menu(cx)),
          )
          .child(
            icon_button("unread-only", Icon::ListFilter, on, CHIP_HEIGHT, 14.)
              .relative()
              .tooltip(hint("Unread only"))
              .on_click(cx.listener(move |this, _, _, cx| this.set_only_unread(!on, cx)))
              .when(unread > 0, |button| {
                button.child(count_badge(unread).absolute().top(px(-4.)).right(px(-6.)))
              }),
          ),
      )
  }

  /// IRC: one line, the filter in use in the monospaced font (its menu also holds the Unread filter and the two
  /// orders) and the New chat button. The views open from the first rows of the buffer list.
  fn irc_header(&self, store: &Store) -> Div {
    h_flex()
      .h(px(32.))
      .flex_shrink_0()
      .pl_2()
      .pr_1()
      .gap_1()
      .items_center()
      .child(self.filter_line(store).font_family(mono()))
      .child(new_chat_button().size(px(24.)))
  }

  /// The filter in use on one line, for the IRC header and a narrow Messages header: the pane's chip label, and with
  /// the Unread filter on "Unread" for All or "{label}, unread" for a kind. A click opens the menu of the panes the
  /// chips offer (the one in use checked, a kind with unread conversations followed by their count), then the Unread
  /// filter, then the two orders. It fills the width it is given; the caller picks its font.
  fn filter_line(&self, store: &Store) -> Stateful<Div> {
    let label = chip_label(&store.pane);
    let current = match (&store.pane, store.unread_only) {
      (_, false) => label.to_string(),
      (Pane::All, true) => "Unread".to_string(),
      (Pane::Kind(_) | Pane::Section(_) | Pane::Shortcut(_), true) => format!("{label}, unread"),
    };
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
      .text_size(px(12.))
      .text_color(rgb(MUTED))
      .hover(|s| s.bg(rgb(ROW_HOVER)))
      .child(div().min_w_0().truncate().child(current))
      .child(icon(Icon::ChevronDown, 12., MUTED))
      .on_mouse_down(MouseButton::Left, move |event, window, cx| {
        cx.stop_propagation();
        let store = handle.read(cx);
        let mut items: Vec<menu::MenuItem> = chip_panes(store)
          .map(|pane| {
            let unread = match pane {
              Pane::All => 0,
              Pane::Kind(_) | Pane::Section(_) | Pane::Shortcut(_) => store.pane_unread(&pane),
            };
            let label = if unread > 0 {
              format!("{} ({unread})", chip_label(&pane))
            } else {
              chip_label(&pane).to_string()
            };
            let on = store.pane == pane;
            let picker = handle.clone();
            menu::MenuItem::new(label, move |_, cx| Self::pick(&picker, pane.clone(), cx)).checked(on)
          })
          .collect();
        let only = store.unread_only;
        let flipper = handle.clone();
        items.push(
          menu::MenuItem::new("Unread only", move |_, cx| {
            flipper.update(cx, |store, cx| store.set_unread_only(!only, cx));
          })
          .checked(only)
          .separated(),
        );
        let sort = cx.global::<Settings>().list_sort;
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
    let on = store.unread_only;
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

/// The row of `view` in a list of the `views` rows and then `Store::list`'s rows, where `position` finds a
/// conversation in `Store::list`: its view row, or the open conversation's row after the view rows. `None` when no
/// row shows it.
fn open_row(
  view: &View,
  mut views: impl Iterator<Item = ViewRow>,
  position: impl FnOnce(&GroupId) -> Option<usize>,
) -> Option<usize> {
  match view {
    View::Conversation(id) => position(id).map(|ix| views.count() + ix),
    View::Home | View::Shortcut(_) | View::Calendar | View::Meet | View::Mail => {
      views.position(|row| row.is_open(view))
    },
  }
}

/// The name of a pane: the title of the Rail list, and the tooltip of its button.
pub fn pane_title(pane: &Pane) -> &str {
  match pane {
    Pane::All => "All chats",
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

/// The number key that opens a place, dim, for the right end of its row, while it has one.
pub fn key_hint(place: &Place, cx: &App) -> Option<Div> {
  let keys = cx.global::<Settings>().hotkey_of(place).and_then(crate::keys::hotkey_keys)?;
  Some(
    div()
      .flex_shrink_0()
      .text_size(px(11.))
      .text_color(rgb(MUTED))
      .child(crate::keys::text(keys)),
  )
}

/// A tooltip naming a button.
pub fn hint(text: &'static str) -> impl Fn(&mut Window, &mut App) -> AnyView {
  move |window, cx| Tooltip::new(text).build(window, cx)
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
fn row_count(store: &Store) -> usize {
  if matches!(store.pane, Pane::Shortcut(_)) {
    shortcut_hits(store).map_or(0, <[SearchHit]>::len)
  } else {
    store.list.rows()
  }
}

/// What the list says when it has no rows.
fn empty_text(store: &Store) -> &'static str {
  match &store.pane {
    Pane::Shortcut(_) if shortcut_hits(store).is_none() => "Loading…",
    Pane::Shortcut(Shortcut::Mentions) => "No one has mentioned you yet.",
    Pane::Shortcut(Shortcut::Starred) => "Messages you star in Google Chat show here.",
    _ if !store.query.is_empty() => "No conversations match. Press Enter to search messages.",
    _ if store.unread_only => "No unread conversations",
    Pane::Section(_) | Pane::Kind(_) => "No conversations here",
    Pane::All => "No conversations yet",
  }
}

/// The kinds of conversation the account has, in the order of the chips.
pub fn kinds_present(store: &Store) -> impl Iterator<Item = Kind> + '_ {
  Kind::ALL
    .into_iter()
    .filter(|kind| store.tabs.iter().any(|tab| tab.pane == Pane::Kind(*kind)))
}

/// The panes the chips offer: All, then People, Groups, Spaces, and Apps when the account has them.
fn chip_panes(store: &Store) -> impl Iterator<Item = Pane> + '_ {
  std::iter::once(Pane::All).chain(kinds_present(store).map(Pane::Kind))
}

/// The name on a filter's chip. Short, so the chips keep to one line at the list's default width.
fn chip_label(pane: &Pane) -> &str {
  match pane {
    Pane::All => "All",
    Pane::Kind(_) | Pane::Section(_) | Pane::Shortcut(_) => pane_title(pane),
  }
}

/// The filter chips of Home and the Messages header, one exclusive pick of `Store::pane`: All, then each kind of
/// conversation the account has, the one in use painted. A kind with unread conversations carries an accent dot at
/// its chip's top-right corner, over the chip so its width never changes. They wrap onto more lines when the width
/// runs out; the caller sizes and places them.
pub fn chips(handle: &Entity<Store>, store: &Store) -> Div {
  let mut row = h_flex().flex_wrap().gap_1();
  for (ix, pane) in chip_panes(store).enumerate() {
    let unread = match pane {
      Pane::All => false,
      Pane::Kind(_) | Pane::Section(_) | Pane::Shortcut(_) => store.pane_unread(&pane) > 0,
    };
    let chip = chip_button(ix, chip_label(&pane), store.pane == pane, unread);
    let handle = handle.clone();
    row = row.child(chip.on_click(move |_, _, cx| ConvList::pick(&handle, pane.clone(), cx)));
  }
  row
}

/// One filter chip: `on` paints the one in use, `unread` adds the accent dot at its top-right corner.
fn chip_button(ix: usize, label: &str, on: bool, unread: bool) -> Stateful<Div> {
  div()
    .id(("chip", ix))
    .relative()
    .flex_shrink_0()
    .h(px(CHIP_HEIGHT))
    .px_2()
    .flex()
    .items_center()
    .rounded_full()
    .text_size(px(11.))
    .cursor_pointer()
    .bg(rgb(if on { ACCENT_BG } else { FIELD }))
    .text_color(rgb(if on { ACCENT } else { TEXT }))
    .when(!on, |chip| chip.hover(|s| s.bg(rgb(BORDER))))
    .child(label.to_string())
    .when(unread, |chip| {
      chip.child(
        div()
          .absolute()
          .top(px(-1.))
          .right(px(-1.))
          .size(px(6.))
          .rounded_full()
          .bg(rgb(ACCENT)),
      )
    })
}

/// A round icon button `size` across with a `glyph_size` glyph. `on` paints it as in use.
fn icon_button(id: &'static str, glyph: Icon, on: bool, size: f32, glyph_size: f32) -> Stateful<Div> {
  div()
    .id(id)
    .size(px(size))
    .flex()
    .flex_shrink_0()
    .items_center()
    .justify_center()
    .rounded_full()
    .cursor_pointer()
    .when(on, |button| button.bg(rgb(ACCENT_BG)))
    .when(!on, |button| button.hover(|s| s.bg(rgb(ROW_HOVER))))
    .child(icon(glyph, glyph_size, if on { ACCENT } else { MUTED }))
}

/// A round icon button of the Messages header. `on` paints the one whose view is open.
fn header_button(id: &'static str, glyph: Icon, on: bool) -> Stateful<Div> {
  icon_button(id, glyph, on, 30., 16.)
}

/// The New chat button of the Messages and IRC headers.
fn new_chat_button() -> Stateful<Div> {
  header_button("new-chat", Icon::Plus, false)
    .tooltip(new_chat_tip)
    .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenNewChat), cx))
}

/// What a press on a sort button does: opens the menu of the two orders, the one in `Settings::list_sort` checked,
/// and picking one writes it.
fn sort_menu(cx: &App) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
  let current = cx.global::<Settings>().list_sort;
  move |event, window, cx| {
    cx.stop_propagation();
    let items = menu::sort_items(current, |sort, cx| Settings::update(cx, |settings| settings.list_sort = sort));
    menu::show(event.position, items, window, cx);
  }
}

/// The sort button of a list header: opens the menu of the two orders, the one in `Settings::list_sort` checked, and
/// picking one writes it.
pub fn sort_button(id: &'static str, cx: &App) -> Stateful<Div> {
  header_button(id, Icon::ArrowDownAZ, false)
    .tooltip(hint("Sort"))
    .on_mouse_down(MouseButton::Left, sort_menu(cx))
}

impl Render for ConvList {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let settings = cx.global::<Settings>();
    let layout = settings.layout;
    let views = if layout == Layout::Irc {
      ViewRow::shown(settings).count()
    } else {
      0
    };
    let store = self.store.read(cx);
    self.follow_filter(store);
    self.follow_open(store, ViewRow::shown(settings).take(views));
    let header = match layout {
      Layout::Rail => self.rail_header(store, cx),
      Layout::Irc => self.irc_header(store),
      Layout::GoogleChat | Layout::Messages => self.messages_header(store, cx),
    };
    let count = row_count(store);
    let body = if count == 0 {
      // IRC keeps its view rows over the empty text.
      v_flex()
        .flex_1()
        .min_h_0()
        .children(
          ViewRow::shown(settings)
            .take(views)
            .enumerate()
            .map(|(ix, view)| self.view_row(ix, view, ix + 1 == views, store, cx)),
        )
        .child(
          div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .px_6()
            .text_center()
            .text_color(rgb(MUTED))
            .child(empty_text(store)),
        )
        .into_any_element()
    } else {
      div()
        .flex_1()
        .min_h_0()
        .child(
          uniform_list(
            "conv-list",
            views + count,
            cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
          )
          .track_scroll(&self.scroll)
          .size_full(),
        )
        .into_any_element()
    };
    v_flex()
      .w(px(settings.column_width(match layout {
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

#[cfg(test)]
mod tests {
  use gchat_core::{GroupId, Shortcut};

  use super::{ViewRow, open_row};
  use crate::store::View;

  fn jordan() -> GroupId {
    GroupId::Dm("jpark".into())
  }

  #[test]
  fn an_irc_view_is_found_among_the_view_rows_shown() {
    // Starred and Meet switched off.
    let shown = [ViewRow::Mentions, ViewRow::Calendar, ViewRow::Mail];
    let none = |_: &GroupId| None;
    assert_eq!(open_row(&View::Shortcut(Shortcut::Mentions), shown.into_iter(), none), Some(0));
    assert_eq!(open_row(&View::Calendar, shown.into_iter(), none), Some(1));
    assert_eq!(open_row(&View::Mail, shown.into_iter(), none), Some(2));
    assert_eq!(open_row(&View::Shortcut(Shortcut::Starred), shown.into_iter(), none), None);
    assert_eq!(open_row(&View::Meet, shown.into_iter(), none), None);
    assert_eq!(open_row(&View::Home, shown.into_iter(), none), None);
  }

  #[test]
  fn a_conversation_comes_after_the_view_rows() {
    let shown = [ViewRow::Mentions, ViewRow::Calendar, ViewRow::Mail];
    let open = View::Conversation(jordan());
    assert_eq!(open_row(&open, shown.into_iter(), |_| Some(4)), Some(7));
    assert_eq!(open_row(&open, shown.into_iter(), |_| None), None);
  }

  #[test]
  fn a_list_without_view_rows_follows_conversations_only() {
    let open = View::Conversation(jordan());
    assert_eq!(open_row(&open, std::iter::empty(), |_| Some(4)), Some(4));
    assert_eq!(open_row(&View::Mail, std::iter::empty(), |_| Some(4)), None);
  }
}
