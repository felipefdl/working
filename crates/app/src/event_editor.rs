//! The event editor: a dialog that makes a new event or changes one. It builds an `EventDraft` and sends it; the
//! calendar view closes it once Google Calendar has the event, or hands back why it was refused.
#![allow(
  clippy::too_many_lines,
  clippy::struct_excessive_bools,
  reason = "one dialog with a field per event property"
)]

use chrono::{Days, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use gchat_core::fuzzy::Fuzzy;
use gchat_core::{
  Calendar, CalendarEvent, Command, EVENT_COLORS, EditScope, EventDraft, EventKind, Micros, Reminder, Repeat, User,
  Visibility,
};
use gpui_kit::component::date_picker::{DatePicker, DatePickerEvent, DatePickerState, DateTime};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::time_field::{HourCycle, TimeField, TimeFieldEvent, TimeFieldState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::calendar_layout::{day_start, event_days, local};
use crate::conv_list::hint;
use crate::menu::{self, MenuItem};
use crate::store::Store;
use crate::theme::{
  ACCENT, ACCENT_BG, BG, BORDER, MUTED, POPOVER, RED, ROW_HOVER, TEXT, control_fill, data_color, rgb,
};
use crate::ui::{Icon, icon, switch};

const WIDTH: f32 = 580.0;
/// People offered under the guests field.
const SUGGESTIONS: usize = 5;
/// The notifications offered, in minutes before the event.
const REMINDERS: [u32; 6] = [0, 5, 10, 30, 60, 1440];

pub enum EditorEvent {
  Close,
}

/// What the editor opens on.
pub enum Seed {
  /// A new event at this time.
  New {
    start: Micros,
    end: Micros,
    all_day: bool,
    /// Google Meet starts turned on.
    meet: bool,
  },
  /// An event to change: it alone, or with `EditScope::All` every occurrence of its series.
  Edit {
    event: Box<CalendarEvent>,
    scope: EditScope,
  },
}

pub struct EventEditor {
  store: Entity<Store>,
  /// The event being changed and the occurrences the change is for. `None` makes a new event.
  editing: Option<(CalendarEvent, EditScope)>,
  kind: EventKind,
  title: Entity<InputState>,
  location: Entity<InputState>,
  description: Entity<TextareaState>,
  guest_input: Entity<InputState>,
  /// Guests' emails, in the order added.
  guests: Vec<String>,
  /// People matching what is typed in the guests field.
  suggestions: Vec<User>,
  start_date: Entity<DatePickerState>,
  end_date: Entity<DatePickerState>,
  start_time: Entity<TimeFieldState>,
  end_time: Entity<TimeFieldState>,
  /// Local start and end. All-day: only the dates count, and `end` is the last day.
  start: NaiveDateTime,
  end: NaiveDateTime,
  all_day: bool,
  repeat: Repeat,
  add_meet: bool,
  calendar_id: String,
  color_id: Option<u8>,
  busy: bool,
  visibility: Visibility,
  reminder: Reminder,
  /// Email the guests about the change.
  notify: bool,
  saving: bool,
  error: Option<String>,
  _subs: Vec<Subscription>,
}

impl EventEmitter<EditorEvent> for EventEditor {}

fn naive(time: Micros) -> NaiveDateTime {
  local(time).naive_local()
}

/// A local date and time as an instant; the first one when a clock change repeats it, the one after when it skips it.
fn instant(time: NaiveDateTime) -> Micros {
  Local
    .from_local_datetime(&time)
    .earliest()
    .map_or_else(|| day_start(time.date()), |at| at.timestamp_micros())
}

const fn repeat_label(repeat: Repeat) -> &'static str {
  match repeat {
    Repeat::Never => "Does not repeat",
    Repeat::Daily => "Daily",
    Repeat::Weekly => "Weekly",
    Repeat::Monthly => "Monthly",
    Repeat::Yearly => "Yearly",
    Repeat::Weekdays => "Every weekday",
  }
}

fn reminder_label(reminder: Reminder) -> String {
  match reminder {
    Reminder::Default => "Default notification".into(),
    Reminder::None => "No notification".into(),
    Reminder::Minutes(0) => "At the start".into(),
    Reminder::Minutes(60) => "1 hour before".into(),
    Reminder::Minutes(1440) => "1 day before".into(),
    Reminder::Minutes(minutes) => format!("{minutes} minutes before"),
  }
}

/// Whether `text` can be invited: something, an at sign, a domain with a dot.
fn is_email(text: &str) -> bool {
  let Some((name, domain)) = text.split_once('@') else {
    return false;
  };
  !name.is_empty() && domain.contains('.') && !domain.ends_with('.') && !text.contains(char::is_whitespace)
}

/// A row of the dialog: an icon, then its fields.
fn row(glyph: Icon) -> Div {
  h_flex()
    .gap_3()
    .items_start()
    .child(div().h(px(34.)).flex().items_center().child(icon(glyph, 17., MUTED)))
}

/// The filled box a text field sits in.
fn field() -> Div {
  div()
    .min_h(px(34.))
    .px_2()
    .flex()
    .items_center()
    .rounded(px(8.))
    .bg(control_fill())
}

/// A button that shows a choice and opens the menu of its alternatives.
fn picker(id: &'static str, label: impl Into<SharedString>) -> Stateful<Div> {
  h_flex()
    .id(id)
    .h(px(34.))
    .pl_3()
    .pr_2()
    .gap_1p5()
    .flex_shrink_0()
    .items_center()
    .rounded(px(8.))
    .bg(control_fill())
    .text_size(px(13.))
    .text_color(rgb(TEXT))
    .cursor_pointer()
    .hover(|style| style.opacity(0.85))
    .child(label.into())
    .child(icon(Icon::ChevronDown, 14., MUTED))
}

impl EventEditor {
  pub fn new(store: Entity<Store>, seed: Seed, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let calendars = store.read(cx).calendar.calendars.clone();
    let own = calendars
      .iter()
      .find(|calendar| calendar.primary)
      .or_else(|| calendars.iter().find(|calendar| calendar.access.can_write()))
      .map_or_else(|| "primary".to_string(), |calendar| calendar.id.clone());
    let (editing, start, end, all_day, meet) = match seed {
      Seed::New { start, end, all_day, meet } => (None, start, end, all_day, meet),
      Seed::Edit { event, scope } => {
        let (start, end, all_day) = (event.start, event.end, event.all_day);
        (Some((*event, scope)), start, end, all_day, false)
      },
    };
    let held = editing.as_ref().map(|(event, _)| event);
    // All-day: the dates of the first and the last day. Timed: the times as they are.
    let (start, end) = if all_day {
      let (first, last) = held.map_or_else(
        || {
          let first = local(start).date_naive();
          (first, local(end - 1).date_naive().max(first))
        },
        event_days,
      );
      (first.and_time(NaiveTime::MIN), last.and_time(NaiveTime::MIN))
    } else {
      (naive(start), naive(end))
    };

    let text = |placeholder: &'static str, value: &str, window: &mut Window, cx: &mut Context<Self>| {
      let state = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
      if !value.is_empty() {
        state.update(cx, |state, cx| state.set_value(value.to_string(), window, cx));
      }
      state
    };
    let title = text("Add a title", held.map_or("", |event| event.title.as_str()), window, cx);
    let location = text("Add a location", held.map_or("", |event| event.location.as_str()), window, cx);
    let guest_input = text("Add guests by name or email", "", window, cx);
    let description = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 6).placeholder("Add a description"));
    if let Some(event) = held.filter(|event| !event.description.is_empty()) {
      let value = event.description.clone();
      description.update(cx, |state, cx| state.set_value(value, window, cx));
    }
    let date = |date: NaiveDate, window: &mut Window, cx: &mut Context<Self>| {
      cx.new(|cx| {
        let mut state = DatePickerState::new(window, cx).date_format("%a, %b %-d, %Y");
        state.set_date(date, window, cx);
        state
      })
    };
    let time = |time: NaiveTime, window: &mut Window, cx: &mut Context<Self>| {
      cx.new(|cx| {
        let mut state = TimeFieldState::new(window, cx).hour_cycle(HourCycle::H12);
        state.set_time(time, window, cx);
        state
      })
    };
    let (start_date, end_date) = (date(start.date(), window, cx), date(end.date(), window, cx));
    let (start_time, end_time) = (time(start.time(), window, cx), time(end.time(), window, cx));

    let subs = vec![
      cx.subscribe_in(&title, window, |this, _, event: &InputEvent, _, cx| {
        if matches!(event, InputEvent::PressEnter { .. }) {
          this.save(cx);
        }
      }),
      cx.subscribe_in(&guest_input, window, |this, _, event: &InputEvent, window, cx| match event {
        InputEvent::Change => this.suggest(cx),
        InputEvent::PressEnter { .. } => this.add_typed_guest(window, cx),
        _ => {},
      }),
      cx.subscribe_in(&start_date, window, |this, _, event: &DatePickerEvent, window, cx| {
        let DatePickerEvent::Change(DateTime::Single(Some(picked))) = event else {
          return;
        };
        this.set_start(picked.date().and_time(this.start.time()), window, cx);
      }),
      cx.subscribe_in(&end_date, window, |this, _, event: &DatePickerEvent, _, cx| {
        let DatePickerEvent::Change(DateTime::Single(Some(picked))) = event else {
          return;
        };
        this.end = picked.date().and_time(this.end.time());
        this.error = None;
        cx.notify();
      }),
      cx.subscribe_in(&start_time, window, |this, _, event: &TimeFieldEvent, window, cx| {
        let TimeFieldEvent::Change(picked) = event;
        this.set_start(this.start.date().and_time(*picked), window, cx);
      }),
      cx.subscribe_in(&end_time, window, |this, _, event: &TimeFieldEvent, _, cx| {
        let TimeFieldEvent::Change(picked) = event;
        this.end = this.end.date().and_time(*picked);
        this.error = None;
        cx.notify();
      }),
    ];
    title.update(cx, |state, cx| state.focus(window, cx));

    let guests = held
      .map(|event| {
        event
          .guests
          .iter()
          .filter(|guest| !guest.me)
          .map(|guest| guest.email.clone())
          .collect()
      })
      .unwrap_or_default();
    Self {
      kind: held.map_or(EventKind::Event, |event| event.kind),
      calendar_id: held.map_or(own, |event| event.calendar_id.clone()),
      color_id: held.and_then(|event| event.color_id),
      // A new all-day event shows the user as free, as Google Calendar makes it.
      busy: held.map_or(!all_day, |event| event.busy),
      visibility: held.map_or(Visibility::Default, |event| event.visibility),
      reminder: held.map_or(Reminder::Default, |event| event.reminder),
      store,
      editing,
      title,
      location,
      description,
      guest_input,
      guests,
      suggestions: Vec::new(),
      start_date,
      end_date,
      start_time,
      end_time,
      start,
      end,
      all_day,
      repeat: Repeat::Never,
      add_meet: meet,
      notify: true,
      saving: false,
      error: None,
      _subs: subs,
    }
  }

  /// The save was refused: shows why and takes edits again.
  pub fn fail(&mut self, message: String, cx: &mut Context<Self>) {
    self.saving = false;
    self.error = Some(message);
    cx.notify();
  }

  /// Moves the start, and the end with it so the event keeps its length.
  fn set_start(&mut self, start: NaiveDateTime, window: &mut Window, cx: &mut Context<Self>) {
    let length = self.end - self.start;
    self.start = start;
    self.end = start + length;
    let end = self.end;
    self.end_date.update(cx, |state, cx| state.set_date(end.date(), window, cx));
    self.end_time.update(cx, |state, cx| state.set_time(end.time(), window, cx));
    self.error = None;
    cx.notify();
  }

  fn set_all_day(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
    if on == self.all_day {
      return;
    }
    self.all_day = on;
    if self.editing.is_none() {
      self.busy = !on;
    }
    // Back to a timed event: an hour from nine, on the days it had.
    if !on && self.start.time() == NaiveTime::MIN && self.end.time() == NaiveTime::MIN {
      let nine = NaiveTime::from_hms_opt(9, 0, 0).unwrap_or(NaiveTime::MIN);
      let ten = NaiveTime::from_hms_opt(10, 0, 0).unwrap_or(NaiveTime::MIN);
      self.start = self.start.date().and_time(nine);
      self.end = self.end.date().and_time(ten);
      self.start_time.update(cx, |state, cx| state.set_time(nine, window, cx));
      self.end_time.update(cx, |state, cx| state.set_time(ten, window, cx));
    }
    self.error = None;
    cx.notify();
  }

  /// Events of the other kinds have a time and no guests, location, or meeting, and live in the user's own calendar.
  const fn plain(&self) -> bool {
    matches!(self.kind, EventKind::Event)
  }

  fn set_kind(&mut self, kind: EventKind, window: &mut Window, cx: &mut Context<Self>) {
    self.kind = kind;
    if !self.plain() {
      self.set_all_day(false, window, cx);
      let calendars = self.store.read(cx).calendar.calendars.clone();
      if let Some(own) = calendars.iter().find(|calendar| calendar.primary) {
        self.calendar_id.clone_from(&own.id);
      }
    }
    cx.notify();
  }

  /// Why the event cannot be saved as it stands.
  fn problem(&self) -> Option<&'static str> {
    if self.all_day {
      return (self.end.date() < self.start.date()).then_some("The last day is before the first.");
    }
    (self.end <= self.start).then_some("The end has to be after the start.")
  }

  fn save(&mut self, cx: &mut Context<Self>) {
    if self.saving {
      return;
    }
    if let Some(problem) = self.problem() {
      self.error = Some(problem.into());
      cx.notify();
      return;
    }
    let (start, end) = if self.all_day {
      let after = self
        .end
        .date()
        .checked_add_days(Days::new(1))
        .unwrap_or_else(|| self.end.date());
      (day_start(self.start.date()), day_start(after))
    } else {
      (instant(self.start), instant(self.end))
    };
    let plain = self.plain();
    // An event that already repeats keeps its rule; a new one, or one that did not repeat, takes the one picked.
    let repeats = self.editing.as_ref().is_some_and(|(event, _)| event.series.is_some());
    let repeat = match (&self.editing, self.repeat) {
      (None, repeat) => Some(repeat),
      (Some(_), Repeat::Never) => None,
      (Some(_), repeat) => (!repeats).then_some(repeat),
    };
    let draft = EventDraft {
      calendar_id: self.calendar_id.clone(),
      editing: self
        .editing
        .as_ref()
        .map(|(event, _)| (event.calendar_id.clone(), event.id.clone())),
      scope: self.editing.as_ref().map_or(EditScope::This, |(_, scope)| *scope),
      kind: self.kind,
      title: self.title.read(cx).value().to_string(),
      start,
      end,
      all_day: self.all_day,
      location: if plain {
        self.location.read(cx).value().to_string()
      } else {
        String::new()
      },
      description: self.description.read(cx).value().to_string(),
      guests: if plain {
        self.guests.clone()
      } else {
        Vec::new()
      },
      add_meet: plain && self.add_meet,
      repeat,
      busy: self.busy,
      visibility: self.visibility,
      color_id: self.color_id,
      reminder: self.reminder,
      notify: self.notify && !self.guests.is_empty(),
    };
    self.saving = true;
    self.error = None;
    self
      .store
      .update(cx, |store, cx| store.write_calendar(Command::SaveEvent(Box::new(draft)), cx));
    cx.notify();
  }

  // Guests.

  /// The people matching the guests field, best first, without those already invited.
  fn suggest(&mut self, cx: &mut Context<Self>) {
    let query = self.guest_input.read(cx).value().trim().to_string();
    self.suggestions = if query.is_empty() {
      Vec::new()
    } else {
      let mut fuzzy = Fuzzy::new(&query);
      let mut hits: Vec<(u32, &User)> = self
        .store
        .read(cx)
        .people
        .iter()
        .filter(|user| !user.email.is_empty() && !self.guests.contains(&user.email))
        .filter_map(|user| {
          let score = fuzzy.score(&user.name).into_iter().chain(fuzzy.score(&user.email)).max();
          score.map(|score| (score, user))
        })
        .collect();
      hits.sort_by(|a, b| b.0.cmp(&a.0));
      hits.into_iter().take(SUGGESTIONS).map(|(_, user)| user.clone()).collect()
    };
    cx.notify();
  }

  fn add_guest(&mut self, email: String, window: &mut Window, cx: &mut Context<Self>) {
    if !self.guests.iter().any(|held| held.eq_ignore_ascii_case(&email)) {
      self.guests.push(email);
    }
    self.guest_input.update(cx, |state, cx| state.set_value("", window, cx));
    self.suggestions.clear();
    cx.notify();
  }

  /// Enter in the guests field: the address typed, or else the first person offered.
  fn add_typed_guest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let typed = self.guest_input.read(cx).value().trim().to_string();
    if is_email(&typed) {
      self.add_guest(typed, window, cx);
    } else if let Some(user) = self.suggestions.first() {
      self.add_guest(user.email.clone(), window, cx);
    }
  }

  /// The name to show for a guest: the person's, when the directory has the address.
  fn guest_name(&self, email: &str, cx: &App) -> String {
    let invited = self
      .editing
      .as_ref()
      .and_then(|(event, _)| event.guests.iter().find(|guest| guest.email == email))
      .map(|guest| guest.name.clone())
      .filter(|name| !name.is_empty());
    invited
      .or_else(|| {
        let people = &self.store.read(cx).people;
        people.iter().find(|user| user.email == email).map(|user| user.name.clone())
      })
      .unwrap_or_else(|| email.to_string())
  }

  fn guests_row(&self, cx: &Context<Self>) -> Div {
    let mut chips = h_flex().gap_1().flex_wrap();
    for (index, email) in self.guests.iter().enumerate() {
      let remove = email.clone();
      chips = chips.child(
        h_flex()
          .id(("guest", index))
          .h(px(26.))
          .pl_2p5()
          .pr_1p5()
          .gap_1()
          .items_center()
          .rounded_full()
          .bg(control_fill())
          .text_size(px(12.))
          .text_color(rgb(TEXT))
          .tooltip({
            let email = email.clone();
            move |window, cx| gpui_kit::component::tooltip::Tooltip::new(email.clone()).build(window, cx)
          })
          .child(self.guest_name(email, cx))
          .child(
            div()
              .id(("guest-remove", index))
              .cursor_pointer()
              .on_click(cx.listener(move |this, _, _, cx| {
                this.guests.retain(|held| *held != remove);
                cx.notify();
              }))
              .child(icon(Icon::X, 12., MUTED)),
          ),
      );
    }
    let mut offered = v_flex();
    for (index, user) in self.suggestions.iter().enumerate() {
      let email = user.email.clone();
      offered = offered.child(
        h_flex()
          .id(("guest-offer", index))
          .h(px(30.))
          .px_2()
          .gap_2()
          .items_center()
          .rounded(px(8.))
          .cursor_pointer()
          .hover(|style| style.bg(rgb(ROW_HOVER)))
          .on_click(cx.listener(move |this, _, window, cx| this.add_guest(email.clone(), window, cx)))
          .child(div().text_size(px(13.)).text_color(rgb(TEXT)).child(user.name.clone()))
          .child(
            div()
              .min_w_0()
              .truncate()
              .text_size(px(12.))
              .text_color(rgb(MUTED))
              .child(user.email.clone()),
          ),
      );
    }
    row(Icon::Users).child(
      v_flex()
        .flex_1()
        .min_w_0()
        .gap_1p5()
        .child(field().child(Input::new(&self.guest_input).appearance(false).bordered(false)))
        .when(!self.suggestions.is_empty(), |column| column.child(offered))
        .when(!self.guests.is_empty(), |column| column.child(chips)),
    )
  }

  // Pickers.

  /// Opens a menu of `choices` at the pointer, the one in use checked; picking one runs `set` on the editor.
  fn choose<T: Copy + PartialEq + 'static>(
    choices: Vec<(SharedString, T)>,
    current: T,
    set: fn(&mut Self, T),
    cx: &Context<Self>,
  ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    let editor = cx.entity();
    move |event, window, cx| {
      cx.stop_propagation();
      let items = choices
        .iter()
        .map(|(label, value)| {
          let (editor, value) = (editor.clone(), *value);
          MenuItem::new(label.clone(), move |_, cx| {
            editor.update(cx, |this, cx| {
              set(this, value);
              cx.notify();
            });
          })
          .checked(value == current)
        })
        .collect();
      menu::show(event.position, items, window, cx);
    }
  }

  fn repeat_picker(&self, cx: &Context<Self>) -> Stateful<Div> {
    let choices = [
      Repeat::Never,
      Repeat::Daily,
      Repeat::Weekly,
      Repeat::Monthly,
      Repeat::Yearly,
      Repeat::Weekdays,
    ]
    .into_iter()
    .map(|repeat| (repeat_label(repeat).into(), repeat))
    .collect();
    picker("event-repeat", repeat_label(self.repeat)).on_mouse_down(
      MouseButton::Left,
      Self::choose(choices, self.repeat, |this, repeat| this.repeat = repeat, cx),
    )
  }

  fn calendar_picker(&self, calendars: &[Calendar], cx: &Context<Self>) -> Stateful<Div> {
    let writable: Vec<&Calendar> = calendars.iter().filter(|calendar| calendar.access.can_write()).collect();
    let current = writable.iter().position(|calendar| calendar.id == self.calendar_id);
    let name = current
      .and_then(|index| writable.get(index))
      .map_or_else(|| "Calendar".to_string(), |calendar| calendar.name.clone());
    let editor = cx.entity();
    let choices: Vec<(SharedString, String)> = writable
      .iter()
      .map(|calendar| (calendar.name.clone().into(), calendar.id.clone()))
      .collect();
    let held = self.calendar_id.clone();
    picker("event-calendar", name).on_mouse_down(MouseButton::Left, move |event, window, cx| {
      cx.stop_propagation();
      let items = choices
        .iter()
        .map(|(label, id)| {
          let (editor, id, checked) = (editor.clone(), id.clone(), *id == held);
          MenuItem::new(label.clone(), move |_, cx| {
            editor.update(cx, |this, cx| {
              this.calendar_id.clone_from(&id);
              cx.notify();
            });
          })
          .checked(checked)
        })
        .collect();
      menu::show(event.position, items, window, cx);
    })
  }

  fn color_picker(&self, calendars: &[Calendar], cx: &Context<Self>) -> Stateful<Div> {
    let calendar_color = calendars
      .iter()
      .find(|calendar| calendar.id == self.calendar_id)
      .map(|calendar| calendar.color);
    let named = EVENT_COLORS.iter().find(|(id, _, _)| Some(*id) == self.color_id);
    let (label, shown) = named.map_or(("Calendar color", calendar_color), |(_, name, color)| (*name, Some(*color)));
    let mut choices: Vec<(SharedString, Option<u8>)> = vec![("Calendar color".into(), None)];
    choices.extend(EVENT_COLORS.iter().map(|(id, name, _)| ((*name).into(), Some(*id))));
    h_flex()
      .id("event-color")
      .h(px(34.))
      .pl_2p5()
      .pr_2()
      .gap_2()
      .flex_shrink_0()
      .items_center()
      .rounded(px(8.))
      .bg(control_fill())
      .text_size(px(13.))
      .text_color(rgb(TEXT))
      .cursor_pointer()
      .hover(|style| style.opacity(0.85))
      .children(shown.map(|color| div().size(px(14.)).rounded_full().bg(data_color(color))))
      .child(label)
      .child(icon(Icon::ChevronDown, 14., MUTED))
      .on_mouse_down(
        MouseButton::Left,
        Self::choose(choices, self.color_id, |this, color| this.color_id = color, cx),
      )
  }

  fn reminder_picker(&self, cx: &Context<Self>) -> Stateful<Div> {
    let mut choices: Vec<(SharedString, Reminder)> = vec![
      (reminder_label(Reminder::Default).into(), Reminder::Default),
      (reminder_label(Reminder::None).into(), Reminder::None),
    ];
    let mut minutes = REMINDERS.to_vec();
    // An event keeps a notification set elsewhere that is not one of these.
    if let Reminder::Minutes(held) = self.reminder
      && !minutes.contains(&held)
    {
      minutes.push(held);
      minutes.sort_unstable();
    }
    choices.extend(minutes.into_iter().map(|minutes| {
      let reminder = Reminder::Minutes(minutes);
      (reminder_label(reminder).into(), reminder)
    }));
    picker("event-reminder", reminder_label(self.reminder)).on_mouse_down(
      MouseButton::Left,
      Self::choose(choices, self.reminder, |this, reminder| this.reminder = reminder, cx),
    )
  }

  fn busy_picker(&self, cx: &Context<Self>) -> Stateful<Div> {
    let choices = vec![("Busy".into(), true), ("Free".into(), false)];
    picker("event-busy", if self.busy { "Busy" } else { "Free" }).on_mouse_down(
      MouseButton::Left,
      Self::choose(choices, self.busy, |this, busy| this.busy = busy, cx),
    )
  }

  fn visibility_picker(&self, cx: &Context<Self>) -> Stateful<Div> {
    let label = |visibility: Visibility| match visibility {
      Visibility::Default => "Default visibility",
      Visibility::Public => "Public",
      Visibility::Private => "Private",
    };
    let choices = [Visibility::Default, Visibility::Public, Visibility::Private]
      .into_iter()
      .map(|visibility| (label(visibility).into(), visibility))
      .collect();
    picker("event-visibility", label(self.visibility)).on_mouse_down(
      MouseButton::Left,
      Self::choose(choices, self.visibility, |this, visibility| this.visibility = visibility, cx),
    )
  }

  // Rows.

  /// Event, Out of office, or Focus time: what a new event is.
  fn kinds(&self, cx: &Context<Self>) -> Div {
    let chip = |id: &'static str, label: &'static str, kind: EventKind| {
      let on = self.kind == kind;
      div()
        .id(id)
        .h(px(30.))
        .px_3()
        .flex()
        .items_center()
        .rounded(px(8.))
        .text_size(px(13.))
        .cursor_pointer()
        .when(on, |chip| chip.bg(rgb(ACCENT_BG)).text_color(rgb(ACCENT)))
        .when(!on, |chip| chip.text_color(rgb(TEXT)).hover(|style| style.bg(rgb(ROW_HOVER))))
        .on_click(cx.listener(move |this, _, window, cx| this.set_kind(kind, window, cx)))
        .child(label)
    };
    h_flex()
      .pl(px(29.))
      .gap_1()
      .child(chip("kind-event", "Event", EventKind::Event))
      .child(chip("kind-away", "Out of office", EventKind::OutOfOffice))
      .child(chip("kind-focus", "Focus time", EventKind::FocusTime))
  }

  fn when_row(&self, cx: &Context<Self>) -> Div {
    let dash = || div().text_color(rgb(MUTED)).child("–");
    let date = |state: &Entity<DatePickerState>| div().w(px(164.)).child(DatePicker::new(state));
    let times = if self.all_day {
      h_flex()
        .gap_2()
        .items_center()
        .child(date(&self.start_date))
        .child(dash())
        .child(date(&self.end_date))
    } else {
      h_flex()
        .gap_2()
        .items_center()
        .flex_wrap()
        .child(date(&self.start_date))
        .child(TimeField::new(&self.start_time))
        .child(dash())
        .child(TimeField::new(&self.end_time))
        // The second date shows once the event runs into another day.
        .when(self.end.date() != self.start.date(), |times| times.child(date(&self.end_date)))
    };
    let all_day = self.all_day;
    // An event that already repeats keeps its rule, so there is none to pick.
    let can_repeat = self.editing.as_ref().is_none_or(|(event, _)| event.series.is_none());
    let options = h_flex()
      .gap_3()
      .items_center()
      .when(self.plain(), |options| {
        options.child(
          h_flex()
            .id("event-all-day")
            .h(px(34.))
            .gap_2()
            .items_center()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| this.set_all_day(!all_day, window, cx)))
            .child(switch(all_day, 16.))
            .child(div().text_size(px(13.)).text_color(rgb(TEXT)).child("All day")),
        )
      })
      .when(can_repeat, |options| options.child(self.repeat_picker(cx)));
    row(Icon::Clock).child(v_flex().flex_1().min_w_0().gap_2().child(times).child(options))
  }

  fn meet_row(&self, cx: &Context<Self>) -> Div {
    let held = self
      .editing
      .as_ref()
      .and_then(|(event, _)| event.meet_url.clone())
      .map(|url| url.trim_start_matches("https://").to_string());
    let content = if let Some(url) = held {
      div()
        .h(px(34.))
        .flex()
        .items_center()
        .text_size(px(13.))
        .text_color(rgb(TEXT))
        .child(url)
        .into_any_element()
    } else {
      let on = self.add_meet;
      h_flex()
        .id("event-meet")
        .h(px(34.))
        .gap_2()
        .items_center()
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| {
          this.add_meet = !on;
          cx.notify();
        }))
        .child(switch(on, 16.))
        .child(
          div()
            .text_size(px(13.))
            .text_color(rgb(TEXT))
            .child("Add Google Meet video conferencing"),
        )
        .into_any_element()
    };
    row(Icon::Video).child(content)
  }

  fn footer(&self, cx: &Context<Self>) -> Div {
    let notify = self.notify;
    let label = if self.saving { "Saving…" } else { "Save" };
    h_flex()
      .h(px(56.))
      .px_4()
      .gap_2()
      .flex_shrink_0()
      .items_center()
      .border_t_1()
      .border_color(rgb(BORDER))
      .child(
        div()
          .flex_1()
          .min_w_0()
          .truncate()
          .text_size(px(12.))
          .text_color(rgb(RED))
          .children(self.error.clone()),
      )
      .when(!self.guests.is_empty() && self.plain(), |footer| {
        footer.child(
          h_flex()
            .id("event-notify")
            .gap_2()
            .pr_2()
            .items_center()
            .cursor_pointer()
            .tooltip(hint("Send the guests an email about this"))
            .on_click(cx.listener(move |this, _, _, cx| {
              this.notify = !notify;
              cx.notify();
            }))
            .child(switch(notify, 16.))
            .child(div().text_size(px(13.)).text_color(rgb(TEXT)).child("Email guests")),
        )
      })
      .child(
        div()
          .id("event-cancel")
          .h(px(32.))
          .px_3()
          .flex()
          .items_center()
          .rounded_full()
          .text_size(px(13.))
          .text_color(rgb(TEXT))
          .cursor_pointer()
          .hover(|style| style.bg(rgb(ROW_HOVER)))
          .on_click(cx.listener(|_, _, _, cx| cx.emit(EditorEvent::Close)))
          .child("Cancel"),
      )
      .child(
        div()
          .id("event-save")
          .h(px(32.))
          .px_4()
          .flex()
          .items_center()
          .rounded_full()
          .bg(rgb(ACCENT))
          .text_size(px(13.))
          .font_weight(FontWeight::MEDIUM)
          .text_color(rgb(BG))
          .when(self.saving, |save| save.opacity(0.6))
          .when(!self.saving, |save| {
            save
              .cursor_pointer()
              .hover(|style| style.opacity(0.9))
              .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
          })
          .child(label),
      )
  }
}

