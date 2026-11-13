//! What the Meet view shows under its toolbar: the Meetings tab (the day, its week, and its meetings), the Calls tab (the
//! people to call), and the card with the link of a meeting made for later, drawn over them.

use std::ops::Range;

use chrono::{Datelike, NaiveDate};
use gpui_kit::component::date_picker::DatePicker;
use gpui_kit::component::input::Input;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::calendar::round_button;
use crate::calendar_layout::today;
use crate::conv_list::hint;
use crate::meet::{MeetView, new_button, solid_button};
use crate::meet_logic::{empty_title, week_of};
use crate::theme::{ACCENT, ACCENT_BG, BG, BORDER, FIELD, MUTED, POPOVER, ROW_HOVER, ROW_SELECTED, TEXT, rgb};
use crate::ui::{Icon, avatar, icon};

/// Widest the day row and the list get, so that a wide window does not pull them apart.
const CONTENT_WIDTH: f32 = 880.0;
/// Widest the Calls tab gets.
const CALLS_WIDTH: f32 = 560.0;
/// The date and its calendar icon, which open the date picker.
const DATE_WIDTH: f32 = 132.0;
/// One day of the week strip.
const DAY_WIDTH: f32 = 38.0;
/// One person of the Calls list, with the air around its row.
const PERSON_HEIGHT: f32 = 56.0;
const CARD_WIDTH: f32 = 400.0;

impl MeetView {
  // The Meetings tab.

  pub(crate) fn meetings_body(&self, cx: &Context<Self>) -> Div {
    let loaded = self.store.read(cx).meet.loaded;
    // While the day is on its way, the toolbar says so and the list holds back: it is not empty yet.
    let below = if self.meetings.is_empty() {
      loaded.then(|| self.empty_state(cx).into_any_element())
    } else {
      Some(self.meeting_list().into_any_element())
    };
    div().size_full().flex().justify_center().child(
      v_flex()
        .size_full()
        .max_w(px(CONTENT_WIDTH))
        .child(self.day_row(cx))
        .children(below),
    )
  }

  /// The date on the left, with the date picker behind it, and the week on the right.
  fn day_row(&self, cx: &Context<Self>) -> Div {
    let week = week_of(self.day);
    h_flex()
      .w_full()
      .flex_shrink_0()
      .px_4()
      .py_3()
      .gap_0p5()
      .items_center()
      .child(
        div()
          .w(px(DATE_WIDTH))
          .flex_shrink_0()
          .child(DatePicker::new(&self.picker).appearance(false)),
      )
      .child(div().flex_1())
      .child(
        round_button("meet-week-previous", Icon::ChevronLeft)
          .tooltip(hint("Previous week"))
          .on_click(cx.listener(|this, _, window, cx| this.step_week(false, window, cx))),
      )
      .children(week.into_iter().enumerate().map(|(ix, day)| self.day_cell(ix, day, cx)))
      .child(
        round_button("meet-week-next", Icon::ChevronRight)
          .tooltip(hint("Next week"))
          .on_click(cx.listener(|this, _, window, cx| this.step_week(true, window, cx))),
      )
  }

  /// A day of the week strip: the weekday over the day number. The selected day is filled and today is ringed, its
  /// weekday in the accent color.
  fn day_cell(&self, ix: usize, day: NaiveDate, cx: &Context<Self>) -> Stateful<Div> {
    let (selected, now) = (day == self.day, day == today());
    v_flex()
      .id(("meet-day", ix))
      .w(px(DAY_WIDTH))
      .py_1()
      .gap_0p5()
      .flex_shrink_0()
      .items_center()
      .rounded(px(10.))
      .cursor_pointer()
      .when(!selected, |cell| cell.hover(|style| style.bg(rgb(ROW_HOVER))))
      .on_click(cx.listener(move |this, _, window, cx| this.go_to(day, window, cx)))
      .child(
        div()
          .text_size(px(11.))
          .text_color(rgb(if now { ACCENT } else { MUTED }))
          .when(now, |label| label.font_weight(FontWeight::MEDIUM))
          .child(day.format("%a").to_string()),
      )
      .child(
        div()
          .size(px(28.))
          .flex()
          .items_center()
          .justify_center()
          .rounded_full()
          .text_size(px(13.))
          .text_color(rgb(TEXT))
          .when(now && !selected, |number| {
            number.border_1().border_color(rgb(ACCENT)).text_color(rgb(ACCENT))
          })
          .when(selected, |number| number.bg(rgb(ACCENT)).text_color(rgb(BG)))
          .child(day.day().to_string()),
      )
  }

