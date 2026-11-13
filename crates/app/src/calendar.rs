//! Google Calendar in the content panel: a toolbar, a side panel with a small month and the calendars, and the view
//! picked (Day, Week, Month, Year, Schedule, or 4 days). The views themselves are in `calendar_grid.rs`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, NaiveDate, Timelike};
use gchat_core::{Calendar, CalendarEvent, EditScope, Micros, Rsvp};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::calendar_layout::{
  HOUR, MINUTE, Mode, add_days, add_months, day_start, local, month_start, month_weeks, now_micros, range, step, title,
  today,
};
use crate::conv_list::hint;
use crate::event_card::{CardEvent, EventCard};
use crate::event_editor::{EditorEvent, EventEditor, Seed};
use crate::menu::{self, MenuItem};
use crate::settings::Settings;
use crate::store::{Store, View};
use crate::theme::{
  ACCENT, BG, BORDER, MUTED, PANEL, RED, ROW_HOVER, ROW_SELECTED, TEXT, WHITE, data_color, ink_on, rgb,
};
use crate::ui::{Icon, icon};

actions!(working, [OpenCalendar]);

/// Height of the calendar's toolbar.
pub const TOOLBAR_HEIGHT: f32 = 56.0;
/// Width of the side panel.
const PANEL_WIDTH: f32 = 228.0;
/// Side of one day of a small month.
const MINI_CELL: f32 = 28.0;
/// The color of an event whose calendar is not listed.
const FALLBACK_COLOR: u32 = 0x039be5;
/// How often the view redraws for the line that marks now, and how many of those pass between two reads of the events.
const TICK: Duration = Duration::from_secs(60);
const TICKS_PER_READ: u32 = 5;

/// How an event is painted, from its color, the user's answer to it, and whether it is over.
pub struct Look {
  /// The event's own color.
  pub color: Rgba,
  /// Painted solid in `color`; otherwise outlined, as an invitation not accepted yet.
  pub filled: bool,
  /// The text color over it.
  pub ink: Rgba,
  /// Declined: the title is struck through.
  pub struck: bool,
  /// It has ended.
  pub past: bool,
}

pub struct CalendarView {
  pub(crate) store: Entity<Store>,
  pub(crate) focus: FocusHandle,
  pub(crate) mode: Mode,
  /// The day the view is on: shown alone by Day, inside the week of Week, in the month of Month.
  pub(crate) anchor: NaiveDate,
  /// The month the side panel shows. Follows `anchor`, until its arrows move it.
  mini: NaiveDate,
  /// The events on screen: the store's, without those of the calendars unchecked in the side panel.
  pub(crate) events: Arc<Vec<CalendarEvent>>,
  /// Each calendar's color, by id.
  colors: HashMap<String, u32>,
  pub(crate) hours: ScrollHandle,
  /// Bring the hours around now into view on the next draw.
  pub(crate) scroll_to_now: bool,
  card: Option<(Entity<EventCard>, Subscription)>,
  editor: Option<(Entity<EventEditor>, Subscription)>,
  /// Why the last deletion or answer failed, until the next write.
  failure: Option<String>,
  /// The calendar is the view on screen.
  active: bool,
  _subs: Vec<Subscription>,
  _tick: Task<()>,
}