impl Render for EventEditor {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let calendars = self.store.read(cx).calendar.calendars.clone();
    let heading = match &self.editing {
      None => "New event",
      Some((_, EditScope::All)) => "Edit all events",
      Some((event, _)) if event.series.is_some() => "Edit this event",
      Some(_) => "Edit event",
    };
    let plain = self.plain();
    let fields = v_flex()
      .id("event-fields")
      .flex_1()
      .min_h_0()
      .px_4()
      .pb_4()
      .gap_3()
      .overflow_y_scroll()
      .child(
        div().pl(px(29.)).child(
          div()
            .h(px(44.))
            .flex()
            .items_center()
            .border_b_2()
            .border_color(rgb(ACCENT))
            .text_size(px(20.))
            .child(Input::new(&self.title).appearance(false).bordered(false)),
        ),
      )
      .when(self.editing.is_none(), |fields| fields.child(self.kinds(cx)))
      .child(self.when_row(cx))
      .when(plain, |fields| {
        fields.child(self.guests_row(cx)).child(self.meet_row(cx)).child(
          row(Icon::MapPin).child(
            field()
              .flex_1()
              .child(Input::new(&self.location).appearance(false).bordered(false)),
          ),
        )
      })
      .child(
        row(Icon::Text).child(
          field()
            .flex_1()
            .py_1()
            .child(Textarea::new(&self.description).appearance(false).bordered(false)),
        ),
      )
      .child(
        row(Icon::Calendar).child(
          h_flex()
            .flex_1()
            .gap_2()
            .flex_wrap()
            .when(plain, |pickers| pickers.child(self.calendar_picker(&calendars, cx)))
            .child(self.color_picker(&calendars, cx))
            .child(self.busy_picker(cx))
            .child(self.visibility_picker(cx)),
        ),
      )
      .child(row(Icon::Bell).child(self.reminder_picker(cx)));

