//! Fixed-width, virtualized sidebar: New chat, Home, Mentions, Starred, Calendar, Meet, and Mail, then the Direct
//! messages / Apps / Spaces sections, each header with its unread count. It scrolls to the open conversation when that
//! changes.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use gchat_core::{GroupId, Shortcut};

use crate::calendar::OpenCalendar;
use crate::conv_list::{key_hint, new_chat_tip, place_tip};
use crate::home::conversation_menu;
use crate::keys;
use crate::mail::OpenMail;
use crate::meet::OpenMeet;
use crate::menu;
use crate::new_chat::OpenNewChat;
use crate::settings::{Layout, Place, Settings};
use crate::store::{Section, SidebarItem, Store, View, shows_unread};
use crate::theme::{MUTED, ROW_HOVER, ROW_SELECTED, TEXT, TEXT_DIM, TEXT_STRONG, rgb};
use crate::ui::{Icon, count_badge, group_avatar, icon, muted_mark, unread_mark};

const ROW_HEIGHT: f32 = 30.0;

/// The Google Chat layout's column. The window root draws it as a cached view, so it redraws itself whenever what it
/// draws changes: the store, the settings, or an image arriving.
pub struct Sidebar {
  store: Entity<Store>,
  scroll: UniformListScrollHandle,
  /// The open conversation the list last scrolled to.
  followed: Option<GroupId>,
  _subs: [Subscription; 3],
}

