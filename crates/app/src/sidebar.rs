//! Fixed-width, virtualized sidebar: Home, Mentions, and Starred, then the Direct messages / Apps / Spaces sections.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use gchat_core::Shortcut;

use crate::menu;
use crate::new_chat::OpenNewChat;
use crate::settings::{Layout, Settings};
use crate::store::{Section, SidebarItem, Store, View};
use crate::theme::{ACCENT, MUTED, ROW_HOVER, ROW_SELECTED, TEXT, TEXT_DIM, TEXT_STRONG, rgb};
use crate::ui::{Icon, group_avatar, icon};

const ROW_HEIGHT: f32 = 30.0;

pub struct Sidebar {
  store: Entity<Store>,
  scroll: UniformListScrollHandle,
}

impl Sidebar {
  pub fn new(store: Entity<Store>) -> Self {
    Self {
      store,
      scroll: UniformListScrollHandle::new(),
    }
  }

  fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
    let store = self.store.read(cx);
    let selected_home = store.view == View::Home;
    let selected_shortcut = match &store.view {
      View::Shortcut(shortcut) => Some(*shortcut),
      View::Home | View::Conversation(_) => None,
    };
    let selected_group = match &store.view {
      View::Conversation(id) => Some(id.clone()),
      View::Home | View::Shortcut(_) => None,
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
                .on_click(move |_, _, cx| store_handle.update(cx, super::store::Store::go_home))
                .child(icon(Icon::House, 16., TEXT))
                .child(label("Home", false)),
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
                .on_click(move |_, _, cx| store_handle.update(cx, |s, cx| s.open_shortcut(shortcut, cx)))
                .child(icon(glyph, 16., TEXT))
                .child(label(name, false)),
            )
            .into_any_element()
        },
        SidebarItem::Header(section) => {
          let section = section.clone();
          let title = section.title().to_string();
          let collapsed = store.is_collapsed(&section);
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
            );
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
          let store_handle = self.store.clone();
          row
            .child(
              item_frame(selected)
                .on_click(move |_, window, cx| {
                  let id = id.clone();
                  store_handle.update(cx, |s, cx| s.open(id, window, cx));
                })
                .child(group_avatar(&store.images, cx, &conv.g, 20.))
                .child(div().flex_1().min_w_0().child(label(&conv.g.name, conv.g.unread).truncate()))
                .when(conv.g.pinned, |d| d.child(icon(Icon::Pin, 12., MUTED)))
                .when(conv.g.unread && !conv.g.pinned, |d| {
                  d.child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(rgb(ACCENT)))
                }),
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
fn label(text: &str, bold: bool) -> Div {
  div()
    .text_size(px(13.))
    .text_color(rgb(if bold { TEXT_STRONG } else { TEXT_DIM }))
    .when(bold, |d| d.font_weight(FontWeight::SEMIBOLD))
    .child(text.to_string())
}

impl Render for Sidebar {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let count = self.store.read(cx).sidebar.len();
    v_flex()
      .w(px(cx.global::<Settings>().column_width(Layout::GoogleChat)))
      .h_full()
      .flex_shrink_0()
      .child(
        div().px_2().pt_2().child(
          item_frame(false)
            .id("new-chat")
            .h(px(ROW_HEIGHT))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenNewChat), cx))
            .child(icon(Icon::Plus, 16., TEXT))
            .child(label("New chat", false)),
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
