//! The Rail layout's icon column: one button per pane (Home, Mentions, Starred, then each section and kind) with its
//! unread count, and Calendar, Meet, and New chat at the bottom.

use gchat_core::Shortcut;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::calendar::OpenCalendar;
use crate::conv_list::{calendar_tip, meet_tip, new_chat_tip, pane_title};
use crate::meet::OpenMeet;
use crate::new_chat::OpenNewChat;
use crate::settings::Settings;
use crate::store::{Kind, Pane, Section, Store, Tab, View};
use crate::theme::{ACCENT, BG, FIELD, MUTED, ROW_HOVER, ROW_SELECTED, TEXT, TEXT_STRONG, Tone, rgb};
use crate::ui::{Icon, icon};
use crate::util;

/// Width of the column.
pub const RAIL_WIDTH: f32 = 68.0;
/// Width of one button, leaving a little room each side of it.
const TAB_WIDTH: f32 = 60.0;

pub struct Rail {
  store: Entity<Store>,
}

impl Rail {
  pub const fn new(store: Entity<Store>) -> Self {
    Self { store }
  }
}

/// What a button shows above its label.
enum Glyph {
  Icon(Icon),
  /// A letter on a tile: the first of a section made in Google Chat, or `#` for Spaces.
  Letter(char),
}

fn glyph(pane: &Pane) -> Glyph {
  match pane {
    Pane::All | Pane::Unread | Pane::Section(Section::Shortcuts) => Glyph::Icon(Icon::House),
    Pane::Shortcut(Shortcut::Mentions) => Glyph::Icon(Icon::AtSign),
    Pane::Shortcut(Shortcut::Starred) => Glyph::Icon(Icon::Star),
    Pane::Kind(Kind::People) => Glyph::Icon(Icon::User),
    Pane::Kind(Kind::Groups) => Glyph::Icon(Icon::Users),
    Pane::Kind(Kind::Spaces) => Glyph::Letter('#'),
    Pane::Kind(Kind::Apps) => Glyph::Icon(Icon::Bot),
    Pane::Section(Section::Dms) => Glyph::Icon(Icon::Thread),
    Pane::Section(Section::Apps) => Glyph::Icon(Icon::Bot),
    Pane::Section(Section::Meetings) => Glyph::Icon(Icon::Video),
    Pane::Section(Section::Spaces) => Glyph::Letter('#'),
    Pane::Section(Section::Custom { name, .. }) => Glyph::Letter(util::initials(name).chars().next().unwrap_or('?')),
  }
}

/// The short name under a button's glyph. Its tooltip has the whole title.
fn label(pane: &Pane) -> &str {
  match pane {
    Pane::All | Pane::Unread => "Home",
    Pane::Kind(kind) => kind.title(),
    Pane::Section(_) | Pane::Shortcut(_) => pane_title(pane),
  }
}

fn glyph_element(glyph: Glyph, color: Tone) -> AnyElement {
  match glyph {
    Glyph::Icon(name) => icon(name, 20., color).into_any_element(),
    Glyph::Letter(letter) => div()
      .size(px(22.))
      .flex()
      .items_center()
      .justify_center()
      .rounded(px(6.))
      .border_1()
      .border_color(rgb(color))
      .text_size(px(12.))
      .font_weight(FontWeight::SEMIBOLD)
      .text_color(rgb(color))
      .child(letter.to_string())
      .into_any_element(),
  }
}

/// The unread count over a button's glyph.
fn badge(count: usize) -> Div {
  let text = if count > 99 {
    "99+".to_string()
  } else {
    count.to_string()
  };
  div()
    .absolute()
    .top(px(-5.))
    .right(px(-9.))
    .min_w(px(16.))
    .h(px(16.))
    .px_1()
    .flex()
    .items_center()
    .justify_center()
    .rounded_full()
    .bg(rgb(ACCENT))
    .text_size(px(10.))
    .font_weight(FontWeight::SEMIBOLD)
    .text_color(rgb(BG))
    .child(text)
}