impl Sidebar {
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
      followed: None,
      _subs: subs,
    }
  }

  /// Scrolls to the open conversation when it changes, so stepping with the keys or the switcher keeps it in view.
  /// A conversation without a row, its section folded, scrolls nothing.
  fn follow_open(&mut self, store: &Store) {
    let open = match &store.view {
      View::Conversation(id) => Some(id),
      View::Home | View::Shortcut(_) | View::Calendar | View::Meet | View::Mail => None,
    };
    if open == self.followed.as_ref() {
      return;
    }
    let row = open.and_then(|id| {
      store
        .sidebar
        .iter()
        .position(|item| matches!(item, SidebarItem::Conv(conv) if conv.g.id == *id))
    });
    if let Some(ix) = row {
      self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
    }
    self.followed = open.cloned();
  }

  fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
    let store = self.store.read(cx);
    let selected_home = store.view == View::Home;
    let selected_shortcut = match &store.view {
      View::Shortcut(shortcut) => Some(*shortcut),
      View::Home | View::Calendar | View::Meet | View::Mail | View::Conversation(_) => None,
    };
    let selected_calendar = store.view == View::Calendar;
    let selected_meet = store.view == View::Meet;
    let selected_mail = store.view == View::Mail;
    let selected_group = match &store.view {
      View::Conversation(id) => Some(id.clone()),
      View::Home | View::Shortcut(_) | View::Calendar | View::Meet | View::Mail => None,
    };
    let mut out = Vec::with_capacity(range.len());
    for ix in range {
      let Some(item) = store.sidebar.get(ix) else {
        break;
      };
      let row = div().id(("sb", ix)).h(px(ROW_HEIGHT)).w_full().px_2();
      let element = match item {
        SidebarItem::Home => {
          let store_handle = self.store.clone();
          row
            .child(
              item_frame(selected_home)
                .tooltip(place_tip("Home", Place::Home))
                .on_mouse_down(MouseButton::Right, menu::place_menu(&self.store, Place::Home))
                .on_click(move |_, _, cx| store_handle.update(cx, super::store::Store::go_home))
                .child(icon(Icon::House, 16., TEXT))
                .child(label("Home", false).flex_1())
                .children(key_hint(&Place::Home, cx)),
            )
            .into_any_element()
        },
        SidebarItem::Shortcut(shortcut) => {
          let shortcut = *shortcut;
          let (glyph, name) = match shortcut {
            Shortcut::Mentions => (Icon::AtSign, "Mentions"),
            Shortcut::Starred => (Icon::Star, "Starred"),
          };
          let store_handle = self.store.clone();
          row
            .child(
              item_frame(selected_shortcut == Some(shortcut))
                .tooltip(place_tip(name, shortcut.into()))
                .on_mouse_down(MouseButton::Right, menu::place_menu(&self.store, shortcut.into()))
                .on_click(move |_, _, cx| store_handle.update(cx, |s, cx| s.open_shortcut(shortcut, cx)))
                .child(icon(glyph, 16., TEXT))
                .child(label(name, false).flex_1())
                .children(key_hint(&shortcut.into(), cx)),
            )
            .into_any_element()
        },
        SidebarItem::Calendar => row
          .child(
            item_frame(selected_calendar)
              .tooltip(place_tip("Calendar", Place::Calendar))
              .on_mouse_down(MouseButton::Right, menu::place_menu(&self.store, Place::Calendar))
              .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenCalendar), cx))
              .child(icon(Icon::Calendar, 16., TEXT))
              .child(label("Calendar", false).flex_1())
              .children(key_hint(&Place::Calendar, cx)),
          )
          .into_any_element(),
        SidebarItem::Meet => row
          .child(
            item_frame(selected_meet)
              .tooltip(place_tip("Meet", Place::Meet))
              .on_mouse_down(MouseButton::Right, menu::place_menu(&self.store, Place::Meet))
              .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenMeet), cx))
              .child(icon(Icon::Video, 16., TEXT))
              .child(label("Meet", false).flex_1())
              .children(key_hint(&Place::Meet, cx)),
          )
          .into_any_element(),
        // Bright and bold while the inbox has unread threads, with their number at the right.
        SidebarItem::Mail => {
          let unread = store.mail.unread as usize;
          row
            .child(
              item_frame(selected_mail)
                .tooltip(place_tip("Mail", Place::Mail))
                .on_mouse_down(MouseButton::Right, menu::place_menu(&self.store, Place::Mail))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenMail), cx))
                .child(icon(Icon::Mail, 16., TEXT))
                .child(label("Mail", unread > 0).flex_1())
                .when(unread > 0, |d| d.child(count_badge(unread).flex_shrink_0()))
                .children(key_hint(&Place::Mail, cx)),
            )
            .into_any_element()
        },
        SidebarItem::Header(section) => {
          let section = section.clone();
          let title = section.title().to_string();
          let collapsed = store.is_collapsed(&section);
          // Shortcuts holds no conversations to count.
          let unread = if section == Section::Shortcuts {
            0
          } else {
            store.section_unread(&section)
          };
          let store_handle = self.store.clone();
          // Shortcuts is not a list of conversations: it has no order to pick.
          let sort_menu: Option<Rc<dyn Fn(Point<Pixels>, &mut Window, &mut App)>> = if section == Section::Shortcuts {
            None
          } else {
            let current = store.section_sort(&section);
            let key = section.key();
            Some(Rc::new(move |at: Point<Pixels>, window: &mut Window, cx: &mut App| {
              let key = key.clone();
              let items = menu::sort_items(current, move |sort, cx| {
                Settings::update(cx, |settings| {
                  settings.section_sort.insert(key.clone(), sort);
                });
              });
              menu::show(at, items, window, cx);
            }))
          };
          let group = format!("hdr-group-{ix}");
          let header = h_flex()
            .id(("hdr", ix))
            .group(group.clone())
            .size_full()
            .px_2()
            .gap_1()
            .items_center()
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(ROW_HOVER)))
            .on_click(move |_, _, cx| store_handle.update(cx, |s, cx| s.toggle_collapsed(section.clone(), cx)))
            .child(icon(
              if collapsed {
                Icon::ChevronRight
              } else {
                Icon::ChevronDown
              },
              14.,
              MUTED,
            ))
            .child(
              div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(MUTED))
                .child(title),
            )
            .when(unread > 0, |header| header.child(count_badge(unread).flex_shrink_0()));
          let header = match sort_menu {
            Some(open) => {
              let right_click = open.clone();
              header
                .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                  cx.stop_propagation();
                  right_click(event.position, window, cx);
                })
                .child(
                  div()
                    .id(("hdr-more", ix))
                    .size(px(22.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .opacity(0.)
                    .group_hover(group, |s| s.opacity(1.))
                    .hover(|s| s.bg(rgb(ROW_SELECTED)))
                    .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                      cx.stop_propagation();
                      open(event.position, window, cx);
                    })
                    .child(icon(Icon::More, 14., MUTED)),
                )
            },
            None => header,
          };
          row.child(header).into_any_element()
        },
        SidebarItem::Conv(conv) => {
          let selected = selected_group.as_ref() == Some(&conv.g.id);
          let id = conv.g.id.clone();
          let name = conv.name.clone();
          let right_click = conversation_menu(&self.store, conv.g.id.clone());
          let store_handle = self.store.clone();
          row
            .child(
              item_frame(selected)
                .tooltip(move |window, cx| Tooltip::new(name.clone()).build(window, cx))
                .on_click(move |_, window, cx| {
                  let id = id.clone();
                  store_handle.update(cx, |s, cx| s.open(id, window, cx));
                })
                .on_mouse_down(MouseButton::Right, right_click)
                .child(group_avatar(&store.images, cx, &conv.g, 20.))
                .child(
                  div()
                    .flex_1()
                    .min_w_0()
                    .child(label(conv.name.clone(), shows_unread(&conv.g)).truncate()),
                )
                .children(key_hint(&Place::Conversation(conv.g.id.clone()), cx))
                .when(conv.g.muted, |d| d.child(muted_mark()))
                .when(conv.g.pinned, |d| d.child(icon(Icon::Pin, 12., MUTED)))
                .child(unread_mark(&conv.g)),
            )
            .into_any_element()
        },
        SidebarItem::Toggle(section) => {
          let section = section.clone();
          let expanded = store.is_expanded(&section);
          let store_handle = self.store.clone();
          row
            .child(
              item_frame(false)
                .on_click(move |_, _, cx| store_handle.update(cx, |s, cx| s.toggle_show_all(section.clone(), cx)))
                .child(div().size(px(20.)).flex().items_center().justify_center().child(icon(
                  if expanded {
                    Icon::ChevronUp
                  } else {
                    Icon::ChevronsUpDown
                  },
                  14.,
                  MUTED,
                )))
                .child(div().text_size(px(13.)).text_color(rgb(MUTED)).child(if expanded {
                  "Show less"
                } else {
                  "Show all"
                })),
            )
            .into_any_element()
        },
      };
      out.push(element);
    }
    out
  }
}

