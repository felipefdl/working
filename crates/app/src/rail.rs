//! The Rail layout's icon column: one button per pane (All, Mentions, Starred, then each section and kind) with its
//! unread count, and Calendar, Meet, Mail (with its unread count), and New chat at the bottom.

use gchat_core::Shortcut;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::calendar::OpenCalendar;
use crate::conv_list::{new_chat_tip, pane_title, place_tip};
use crate::mail::OpenMail;
use crate::meet::OpenMeet;
use crate::menu;
use crate::new_chat::OpenNewChat;
use crate::settings::{Place, Settings};
use crate::store::{Kind, Pane, Section, Store, Tab, View};
use crate::theme::{FIELD, MUTED, ROW_HOVER, ROW_SELECTED, TEXT, TEXT_STRONG, Tone, rgb};
use crate::ui::{Icon, count_badge, icon};
use crate::util;

/// Width of the column.
pub const RAIL_WIDTH: f32 = 68.0;
/// Width of one button, leaving a little room each side of it.
const TAB_WIDTH: f32 = 60.0;
/// Height of a pane button when the rail has room: its glyph and label with padding above and below.
const TAB_HEIGHT: f32 = 56.0;
/// The least a pane button shrinks to when the rail is short: its glyph, the gap, and one line of label, without
/// padding. Below that the list of panes scrolls instead.
const TAB_MIN_HEIGHT: f32 = 44.0;

/// The window root draws it as a cached view, so it redraws itself whenever the store or the settings change.
pub struct Rail {
  store: Entity<Store>,
  _subs: [Subscription; 2],
}

impl Rail {
  pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
    let subs = [
      cx.observe(&store, |_, _, cx| cx.notify()),
      cx.observe_global::<Settings>(|_, cx| cx.notify()),
    ];
    Self { store, _subs: subs }
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
    Pane::All | Pane::Section(Section::Dms) => Glyph::Icon(Icon::Thread),
    Pane::Section(Section::Shortcuts) => Glyph::Icon(Icon::House),
    Pane::Shortcut(Shortcut::Mentions) => Glyph::Icon(Icon::AtSign),
    Pane::Shortcut(Shortcut::Starred) => Glyph::Icon(Icon::Star),
    Pane::Kind(Kind::People) => Glyph::Icon(Icon::User),
    Pane::Kind(Kind::Groups) => Glyph::Icon(Icon::Users),
    Pane::Kind(Kind::Spaces) => Glyph::Letter('#'),
    Pane::Kind(Kind::Apps) => Glyph::Icon(Icon::Bot),
    Pane::Section(Section::Apps) => Glyph::Icon(Icon::Bot),
    Pane::Section(Section::Meetings) => Glyph::Icon(Icon::Video),
    Pane::Section(Section::Spaces) => Glyph::Letter('#'),
    Pane::Section(Section::Custom { name, .. }) => Glyph::Letter(util::initials(name).chars().next().unwrap_or('?')),
  }
}

/// The short name under a button's glyph. Its tooltip has the whole title.
fn label(pane: &Pane) -> &str {
  match pane {
    Pane::All => "All",
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
  count_badge(count).absolute().top(px(-5.)).right(px(-9.))
}

/// A pane's button. Mentions and Starred carry their number key in the tooltip and the place menu on a right click, as
/// their sidebar rows do; the others have their title as the tooltip.
fn tab_button(store: &Entity<Store>, ix: usize, tab: &Tab, selected: bool) -> Stateful<Div> {
  let color = if selected { TEXT_STRONG } else { MUTED };
  let (handle, pane) = (store.clone(), tab.pane.clone());
  let button = v_flex()
    .id(("rail", ix))
    .w(px(TAB_WIDTH))
    .h(px(TAB_HEIGHT))
    .min_h(px(TAB_MIN_HEIGHT))
    .justify_center()
    .items_center()
    .gap_1()
    .rounded(px(10.))
    .cursor_pointer()
    .when(selected, |button| button.bg(rgb(ROW_SELECTED)))
    .when(!selected, |button| button.hover(|s| s.bg(rgb(ROW_HOVER))))
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
    );
  match &tab.pane {
    Pane::Shortcut(shortcut) => {
      let name = match shortcut {
        Shortcut::Mentions => "Mentions",
        Shortcut::Starred => "Starred",
      };
      button
        .tooltip(place_tip(name, (*shortcut).into()))
        .on_mouse_down(MouseButton::Right, menu::place_menu(store, (*shortcut).into()))
    },
    Pane::All | Pane::Kind(_) | Pane::Section(_) => {
      let title = pane_title(&tab.pane).to_string();
      button.tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
    },
  }
}