impl CalendarView {
  pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let subs = vec![
      cx.observe_in(&store, window, |this, _, window, cx| this.sync(window, cx)),
      cx.observe_global::<Settings>(|this, cx| {
        this.refresh_events(cx);
        cx.notify();
      }),
    ];
    let tick = cx.spawn(async move |this, cx| {
      let mut ticks = 0_u32;
      loop {
        cx.background_executor().timer(TICK).await;
        ticks = ticks.wrapping_add(1);
        let alive = this.update(cx, |this, cx| {
          if !this.active {
            return;
          }
          if ticks.is_multiple_of(TICKS_PER_READ) && this.editor.is_none() {
            this.load(cx);
          }
          cx.notify();
        });
        if alive.is_err() {
          break;
        }
      }
    });
    let anchor = today();
    Self {
      store,
      focus: cx.focus_handle(),
      mode: cx.global::<Settings>().calendar_mode,
      anchor,
      mini: month_start(anchor),
      events: Arc::default(),
      colors: HashMap::new(),
      hours: ScrollHandle::new(),
      scroll_to_now: true,
      card: None,
      editor: None,
      failure: None,
      active: false,
      _subs: subs,
      _tick: tick,
    }
  }

  /// Follows the store: loads when the calendar comes on screen, takes the events, and hears how a write went.
  fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let shown = self.store.read(cx).view == View::Calendar;
    if shown && !self.active {
      self.active = true;
      self.scroll_to_now = true;
      self.load(cx);
    } else if !shown && self.active {
      self.active = false;
      self.card = None;
      self.editor = None;
    }
    if !self.active {
      return;
    }
    self.refresh_events(cx);
    let written = self.store.update(cx, |store, _| store.take_calendar_write());
    match written {
      Some(Ok(())) => {
        self.failure = None;
        if self.editor.take().is_some() {
          window.focus(&self.focus, cx);
        }
      },
      Some(Err(message)) => match &self.editor {
        Some((editor, _)) => editor.update(cx, |editor, cx| editor.fail(message, cx)),
        None => self.failure = Some(message),
      },
      None => {},
    }
    cx.notify();
  }

  fn refresh_events(&mut self, cx: &App) {
    let data = &self.store.read(cx).calendar;
    let hidden = &cx.global::<Settings>().calendar_hidden;
    self.colors = data
      .calendars
      .iter()
      .map(|calendar| (calendar.id.clone(), calendar.color))
      .collect();
    self.events = if hidden.is_empty() {
      data.events.clone()
    } else {
      let kept = data.events.iter().filter(|event| !hidden.contains(&event.calendar_id));
      Arc::new(kept.cloned().collect())
    };
  }

  /// Asks for the events of the time on screen.
  fn load(&self, cx: &mut Context<Self>) {
    let (start, end) = range(self.mode, self.anchor);
    self.store.update(cx, |store, cx| store.load_calendar(start, end, cx));
  }

  /// What takes the keyboard for the calendar's one-letter keys.
  pub fn focus_handle(&self) -> FocusHandle {
    self.focus.clone()
  }

  /// The event card and the editor, which the window root draws over everything.
  pub fn overlays(&self) -> Vec<AnyElement> {
    let card = self.card.as_ref().map(|(card, _)| card.clone().into_any_element());
    let editor = self.editor.as_ref().map(|(editor, _)| editor.clone().into_any_element());
    card.into_iter().chain(editor).collect()
  }

  /// The editor is open: it has the keyboard.
  pub const fn has_modal(&self) -> bool {
    self.editor.is_some()
  }

  /// Closes the editor, or else the event card. Answers whether one was open.
  pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    if self.editor.take().is_none() && self.card.take().is_none() {
      return false;
    }
    window.focus(&self.focus, cx);
    cx.notify();
    true
  }

  // Moving around.

  pub(crate) fn go_to(&mut self, date: NaiveDate, cx: &mut Context<Self>) {
    // The card is about an event of the view on screen: it goes when the view changes.
    self.card = None;
    self.anchor = date;
    self.mini = month_start(date);
    self.load(cx);
    cx.notify();
  }

  pub(crate) fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
    if mode == self.mode {
      return;
    }
    self.card = None;
    self.scroll_to_now = mode.has_hours() && !self.mode.has_hours();
    self.mode = mode;
    Settings::update(cx, |settings| settings.calendar_mode = mode);
    self.load(cx);
    cx.notify();
  }

  /// Shows one day: what a click on a day's number does in every view.
  pub(crate) fn open_day(&mut self, date: NaiveDate, cx: &mut Context<Self>) {
    self.anchor = date;
    self.mini = month_start(date);
    if self.mode == Mode::Day {
      self.load(cx);
      cx.notify();
    } else {
      self.set_mode(Mode::Day, cx);
    }
  }

  fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
    self.go_to(step(self.mode, self.anchor, forward), cx);
  }

  fn go_today(&mut self, cx: &mut Context<Self>) {
    self.scroll_to_now = true;
    self.go_to(today(), cx);
  }

  fn toggle_calendar(id: &str, cx: &mut App) {
    Settings::update(cx, |settings| {
      if let Some(index) = settings.calendar_hidden.iter().position(|held| held == id) {
        settings.calendar_hidden.remove(index);
      } else {
        settings.calendar_hidden.push(id.to_string());
      }
    });
  }

  // Events.

  /// The color an event is painted in: its own, or its calendar's.
  pub(crate) fn color_of(&self, event: &CalendarEvent) -> u32 {
    event
      .color
      .or_else(|| self.colors.get(&event.calendar_id).copied())
      .unwrap_or(FALLBACK_COLOR)
  }

  pub(crate) fn look(&self, event: &CalendarEvent, now: Micros) -> Look {
    let color = self.color_of(event);
    let declined = event.my_rsvp == Some(Rsvp::Declined);
    let filled = !matches!(event.my_rsvp, Some(Rsvp::NeedsAction | Rsvp::Declined));
    Look {
      color: data_color(color),
      filled,
      ink: match (filled, declined) {
        (true, _) => ink_on(color),
        (false, true) => rgb(MUTED),
        (false, false) => rgb(TEXT),
      },
      struck: declined,
      past: event.end <= now,
    }
  }

  /// Opens the card of an event at the pointer.
  pub(crate) fn open_card(
    &mut self,
    event: &CalendarEvent,
    at: Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let calendar = self
      .store
      .read(cx)
      .calendar
      .calendars
      .iter()
      .find(|calendar| calendar.id == event.calendar_id)
      .cloned();
    let (store, color, event) = (self.store.clone(), self.color_of(event), event.clone());
    let card = cx.new(|_| EventCard::new(store, event, calendar, color, at));
    let sub = cx.subscribe_in(&card, window, |this, card, event: &CardEvent, window, cx| match event {
      CardEvent::Close => {
        this.card = None;
        window.focus(&this.focus, cx);
        cx.notify();
      },
      CardEvent::Edit(scope) => {
        let event = card.read(cx).event().clone();
        this.card = None;
        this.open_editor(Seed::Edit { event: Box::new(event), scope: *scope }, window, cx);
      },
    });
    self.failure = None;
    self.card = Some((card, sub));
    cx.notify();
  }

  pub(crate) fn open_editor(&mut self, seed: Seed, window: &mut Window, cx: &mut Context<Self>) {
    let store = self.store.clone();
    let editor = cx.new(|cx| EventEditor::new(store, seed, window, cx));
    let sub = cx.subscribe_in(&editor, window, |this, _, _: &EditorEvent, window, cx| {
      this.editor = None;
      window.focus(&this.focus, cx);
      cx.notify();
    });
    self.card = None;
    self.failure = None;
    self.editor = Some((editor, sub));
    cx.notify();
  }

  /// A new event of one hour: from `start`, or from the next half hour of the day on screen.
  pub(crate) fn new_event(&mut self, start: Option<Micros>, window: &mut Window, cx: &mut Context<Self>) {
    self.open_new_event(start, false, window, cx);
  }

  /// A new event of one hour on `date` with Google Meet turned on, as Meet's "Schedule in Google Calendar" asks for.
  pub fn new_meeting(&mut self, date: NaiveDate, window: &mut Window, cx: &mut Context<Self>) {
    self.go_to(date, cx);
    self.open_new_event(None, true, window, cx);
  }

  fn open_new_event(&mut self, start: Option<Micros>, meet: bool, window: &mut Window, cx: &mut Context<Self>) {
    let start = start.unwrap_or_else(|| {
      let now = local(now_micros());
      // Today: the next half hour. Another day: nine in the morning.
      let minutes = if self.anchor == today() {
        i64::from(now.hour() * 60 + now.minute()) / 30 * 30 + 30
      } else {
        9 * 60
      };
      day_start(self.anchor) + minutes * MINUTE
    });
    let seed = Seed::New {
      start,
      end: start + HOUR,
      all_day: false,
      meet,
    };
    self.open_editor(seed, window, cx);
  }

  /// A new all-day event on `date`.
  pub(crate) fn new_all_day(&mut self, date: NaiveDate, window: &mut Window, cx: &mut Context<Self>) {
    let seed = Seed::New {
      start: day_start(date),
      end: day_start(add_days(date, 1)),
      all_day: true,
      meet: false,
    };
    self.open_editor(seed, window, cx);
  }

  fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
    let stroke = &event.keystroke;
    if stroke.modifiers.platform || stroke.modifiers.control || stroke.modifiers.alt {
      return;
    }
    match stroke.key.as_str() {
      "escape" => {
        if !self.dismiss(window, cx) {
          return;
        }
      },
      "t" => self.go_today(cx),
      "left" | "k" | "p" => self.step(false, cx),
      "right" | "j" | "n" => self.step(true, cx),
      "c" => self.new_event(None, window, cx),
      key => match Mode::ALL.into_iter().find(|mode| mode.key() == key) {
        Some(mode) => self.set_mode(mode, cx),
        None => return,
      },
    }
    cx.stop_propagation();
  }

  // Chrome.

  fn mode_button(&self, cx: &Context<Self>) -> Stateful<Div> {
    let (view, current) = (cx.entity(), self.mode);
    pill("calendar-mode")
      .pr_2()
      .gap_1()
      .child(current.label())
      .child(icon(Icon::ChevronDown, 14., MUTED))
      .on_mouse_down(MouseButton::Left, move |event, window, cx| {
        cx.stop_propagation();
        let items = Mode::ALL
          .into_iter()
          .map(|mode| {
            let view = view.clone();
            let label = format!("{}  ({})", mode.label(), mode.key().to_uppercase());
            MenuItem::new(label, move |_, cx| view.update(cx, |this, cx| this.set_mode(mode, cx)))
              .checked(mode == current)
          })
          .collect();
        menu::show(event.position, items, window, cx);
      })
  }

  /// What the toolbar says while something is on its way or went wrong.
  fn status(&self, cx: &App) -> Option<Div> {
    let data = &self.store.read(cx).calendar;
    let line = |text: String, tone| {
      div()
        .max_w(px(280.))
        .truncate()
        .text_size(px(12.))
        .text_color(rgb(tone))
        .child(text)
    };
    if let Some(error) = self.failure.as_ref().or(data.error.as_ref()) {
      return Some(line(error.clone(), RED));
    }
    if data.writing {
      return Some(line("Saving…".into(), MUTED));
    }
    (!data.loaded).then(|| line("Loading…".into(), MUTED))
  }

  fn toolbar(&self, cx: &mut Context<Self>) -> Div {
    let panel_open = !cx.global::<Settings>().calendar_panel_closed;
    h_flex()
      .h(px(TOOLBAR_HEIGHT))
      .flex_shrink_0()
      .px_4()
      .gap_2()
      .items_center()
      .child(
        round_button("calendar-panel", Icon::List)
          .when(panel_open, |button| button.bg(rgb(ROW_SELECTED)))
          .tooltip(hint(if panel_open {
            "Hide the calendars"
          } else {
            "Show the calendars"
          }))
          .on_click(|_, _, cx| {
            Settings::update(cx, |settings| settings.calendar_panel_closed = !settings.calendar_panel_closed);
          }),
      )
      .child(
        pill("calendar-today")
          .child("Today")
          .tooltip(|window, cx| Tooltip::new(today().format("%A, %B %-d").to_string()).build(window, cx))
          .on_click(cx.listener(|this, _, _, cx| this.go_today(cx))),
      )
      .child(
        h_flex()
          .child(
            round_button("calendar-previous", Icon::ChevronLeft)
              .tooltip(hint("Previous"))
              .on_click(cx.listener(|this, _, _, cx| this.step(false, cx))),
          )
          .child(
            round_button("calendar-next", Icon::ChevronRight)
              .tooltip(hint("Next"))
              .on_click(cx.listener(|this, _, _, cx| this.step(true, cx))),
          ),
      )
      .child(
        div()
          .flex_1()
          .min_w_0()
          .truncate()
          .text_size(px(20.))
          .text_color(rgb(TEXT))
          .child(title(self.mode, self.anchor)),
      )
      .children(self.status(cx))
      .child(self.mode_button(cx))
      .child(
        h_flex()
          .id("calendar-new")
          .h(px(32.))
          .pl_2p5()
          .pr_3()
          .gap_1p5()
          .flex_shrink_0()
          .items_center()
          .rounded_full()
          .bg(rgb(ACCENT))
          .text_size(px(13.))
          .font_weight(FontWeight::MEDIUM)
          .text_color(rgb(BG))
          .cursor_pointer()
          .hover(|style| style.opacity(0.9))
          .tooltip(hint("New event (C)"))
          .on_click(cx.listener(|this, _, window, cx| this.new_event(None, window, cx)))
          .child(icon(Icon::Plus, 15., BG))
          .child("New event"),
      )
  }

  /// A small month: the side panel's, and each of the Year view's twelve. A click on a day moves the view on screen
  /// to it, or with `marks` (the Year view) opens that day; `marks` puts a dot under the days that have events. `slot`
  /// tells the months of one screen apart.
  pub(crate) fn mini_month(
    &self,
    month: NaiveDate,
    slot: usize,
    marks: Option<&HashSet<NaiveDate>>,
    cx: &mut Context<Self>,
  ) -> Div {
    let now = today();
    let cell = || {
      div()
        .size(px(MINI_CELL))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .text_size(px(11.))
    };
    let mut grid = v_flex().child(
      h_flex().children(
        ["S", "M", "T", "W", "T", "F", "S"]
          .into_iter()
          .map(|letter| cell().text_color(rgb(MUTED)).child(letter)),
      ),
    );
    for (week_ix, week) in month_weeks(month).into_iter().enumerate() {
      let mut row = h_flex();
      for (day_ix, day) in week.into_iter().enumerate() {
        if day.month() != month.month() {
          row = row.child(cell());
          continue;
        }
        let is_today = day == now;
        let picked = day == self.anchor && !is_today;
        let marked = marks.is_some_and(|marks| marks.contains(&day));
        let open_day = marks.is_some();
        row = row.child(
          cell()
            .id(("mini-day", slot * 64 + week_ix * 7 + day_ix))
            .rounded_full()
            .cursor_pointer()
            .text_color(rgb(TEXT))
            .when(is_today, |cell| cell.bg(rgb(ACCENT)).text_color(rgb(BG)))
            .when(picked, |cell| cell.bg(rgb(ROW_SELECTED)))
            .when(!is_today && !picked, |cell| cell.hover(|style| style.bg(rgb(ROW_HOVER))))
            .on_click(cx.listener(move |this, _, _, cx| {
              if open_day {
                this.open_day(day, cx);
              } else {
                this.go_to(day, cx);
              }
            }))
            .child(day.day().to_string())
            .when(marked, |cell| {
              let dot = if is_today { BG } else { ACCENT };
              cell.child(div().size(px(3.)).rounded_full().bg(rgb(dot)))
            }),
        );
      }
      grid = grid.child(row);
    }
    grid
  }

  /// A section of the side panel's list: its title, then a checkbox in each calendar's color.
  fn calendar_list(name: &'static str, calendars: &[&Calendar], hidden: &[String]) -> Div {
    let mut list = v_flex().child(
      div()
        .h(px(26.))
        .px_2()
        .flex()
        .items_center()
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(MUTED))
        .child(name),
    );
    for calendar in calendars {
      let checked = !hidden.contains(&calendar.id);
      let id = calendar.id.clone();
      list = list.child(
        h_flex()
          .id(SharedString::from(format!("calendar-{}", calendar.id)))
          .h(px(30.))
          .px_2()
          .gap_2p5()
          .items_center()
          .rounded(px(8.))
          .cursor_pointer()
          .hover(|style| style.bg(rgb(ROW_HOVER)))
          .on_click(move |_, _, cx| Self::toggle_calendar(&id, cx))
          .child(
            div()
              .size(px(16.))
              .flex()
              .flex_shrink_0()
              .items_center()
              .justify_center()
              .rounded(px(4.))
              .border_2()
              .border_color(data_color(calendar.color))
              .when(checked, |bx| {
                bx.bg(data_color(calendar.color))
                  .child(icon(Icon::Check, 12., WHITE).text_color(ink_on(calendar.color)))
              }),
          )
          .child(
            div()
              .flex_1()
              .min_w_0()
              .truncate()
              .text_size(px(13.))
              .text_color(rgb(TEXT))
              .child(calendar.name.clone()),
          ),
      );
    }
    list
  }

  /// The side panel: a small month to move by, and the calendars to show or hide.
  fn panel(&self, cx: &mut Context<Self>) -> Stateful<Div> {
    let calendars = self.store.read(cx).calendar.calendars.clone();
    let hidden = cx.global::<Settings>().calendar_hidden.clone();
    let (mine, others): (Vec<&Calendar>, Vec<&Calendar>) =
      calendars.iter().partition(|calendar| calendar.access.can_write());
    let month = self.mini;
    let mini_header = h_flex()
      .h(px(30.))
      .pl_2()
      .items_center()
      .child(
        div()
          .flex_1()
          .text_size(px(13.))
          .font_weight(FontWeight::MEDIUM)
          .text_color(rgb(TEXT))
          .child(month.format("%B %Y").to_string()),
      )
      .child(
        round_button("mini-previous", Icon::ChevronLeft).on_click(cx.listener(|this, _, _, cx| {
          this.mini = add_months(this.mini, 1, false);
          cx.notify();
        })),
      )
      .child(
        round_button("mini-next", Icon::ChevronRight).on_click(cx.listener(|this, _, _, cx| {
          this.mini = add_months(this.mini, 1, true);
          cx.notify();
        })),
      );
    v_flex()
      .id("calendar-side")
      .w(px(PANEL_WIDTH))
      .h_full()
      .flex_shrink_0()
      .px_3()
      .pb_3()
      .gap_3()
      .overflow_y_scroll()
      .border_r_1()
      .border_color(rgb(BORDER))
      .child(
        v_flex()
          .items_center()
          .child(mini_header.w(px(MINI_CELL * 7.)))
          .child(self.mini_month(month, 0, None, cx)),
      )
      .when(!mine.is_empty(), |panel| {
        panel.child(Self::calendar_list("My calendars", &mine, &hidden))
      })
      .when(!others.is_empty(), |panel| {
        panel.child(Self::calendar_list("Other calendars", &others, &hidden))
      })
  }
}

