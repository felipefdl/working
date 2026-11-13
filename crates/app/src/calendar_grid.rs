//! The calendar's views: hours down the side (Day, 4 days, Week), Month, Year, and Schedule.

use std::sync::Arc;

use chrono::{Datelike, NaiveDate, Timelike};
use gchat_core::{CalendarEvent, Micros};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::calendar::{CalendarView, TOOLBAR_HEIGHT};
use crate::calendar_layout::{
  Block, DAY_MINUTES, MINUTE, Mode, Run, add_months, agenda, busy_days, clock, clock_range, day_blocks, day_start,
  days, hour_label, is_banner, local, month_weeks, now_micros, row, today, whole,
};
use crate::root::TOP_BAR_HEIGHT;
use crate::theme::{ACCENT, BG, BORDER, MUTED, PANEL, RED, ROW_HOVER, TEXT, rgb};
use crate::ui::{Icon, icon};

/// Height of one hour in a day column.
const HOUR_HEIGHT: f32 = 48.0;
/// Width of the column of hour labels.
const GUTTER: f32 = 56.0;
/// Height of one lane of a row of days: an all-day bar, or a line of the Month view.
const LANE: f32 = 22.0;
/// Most lanes the all-day strip of the hours views grows to.
const STRIP_LANES: u16 = 4;
/// Height of the Month view's weekday names, and of the day number in each of its cells.
const WEEKDAYS_HEIGHT: f32 = 28.0;
const DAY_NUMBER: f32 = 28.0;
/// Minutes a click on an empty slot snaps to.
const SLOT_MINUTES: u16 = 30;

type Events = Arc<Vec<CalendarEvent>>;

/// The minute of the day at `y` pixels down a day column, on a slot boundary.
fn slot_at(y: f32) -> u16 {
  let minute = whole(y / HOUR_HEIGHT * 60.) / SLOT_MINUTES * SLOT_MINUTES;
  minute.min(DAY_MINUTES - SLOT_MINUTES)
}