/// A button under the panes for a view of its own, Calendar, Meet, or Mail: it changes what the panel shows, not which
/// conversations are listed. `unread` goes over its glyph, as on the panes, when there is any. It is as tall as a
/// pane button at its least (`TAB_MIN_HEIGHT`), so the panes above keep the room a short window has.
fn view_button(id: &'static str, glyph: Icon, name: &'static str, selected: bool, unread: usize) -> Stateful<Div> {
  let color = if selected { TEXT_STRONG } else { MUTED };
  v_flex()
    .id(id)
    .w(px(TAB_WIDTH))
    .h(px(TAB_MIN_HEIGHT))
    .flex_shrink_0()
    .justify_center()
    .items_center()
    .gap_1()
    .rounded(px(10.))
    .cursor_pointer()
    .when(selected, |button| button.bg(rgb(ROW_SELECTED)))
    .when(!selected, |button| button.hover(|s| s.bg(rgb(ROW_HOVER))))
    .child(
      div()
        .relative()
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(glyph, 20., color))
        .when(unread > 0, |holder| holder.child(badge(unread))),
    )
    .child(div().text_size(px(11.)).text_color(rgb(color)).child(name))
}

impl Render for Rail {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let (show_calendar, show_meet, show_mail) = {
      let settings = cx.global::<Settings>();
      (settings.calendar, settings.meet, settings.mail)
    };
    let store = self.store.read(cx);
    let tabs = store
      .tabs
      .iter()
      .enumerate()
      .map(|(ix, tab)| tab_button(&self.store, ix, tab, store.pane == tab.pane));
    v_flex()
      .w(px(RAIL_WIDTH))
      .h_full()
      .flex_shrink_0()
      .items_center()
      .gap_2()
      .pb_3()
      .child(
        v_flex()
          .id("rail-tabs")
          .flex_1()
          .min_h_0()
          .w_full()
          .items_center()
          // Inside the scrolling list, so the unread badge over the first button is not cut by its edge.
          .pt_2()
          .gap_1()
          .overflow_y_scroll()
          .children(tabs),
      )
      // The views, as close together as the panes.
      .child(
        v_flex()
          .flex_shrink_0()
          .items_center()
          .gap_1()
          .when(show_calendar, |views| {
            views.child(
              view_button("rail-calendar", Icon::Calendar, "Calendar", store.view == View::Calendar, 0)
                .tooltip(place_tip("Calendar", Place::Calendar))
                .on_mouse_down(MouseButton::Right, menu::place_menu(&self.store, Place::Calendar))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenCalendar), cx)),
            )
          })
          .when(show_meet, |views| {
            views.child(
              view_button("rail-meet", Icon::Video, "Meet", store.view == View::Meet, 0)
                .tooltip(place_tip("Meet", Place::Meet))
                .on_mouse_down(MouseButton::Right, menu::place_menu(&self.store, Place::Meet))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenMeet), cx)),
            )
          })
          .when(show_mail, |views| {
            views.child(
              view_button(
                "rail-mail",
                Icon::Mail,
                "Mail",
                store.view == View::Mail,
                store.mail.unread as usize,
              )
              .tooltip(place_tip("Mail", Place::Mail))
              .on_mouse_down(MouseButton::Right, menu::place_menu(&self.store, Place::Mail))
              .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenMail), cx)),
            )
          }),
      )
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