  /// The day's meetings, a row each: the time over the title. A click joins.
  fn meeting_list(&self) -> Stateful<Div> {
    let mut list = v_flex()
      .id("meet-list")
      .flex_1()
      .min_h_0()
      .px_4()
      .pb_4()
      .gap_1()
      .overflow_y_scroll();
    for (ix, meeting) in self.meetings.iter().enumerate() {
      let url = meeting.url.clone();
      list = list.child(
        h_flex()
          .id(("meeting", ix))
          .w_full()
          .flex_shrink_0()
          .min_h(px(60.))
          .px_3()
          .py_2()
          .gap_3()
          .items_center()
          .rounded(px(12.))
          .cursor_pointer()
          .hover(|style| style.bg(rgb(ROW_HOVER)))
          .tooltip(hint("Join the meeting"))
          .on_click(move |_, _, cx| crate::call::open(&url, cx))
          .child(
            div()
              .size(px(40.))
              .flex()
              .flex_shrink_0()
              .items_center()
              .justify_center()
              .rounded_full()
              .bg(rgb(ACCENT_BG))
              .child(icon(Icon::Video, 18., ACCENT)),
          )
          .child(
            v_flex()
              .flex_1()
              .min_w_0()
              .gap_0p5()
              .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(meeting.when.clone()))
              .child(
                div()
                  .truncate()
                  .text_size(px(15.))
                  .text_color(rgb(TEXT))
                  .child(meeting.title.clone()),
              ),
          ),
      );
    }
    list
  }

  /// What the list says when the day has no meetings, with the way to make one.
  fn empty_state(&self, cx: &Context<Self>) -> Div {
    let busy = self.store.read(cx).meet.creating.is_some();
    v_flex()
      .flex_1()
      .min_h_0()
      .px_6()
      .gap_2()
      .items_center()
      .justify_center()
      .text_center()
      .child(
        div()
          .text_size(px(20.))
          .text_color(rgb(TEXT))
          .child(empty_title(self.day, today())),
      )
      .child(
        div()
          .text_size(px(14.))
          .text_color(rgb(MUTED))
          .child("Schedule a meeting or enjoy the free time"),
      )
      .child(div().pt_2().child(new_button("meet-new-empty", cx.entity(), busy)))
  }

  // The Calls tab.

  pub(crate) fn calls_body(&self, cx: &Context<Self>) -> Div {
    let ready = self.picked.is_some() && self.store.read(cx).meet.calling.is_none();
    let field = h_flex()
      .flex_1()
      .min_w_0()
      .h(px(40.))
      .px_3()
      .gap_2()
      .items_center()
      .rounded_full()
      .bg(rgb(FIELD))
      .child(icon(Icon::Search, 15., MUTED))
      .child(div().flex_1().child(Input::new(&self.search).appearance(false).bordered(false)));
    let call = solid_button("meet-call", Icon::Phone, "Call", ready).when(ready, |button| {
      button.on_click(cx.listener(|this, _, _, cx| this.call_picked(cx)))
    });
    let people = if self.people.is_empty() {
      div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(MUTED))
        .child("No people match")
    } else {
      div().flex_1().min_h_0().child(
        uniform_list(
          "meet-people",
          self.people.len(),
          cx.processor(|this, visible: Range<usize>, _, cx| this.person_rows(visible, cx)),
        )
        .track_scroll(&self.people_scroll)
        .size_full(),
      )
    };
    div().size_full().flex().justify_center().child(
      v_flex()
        .size_full()
        .max_w(px(CALLS_WIDTH))
        .px_4()
        .py_4()
        .gap_3()
        .child(h_flex().gap_2().items_center().child(field).child(call))
        .child(people),
    )
  }

  /// The rows of the people list in `visible`: avatar, name, and email. A click picks the person, a second one calls.
  fn person_rows(&self, visible: Range<usize>, cx: &Context<Self>) -> Vec<AnyElement> {
    let images = self.store.read(cx).images.clone();
    let mut rows = Vec::with_capacity(visible.len());
    for ix in visible {
      let Some(user) = self.people.get(ix) else {
        break;
      };
      let picked = self.picked.as_deref() == Some(user.id.as_str());
      let id = user.id.clone();
      let row = h_flex()
        .id(("person", ix))
        .size_full()
        .px_3()
        .gap_3()
        .items_center()
        .rounded(px(10.))
        .cursor_pointer()
        .when(picked, |row| row.bg(rgb(ROW_SELECTED)))
        .when(!picked, |row| row.hover(|style| style.bg(rgb(ROW_HOVER))))
        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
          this.picked = Some(id.clone());
          if event.click_count() >= 2 {
            this.call_picked(cx);
          }
          cx.notify();
        }))
        .child(avatar(&images, cx, user.avatar_url.as_deref(), None, &user.name, 36.))
        .child(
          v_flex()
            .min_w_0()
            .child(
              div()
                .truncate()
                .text_size(px(14.))
                .text_color(rgb(TEXT))
                .child(user.name.clone()),
            )
            .when(!user.email.is_empty(), |details| {
              details.child(
                div()
                  .truncate()
                  .text_size(px(12.))
                  .text_color(rgb(MUTED))
                  .child(user.email.clone()),
              )
            }),
        );
      rows.push(
        div()
          .id(("meet-person", ix))
          .h(px(PERSON_HEIGHT))
          .w_full()
          .py_0p5()
          .child(row)
          .into_any_element(),
      );
    }
    rows
  }

  // The link card.

  /// The card over the view that shows the link of a meeting made for later: copy it, or join the meeting.
  pub(crate) fn link_card(&self, url: &str, cx: &Context<Self>) -> AnyElement {
    let (copy, join) = (url.to_string(), url.to_string());
    let shown = url.trim_start_matches("https://").to_string();
    let copy_glyph = if self.copied { Icon::Check } else { Icon::Copy };
    div()
      .absolute()
      .top_0()
      .left_0()
      .size_full()
      .flex()
      .items_center()
      .justify_center()
      .occlude()
      .on_mouse_down(
        MouseButton::Left,
        cx.listener(|this, _, window, cx| this.close_card(window, cx)),
      )
      .child(
        v_flex()
          .w(px(CARD_WIDTH))
          .p_4()
          .gap_3()
          .rounded(px(14.))
          .bg(rgb(POPOVER))
          .border_1()
          .border_color(rgb(BORDER))
          .shadow_lg()
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .child(
            h_flex()
              .items_center()
              .gap_2()
              .child(
                div()
                  .flex_1()
                  .text_size(px(16.))
                  .font_weight(FontWeight::MEDIUM)
                  .text_color(rgb(TEXT))
                  .child("Your meeting is ready"),
              )
              .child(
                round_button("meet-card-close", Icon::X)
                  .tooltip(hint("Close"))
                  .on_click(cx.listener(|this, _, window, cx| this.close_card(window, cx))),
              ),
          )
          .child(
            div()
              .text_size(px(13.))
              .text_color(rgb(MUTED))
              .child("Share this link with the people you want to meet with. Save it to use it later too."),
          )
          .child(
            h_flex()
              .h(px(40.))
              .pl_3()
              .pr_1()
              .gap_2()
              .items_center()
              .rounded(px(10.))
              .bg(rgb(FIELD))
              .child(
                div()
                  .flex_1()
                  .min_w_0()
                  .truncate()
                  .text_size(px(13.))
                  .text_color(rgb(TEXT))
                  .child(shown),
              )
              .child(
                round_button("meet-card-copy", copy_glyph)
                  .tooltip(hint("Copy the meeting link"))
                  .on_click(cx.listener(move |this, _, _, cx| this.copy_link(&copy, cx))),
              ),
          )
          .child(
            h_flex()
              .justify_end()
              .child(
                solid_button("meet-card-join", Icon::Video, "Join now", true).on_click(cx.listener(
                  move |this, _, window, cx| {
                    crate::call::open(&join, cx);
                    this.close_card(window, cx);
                  },
                )),
              ),
          ),
      )
      .into_any_element()
  }
}