fn item_frame(selected: bool) -> Stateful<Div> {
  h_flex()
    .id("item")
    .size_full()
    .px_2()
    .gap_2()
    .items_center()
    .rounded(px(8.))
    .cursor_pointer()
    .when(selected, |d| d.bg(rgb(ROW_SELECTED)))
    .when(!selected, |d| d.hover(|s| s.bg(rgb(ROW_HOVER))))
}

/// A sidebar name: bright and bold when unread, dimmer otherwise, so unread rows stand out. Selection only
/// changes the row's background.
fn label(text: impl Into<SharedString>, bold: bool) -> Div {
  div()
    .text_size(px(13.))
    .text_color(rgb(if bold { TEXT_STRONG } else { TEXT_DIM }))
    .when(bold, |d| d.font_weight(FontWeight::SEMIBOLD))
    .child(text.into())
}

impl Render for Sidebar {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let store = self.store.read(cx);
    self.follow_open(store);
    let count = store.sidebar.len();
    v_flex()
      .w(px(cx.global::<Settings>().column_width(Layout::GoogleChat)))
      .h_full()
      .flex_shrink_0()
      .child(
        div().px_2().pt_2().child(
          item_frame(false)
            .id("new-chat")
            .h(px(ROW_HEIGHT))
            .tooltip(new_chat_tip)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenNewChat), cx))
            .child(icon(Icon::Plus, 16., TEXT))
            .child(label("New chat", false).flex_1())
            .child(
              div()
                .flex_shrink_0()
                .text_size(px(11.))
                .text_color(rgb(MUTED))
                .child(keys::text("secondary-n")),
            ),
        ),
      )
      .child(
        uniform_list(
          "sidebar",
          count,
          cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .w_full()
        .min_h_0(),
      )
  }
}