/// A round icon button of the toolbar.
pub fn round_button(id: &'static str, glyph: Icon) -> Stateful<Div> {
  div()
    .id(id)
    .size(px(30.))
    .flex()
    .flex_shrink_0()
    .items_center()
    .justify_center()
    .rounded_full()
    .cursor_pointer()
    .hover(|style| style.bg(rgb(ROW_HOVER)))
    .child(icon(glyph, 16., TEXT))
}

/// An outlined button with a label.
pub fn pill(id: &'static str) -> Stateful<Div> {
  h_flex()
    .id(id)
    .h(px(32.))
    .px_3()
    .flex_shrink_0()
    .items_center()
    .rounded_full()
    .border_1()
    .border_color(rgb(BORDER))
    .text_size(px(13.))
    .text_color(rgb(TEXT))
    .cursor_pointer()
    .hover(|style| style.bg(rgb(ROW_HOVER)))
}

impl Render for CalendarView {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let panel_open = !cx.global::<Settings>().calendar_panel_closed;
    let body = match self.mode {
      Mode::Day | Mode::FourDays | Mode::Week => self.hours_view(cx),
      Mode::Month => self.month_view(window, cx),
      Mode::Year => self.year_view(cx),
      Mode::Schedule => self.schedule_view(cx),
    };
    v_flex()
      .id("calendar")
      .size_full()
      .bg(rgb(PANEL))
      .track_focus(&self.focus)
      .key_context("Calendar")
      .on_key_down(cx.listener(Self::on_key))
      .child(self.toolbar(cx))
      .child(
        h_flex()
          .flex_1()
          .min_h_0()
          .border_t_1()
          .border_color(rgb(BORDER))
          .when(panel_open, |row| row.child(self.panel(cx)))
          .child(div().flex_1().min_w_0().h_full().child(body)),
      )
  }
}

/// Which occurrences a change to a recurring event is for: a menu at the pointer with the two choices. An event that
/// does not repeat has nothing to ask. Either way `then` runs after the caller's own update is over, so it may update
/// the view that asked.
pub fn ask_scope(
  event: &CalendarEvent,
  at: Point<Pixels>,
  window: &mut Window,
  cx: &mut App,
  then: impl Fn(EditScope, &mut Window, &mut App) + 'static,
) {
  if event.series.is_none() {
    window.defer(cx, move |window, cx| then(EditScope::This, window, cx));
    return;
  }
  let then = std::rc::Rc::new(then);
  let item = |label: &'static str, scope: EditScope| {
    let then = then.clone();
    MenuItem::new(label, move |window, cx| then(scope, window, cx))
  };
  let items = vec![item("This event", EditScope::This), item("All events", EditScope::All)];
  menu::show(at, items, window, cx);
}