    div()
      .absolute()
      .top_0()
      .left_0()
      .size_full()
      .flex()
      .justify_center()
      .items_start()
      .pt(px(64.))
      .bg(rgba(0x0000_0073))
      .occlude()
      .key_context("EventEditor")
      .on_action(cx.listener(|_, _: &Escape, _, cx| {
        cx.emit(EditorEvent::Close);
        cx.stop_propagation();
      }))
      .child(
        v_flex()
          .w(px(WIDTH))
          .max_h(window.viewport_size().height - px(96.))
          .rounded(px(14.))
          .bg(rgb(POPOVER))
          .border_1()
          .border_color(rgb(BORDER))
          .shadow_lg()
          .overflow_hidden()
          .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
            let stroke = &event.keystroke;
            if stroke.key == "enter" && stroke.modifiers.platform {
              this.save(cx);
              cx.stop_propagation();
            }
          }))
          .child(
            h_flex()
              .h(px(48.))
              .pl_4()
              .pr_2()
              .flex_shrink_0()
              .items_center()
              .child(
                div()
                  .flex_1()
                  .text_size(px(13.))
                  .font_weight(FontWeight::MEDIUM)
                  .text_color(rgb(MUTED))
                  .child(heading),
              )
              .child(
                div()
                  .id("event-editor-close")
                  .size(px(30.))
                  .flex()
                  .items_center()
                  .justify_center()
                  .rounded_full()
                  .cursor_pointer()
                  .hover(|style| style.bg(rgb(ROW_HOVER)))
                  .on_click(cx.listener(|_, _, _, cx| cx.emit(EditorEvent::Close)))
                  .child(icon(Icon::X, 16., TEXT)),
              ),
          )
          .child(fields)
          .child(self.footer(cx)),
      )
  }
}

#[cfg(test)]
mod tests {
  use super::{is_email, reminder_label};
  use gchat_core::Reminder;

  #[test]
  fn guests_need_a_whole_address() {
    assert!(is_email("jpark@example.com"));
    assert!(!is_email("jpark@example"));
    assert!(!is_email("@example.com"));
    assert!(!is_email("jordan park@example.com"));
    assert!(!is_email("jordan"));
  }

  #[test]
  fn reminders_read_in_the_largest_unit() {
    assert_eq!(reminder_label(Reminder::Minutes(10)), "10 minutes before");
    assert_eq!(reminder_label(Reminder::Minutes(60)), "1 hour before");
    assert_eq!(reminder_label(Reminder::Minutes(1440)), "1 day before");
  }
}