/// Whether a button is the pane in use. The Unread pane a Messages chip may have left is All narrowed to the unread.
fn is_picked(store: &Store, pane: &Pane) -> bool {
  store.pane == *pane || (*pane == Pane::All && store.pane == Pane::Unread)
}

fn tab_button(handle: &Entity<Store>, ix: usize, tab: &Tab, selected: bool) -> Stateful<Div> {
  let color = if selected { TEXT_STRONG } else { MUTED };
  let (handle, pane, title) = (handle.clone(), tab.pane.clone(), pane_title(&tab.pane).to_string());
  v_flex()
    .id(("rail", ix))
    .w(px(TAB_WIDTH))
    .py_1p5()
    .items_center()
    .gap_1()
    .rounded(px(10.))
    .cursor_pointer()
    .when(selected, |button| button.bg(rgb(ROW_SELECTED)))
    .when(!selected, |button| button.hover(|s| s.bg(rgb(ROW_HOVER))))
    .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
    .on_click(move |_, _, cx| {
      handle.update(cx, |s, cx| s.set_pane(pane.clone(), cx));
    })
    .child(
      div()
        .relative()
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .child(glyph_element(glyph(&tab.pane), color))
        .when(tab.unread > 0, |holder| holder.child(badge(tab.unread))),
    )
    .child(
      div()
        .max_w(px(TAB_WIDTH - 8.))
        .truncate()
        .text_size(px(11.))
        .text_color(rgb(color))
        .child(label(&tab.pane).to_string()),
    )
}

/// A button under the panes for a view of its own, Calendar or Meet: it changes what the panel shows, not which
/// conversations are listed.
fn view_button(id: &'static str, glyph: Icon, name: &'static str, selected: bool) -> Stateful<Div> {
  let color = if selected { TEXT_STRONG } else { MUTED };
  v_flex()
    .id(id)
    .w(px(TAB_WIDTH))
    .py_1p5()
    .flex_shrink_0()
    .items_center()
    .gap_1()
    .rounded(px(10.))
    .cursor_pointer()
    .when(selected, |button| button.bg(rgb(ROW_SELECTED)))
    .when(!selected, |button| button.hover(|s| s.bg(rgb(ROW_HOVER))))
    .child(
      div()
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(glyph, 20., color)),
    )
    .child(div().text_size(px(11.)).text_color(rgb(color)).child(name))
}

impl Render for Rail {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let (show_calendar, show_meet) = {
      let settings = cx.global::<Settings>();
      (settings.show_calendar, settings.show_meet)
    };
    let store = self.store.read(cx);
    let tabs = store
      .tabs
      .iter()
      .enumerate()
      .map(|(ix, tab)| tab_button(&self.store, ix, tab, is_picked(store, &tab.pane)));
    v_flex()
      .w(px(RAIL_WIDTH))
      .h_full()
      .flex_shrink_0()
      .items_center()
      .gap_2()
      .pt_2()
      .pb_3()
      .child(
        v_flex()
          .id("rail-tabs")
          .flex_1()
          .min_h_0()
          .w_full()
          .items_center()
          .gap_1()
          .overflow_y_scroll()
          .children(tabs),
      )
      .when(show_calendar, |rail| {
        rail.child(
          view_button("rail-calendar", Icon::Calendar, "Calendar", store.view == View::Calendar)
            .tooltip(calendar_tip)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenCalendar), cx)),
        )
      })
      .when(show_meet, |rail| {
        rail.child(
          view_button("rail-meet", Icon::Video, "Meet", store.view == View::Meet)
            .tooltip(meet_tip)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenMeet), cx)),
        )
      })
      .child(
        div()
          .id("rail-new-chat")
          .size(px(40.))
          .flex()
          .items_center()
          .justify_center()
          .rounded_full()
          .bg(rgb(FIELD))
          .cursor_pointer()
          .hover(|s| s.bg(rgb(ROW_SELECTED)))
          .tooltip(new_chat_tip)
          .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenNewChat), cx))
          .child(icon(Icon::Plus, 18., TEXT)),
      )
  }
}