impl CalendarView {
  /// The listener that opens an event's card where it was pressed.
  fn opens_card(
    events: &Events,
    index: usize,
    cx: &Context<Self>,
  ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    let events = events.clone();
    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
      cx.stop_propagation();
      if let Some(held) = events.get(index) {
        this.open_card(held, event.position, window, cx);
      }
    })
  }

  /// One event in a lane of a row of days: a bar for an all-day event, a dot with its time for a timed one.
  fn chip(&self, events: &Events, run: Run, now: Micros, cx: &Context<Self>) -> Option<Stateful<Div>> {
    let event = events.get(run.event)?;
    let look = self.look(event, now);
    let title = div()
      .flex_1()
      .min_w_0()
      .truncate()
      .when(look.struck, gpui_kit::Styled::line_through)
      .child(event.title.clone());
    let chip = h_flex()
      .id(("chip", run.event * 8 + usize::from(run.first)))
      .size_full()
      .px_1p5()
      .gap_1p5()
      .items_center()
      .overflow_hidden()
      .rounded(px(5.))
      .text_size(px(12.))
      .cursor_pointer()
      .when(look.past, |chip| chip.opacity(0.6))
      .on_mouse_down(MouseButton::Left, Self::opens_card(events, run.event, cx));
    Some(if run.banner {
      chip
        .when(run.open_start, |chip| chip.rounded_l(px(0.)))
        .when(run.open_end, |chip| chip.rounded_r(px(0.)))
        .when(look.filled, |chip| chip.bg(look.color))
        .when(!look.filled, |chip| chip.bg(rgb(PANEL)).border_1().border_color(look.color))
        .text_color(look.ink)
        .child(title)
    } else {
      let dot = div()
        .size(px(8.))
        .flex_shrink_0()
        .rounded_full()
        .when(look.filled, |dot| dot.bg(look.color))
        .when(!look.filled, |dot| dot.border_1().border_color(look.color));
      chip
        .hover(|style| style.bg(rgb(ROW_HOVER)))
        .child(dot)
        .child(div().flex_shrink_0().text_color(rgb(MUTED)).child(clock(event.start)))
        .child(title.text_color(rgb(if look.struck { MUTED } else { TEXT })))
    })
  }

  /// The runs of one row of days, each in its lane, `top` pixels below the row's top.
  fn runs(&self, events: &Events, runs: &[Run], columns: usize, top: f32, cx: &Context<Self>) -> Vec<AnyElement> {
    let now = now_micros();
    let count = f32::from(u8::try_from(columns).unwrap_or(u8::MAX).max(1));
    runs
      .iter()
      .filter_map(|run| {
        let chip = self.chip(events, *run, now, cx)?;
        let span = f32::from(run.last - run.first + 1);
        Some(
          div()
            .absolute()
            .top(px(f32::from(run.lane).mul_add(LANE, top)))
            .left(relative(f32::from(run.first) / count))
            .w(relative(span / count))
            .h(px(LANE - 2.))
            .px(px(2.))
            .child(chip)
            .into_any_element(),
        )
      })
      .collect()
  }

  /// `+2 more` under the days with more events than lanes. A click opens the day.
  fn more_labels(more: &[u16], days: &[NaiveDate], lanes: u16, top: f32, cx: &Context<Self>) -> Vec<AnyElement> {
    let count = f32::from(u8::try_from(days.len()).unwrap_or(u8::MAX).max(1));
    let lane = f32::from(lanes.saturating_sub(1));
    more
      .iter()
      .zip(days)
      .enumerate()
      .filter(|(_, (hidden, _))| **hidden > 0)
      .map(|(index, (hidden, day))| {
        let (day, column) = (*day, f32::from(u8::try_from(index).unwrap_or(u8::MAX)));
        div()
          .absolute()
          .top(px(lane.mul_add(LANE, top)))
          .left(relative(column / count))
          .w(relative(1. / count))
          .h(px(LANE - 2.))
          .px(px(2.))
          .child(
            div()
              .id(("more", index))
              .size_full()
              .px_1p5()
              .flex()
              .items_center()
              .rounded(px(5.))
              .text_size(px(12.))
              .font_weight(FontWeight::MEDIUM)
              .text_color(rgb(MUTED))
              .cursor_pointer()
              .hover(|style| style.bg(rgb(ROW_HOVER)))
              .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                  cx.stop_propagation();
                  this.open_day(day, cx);
                }),
              )
              .child(format!("+{hidden} more")),
          )
          .into_any_element()
      })
      .collect()
  }

  // Day, 4 days, and Week.

  /// A timed event in its day column.
  fn block(&self, events: &Events, block: Block, now: Micros, cx: &Context<Self>) -> Option<AnyElement> {
    let event = events.get(block.event)?;
    let look = self.look(event, now);
    let per_minute = HOUR_HEIGHT / 60.;
    let height = f32::from(block.end - block.start).mul_add(per_minute, -2.);
    let width = 1. / f32::from(block.columns);
    let last = block.column + 1 == block.columns;
    // A tall block puts the time under the title; a short one fits both on its one line.
    let tall = height >= 38.;
    let title = if tall {
      event.title.clone()
    } else {
      format!("{}, {}", event.title, clock(event.start))
    };
    let inner = v_flex()
      .id(("block", block.event))
      .size_full()
      .px_1p5()
      .py_0p5()
      .overflow_hidden()
      .rounded(px(6.))
      .text_size(px(12.))
      .line_height(px(16.))
      .text_color(look.ink)
      .cursor_pointer()
      .when(look.filled, |inner| inner.bg(look.color))
      .when(!look.filled, |inner| inner.bg(rgb(PANEL)).border_1().border_color(look.color))
      .when(look.past, |inner| inner.opacity(0.6))
      .on_mouse_down(MouseButton::Left, Self::opens_card(events, block.event, cx))
      .child(
        div()
          .truncate()
          .font_weight(FontWeight::MEDIUM)
          .when(look.struck, gpui_kit::Styled::line_through)
          .child(title),
      )
      .when(tall, |inner| {
        inner.child(
          div()
            .truncate()
            .text_size(px(11.))
            .opacity(0.85)
            .child(clock_range(event.start, event.end)),
        )
      });
    Some(
      div()
        .absolute()
        .top(px(f32::from(block.start) * per_minute))
        .left(relative(f32::from(block.column) * width))
        .w(relative(width))
        .h(px(height))
        .pl(px(1.))
        // Room at the column's right edge to click an empty slot beside a full-width event.
        .pr(px(if last { 10. } else { 1. }))
        .child(inner)
        .into_any_element(),
    )
  }

  /// One day's column of hours: the hour lines, its timed events, and the line that marks now.
  fn day_column(
    &self,
    index: usize,
    day: NaiveDate,
    events: &Events,
    now: Micros,
    cx: &Context<Self>,
  ) -> Stateful<Div> {
    let base = day_start(day);
    let lines = (0..24).map(|_| div().h(px(HOUR_HEIGHT)).flex_shrink_0().border_b_1().border_color(rgb(BORDER)));
    let mut column = div()
      .id(("day-column", index))
      .flex_1()
      .min_w_0()
      .h_full()
      .relative()
      .border_l_1()
      .border_color(rgb(BORDER))
      .on_mouse_down(
        MouseButton::Left,
        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
          let top = this.hours.bounds().origin.y + this.hours.offset().y;
          let minute = slot_at((event.position.y - top).as_f32());
          this.new_event(Some(base + i64::from(minute) * MINUTE), window, cx);
        }),
      )
      .child(v_flex().size_full().children(lines));
    for block in day_blocks(events, day) {
      column = column.children(self.block(events, block, now, cx));
    }
    if day == today() {
      let at = local(now);
      let y = f32::from(u16::try_from(at.hour() * 60 + at.minute()).unwrap_or(0)) * HOUR_HEIGHT / 60.;
      column = column
        .child(div().absolute().top(px(y - 1.)).left_0().right_0().h(px(2.)).bg(rgb(RED)))
        .child(
          div()
            .absolute()
            .top(px(y - 5.))
            .left(px(-5.))
            .size(px(10.))
            .rounded_full()
            .bg(rgb(RED)),
        );
    }
    column
  }

  /// The names and numbers of the days over their columns. A click opens the day.
  fn day_heads(days: &[NaiveDate], cx: &Context<Self>) -> Div {
    let now = today();
    let heads = days.iter().enumerate().map(|(index, day)| {
      let (date, is_today) = (*day, *day == now);
      v_flex()
        .id(("day-head", index))
        .flex_1()
        .min_w_0()
        .pt_2()
        .pb_1()
        .gap_0p5()
        .items_center()
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| this.open_day(date, cx)))
        .child(
          div()
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(if is_today { ACCENT } else { MUTED }))
            .child(day.format("%a").to_string().to_uppercase()),
        )
        .child(
          div()
            .size(px(36.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .text_size(px(20.))
            .when(is_today, |number| number.bg(rgb(ACCENT)).text_color(rgb(BG)))
            .when(!is_today, |number| {
              number.text_color(rgb(TEXT)).hover(|style| style.bg(rgb(ROW_HOVER)))
            })
            .child(day.day().to_string()),
        )
    });
    h_flex()
      .flex_shrink_0()
      .child(div().w(px(GUTTER)).flex_shrink_0())
      .children(heads)
  }

  pub(crate) fn hours_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
    let days = days(self.mode, self.anchor);
    let events = self.events.clone();
    let now = now_micros();
    if self.scroll_to_now {
      self.scroll_to_now = false;
      // Today: from an hour and a half before now. Another day: from the morning.
      let minute = if days.contains(&today()) {
        let at = local(now);
        u16::try_from(at.hour() * 60 + at.minute()).unwrap_or(0)
      } else {
        9 * 60
      };
      let top = f32::from(minute.saturating_sub(90)) * HOUR_HEIGHT / 60.;
      self.hours.set_offset(point(px(0.), px(-top)));
    }

    let strip = row(&events, &days, STRIP_LANES, false);
    let strip_height = f32::from(strip.lanes).mul_add(LANE, 6.);
    let all_day = h_flex()
      .flex_shrink_0()
      .border_b_1()
      .border_color(rgb(BORDER))
      .child(div().w(px(GUTTER)).flex_shrink_0())
      .child(
        div()
          .id("all-day")
          .flex_1()
          .min_w_0()
          .h(px(strip_height))
          .relative()
          .children(self.runs(&events, &strip.runs, days.len(), 3., cx))
          .children(Self::more_labels(&strip.more, &days, STRIP_LANES, 3., cx)),
      );

    let labels = (1..24_u8).map(|hour| {
      div()
        .absolute()
        .top(px(f32::from(hour).mul_add(HOUR_HEIGHT, -7.)))
        .right(px(8.))
        .text_size(px(10.))
        .text_color(rgb(MUTED))
        .child(hour_label(hour))
    });
    let columns: Vec<Stateful<Div>> = days
      .iter()
      .enumerate()
      .map(|(index, day)| self.day_column(index, *day, &events, now, cx))
      .collect();
    let grid = h_flex()
      .h(px(24. * HOUR_HEIGHT))
      .w_full()
      .child(div().w(px(GUTTER)).h_full().flex_shrink_0().relative().children(labels))
      .children(columns);
    v_flex()
      .size_full()
      .child(Self::day_heads(&days, cx))
      .child(all_day)
      .child(
        div()
          .id("calendar-hours")
          .flex_1()
          .min_h_0()
          .overflow_y_scroll()
          .track_scroll(&self.hours)
          .child(grid),
      )
      .into_any_element()
  }

  // Month.

  /// One week of the Month view: its seven cells, and its events in lanes over them.
  fn week_row(
    &self,
    index: usize,
    week: &[NaiveDate],
    lanes: u16,
    events: &Events,
    cx: &Context<Self>,
  ) -> Stateful<Div> {
    let (now, month) = (today(), self.anchor.month());
    let placed = row(events, week, lanes, true);
    let cells = week.iter().enumerate().map(|(column, day)| {
      let (date, is_today) = (*day, *day == now);
      let label = if day.day() == 1 {
        day.format("%b %-d").to_string()
      } else {
        day.day().to_string()
      };
      let number = div()
        .id(("month-number", column))
        .min_w(px(22.))
        .h(px(22.))
        .px_1p5()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .text_size(px(12.))
        .cursor_pointer()
        .when(is_today, |number| number.bg(rgb(ACCENT)).text_color(rgb(BG)))
        .when(!is_today, |number| {
          number
            .text_color(rgb(if day.month() == month { TEXT } else { MUTED }))
            .hover(|style| style.bg(rgb(ROW_HOVER)))
        })
        .on_mouse_down(
          MouseButton::Left,
          cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.open_day(date, cx);
          }),
        )
        .child(label);
      div()
        .id(("month-day", column))
        .flex_1()
        .min_w_0()
        .h_full()
        .when(column > 0, gpui_kit::Styled::border_l_1)
        .border_color(rgb(BORDER))
        .on_mouse_down(
          MouseButton::Left,
          cx.listener(move |this, _, window, cx| this.new_all_day(date, window, cx)),
        )
        .child(div().h(px(DAY_NUMBER)).flex().items_center().justify_center().child(number))
    });
    div()
      .id(("week", index))
      .flex_1()
      .min_h_0()
      .relative()
      .overflow_hidden()
      .border_t_1()
      .border_color(rgb(BORDER))
      .child(h_flex().size_full().children(cells))
      .children(self.runs(events, &placed.runs, week.len(), DAY_NUMBER, cx))
      .children(Self::more_labels(&placed.more, week, lanes, DAY_NUMBER, cx))
  }

  pub(crate) fn month_view(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
    let weeks = month_weeks(self.anchor);
    let events = self.events.clone();
    // The lanes a week has room for, from the height the window leaves its row.
    let chrome = TOP_BAR_HEIGHT + TOOLBAR_HEIGHT + WEEKDAYS_HEIGHT + 10.;
    let rows = f32::from(u8::try_from(weeks.len()).unwrap_or(6).max(1));
    let row_height = (window.viewport_size().height.as_f32() - chrome) / rows;
    let lanes = whole((row_height - DAY_NUMBER - 2.) / LANE).max(1);

    let weekdays = weeks.first().map(|week| {
      h_flex().h(px(WEEKDAYS_HEIGHT)).flex_shrink_0().children(week.iter().map(|day| {
        div()
          .flex_1()
          .flex()
          .justify_center()
          .text_size(px(11.))
          .font_weight(FontWeight::MEDIUM)
          .text_color(rgb(MUTED))
          .child(day.format("%a").to_string().to_uppercase())
      }))
    });
    let rows: Vec<Stateful<Div>> = weeks
      .iter()
      .enumerate()
      .map(|(index, week)| self.week_row(index, week, lanes, &events, cx))
      .collect();
    v_flex().size_full().children(weekdays).children(rows).into_any_element()
  }

  // Year.

  pub(crate) fn year_view(&self, cx: &mut Context<Self>) -> AnyElement {
    let first = NaiveDate::from_ymd_opt(self.anchor.year(), 1, 1).unwrap_or(self.anchor);
    let marks = busy_days(&self.events);
    let mut months = h_flex().flex_wrap().items_start().justify_center().gap_x_8().gap_y_4().p_6();
    for index in 0..12_u8 {
      let month = add_months(first, u32::from(index), true);
      let slot = usize::from(index) + 1;
      months = months.child(
        v_flex()
          .gap_1()
          .child(
            div()
              .id(("year-month", slot))
              .h(px(28.))
              .px_2()
              .flex()
              .items_center()
              .rounded(px(8.))
              .text_size(px(14.))
              .font_weight(FontWeight::MEDIUM)
              .text_color(rgb(TEXT))
              .cursor_pointer()
              .hover(|style| style.bg(rgb(ROW_HOVER)))
              .on_click(cx.listener(move |this, _, _, cx| {
                this.anchor = month;
                this.set_mode(Mode::Month, cx);
              }))
              .child(month.format("%B").to_string()),
          )
          .child(self.mini_month(month, slot, Some(&marks), cx)),
      );
    }
    div()
      .id("calendar-year")
      .size_full()
      .overflow_y_scroll()
      .child(months)
      .into_any_element()
  }

  // Schedule.

  /// One event's line in the Schedule view.
  fn agenda_event(&self, events: &Events, index: usize, now: Micros, cx: &Context<Self>) -> Option<Stateful<Div>> {
    let event = events.get(index)?;
    let look = self.look(event, now);
    let time = if is_banner(event) {
      "All day".to_string()
    } else {
      clock_range(event.start, event.end)
    };
    Some(
      h_flex()
        .id(("agenda-event", index))
        .h(px(32.))
        .px_2()
        .gap_3()
        .items_center()
        .rounded(px(8.))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(ROW_HOVER)))
        .when(look.past, |line| line.opacity(0.6))
        .on_mouse_down(MouseButton::Left, Self::opens_card(events, index, cx))
        .child(
          div()
            .size(px(10.))
            .flex_shrink_0()
            .rounded_full()
            .when(look.filled, |dot| dot.bg(look.color))
            .when(!look.filled, |dot| dot.border_2().border_color(look.color)),
        )
        .child(
          div()
            .w(px(124.))
            .flex_shrink_0()
            .text_size(px(12.))
            .text_color(rgb(MUTED))
            .child(time),
        )
        .child(
          div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_size(px(13.))
            .text_color(rgb(TEXT))
            .when(look.struck, gpui_kit::Styled::line_through)
            .child(event.title.clone()),
        )
        .when(event.meet_url.is_some(), |line| line.child(icon(Icon::Video, 14., MUTED))),
    )
  }

  pub(crate) fn schedule_view(&self, cx: &Context<Self>) -> AnyElement {
    let days = days(Mode::Schedule, self.anchor);
    let events = self.events.clone();
    let listed = agenda(&events, &days);
    if listed.is_empty() {
      let loaded = self.store.read(cx).calendar.loaded;
      return div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(MUTED))
        .child(if loaded {
          "Nothing is scheduled in these two months."
        } else {
          "Loading…"
        })
        .into_any_element();
    }
    let (now, this_day) = (now_micros(), today());
    let mut list = v_flex();
    for (index, (day, held)) in listed.into_iter().enumerate() {
      let is_today = day == this_day;
      let head = h_flex()
        .id("agenda-head")
        .w(px(108.))
        .h(px(32.))
        .flex_shrink_0()
        .gap_2()
        .items_center()
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| this.open_day(day, cx)))
        .child(
          div()
            .size(px(30.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .rounded_full()
            .text_size(px(17.))
            .when(is_today, |number| number.bg(rgb(ACCENT)).text_color(rgb(BG)))
            .when(!is_today, |number| number.text_color(rgb(TEXT)))
            .child(day.day().to_string()),
        )
        .child(
          div()
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(if is_today { ACCENT } else { MUTED }))
            .child(day.format("%b, %a").to_string().to_uppercase()),
        );
      let lines = held.into_iter().filter_map(|event| self.agenda_event(&events, event, now, cx));
      list = list.child(
        h_flex()
          .id(("agenda", index))
          .px_4()
          .py_1p5()
          .gap_2()
          .items_start()
          .border_b_1()
          .border_color(rgb(BORDER))
          .child(head)
          .child(v_flex().flex_1().min_w_0().children(lines)),
      );
    }
    div()
      .id("calendar-schedule")
      .size_full()
      .overflow_y_scroll()
      .child(list)
      .into_any_element()
  }
}
