//! Calendar date and layout math: which days a view shows, where events sit in a day column or a row of days, and
//! how times read. No GPUI here.

use std::collections::HashSet;

use chrono::{DateTime, Datelike, Days, Local, Months, NaiveDate, TimeZone, Timelike};
use gchat_core::{CalendarEvent, Micros, boundary_date, local_midnight};
use serde::{Deserialize, Serialize};

pub const MINUTE: Micros = 60_000_000;
pub const HOUR: Micros = 60 * MINUTE;
pub const DAY: Micros = 24 * HOUR;
/// Minutes in a day column.
pub const DAY_MINUTES: u16 = 24 * 60;
/// Days the Schedule view lists from its first one.
const SCHEDULE_DAYS: u64 = 62;
/// A block is drawn at least this long, so a short event still shows its title.
const MIN_BLOCK_MINUTES: u16 = 25;

/// How the calendar lays out time, as Google Calendar's views.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
  Day,
  #[default]
  Week,
  Month,
  Year,
  /// A list of the coming events, by day.
  Schedule,
  FourDays,
}

impl Mode {
  pub const ALL: [Self; 6] = [Self::Day, Self::Week, Self::Month, Self::Year, Self::Schedule, Self::FourDays];

  pub const fn label(self) -> &'static str {
    match self {
      Self::Day => "Day",
      Self::Week => "Week",
      Self::Month => "Month",
      Self::Year => "Year",
      Self::Schedule => "Schedule",
      Self::FourDays => "4 days",
    }
  }

  /// The key that switches to this view, as in Google Calendar.
  pub const fn key(self) -> &'static str {
    match self {
      Self::Day => "d",
      Self::Week => "w",
      Self::Month => "m",
      Self::Year => "y",
      Self::Schedule => "a",
      Self::FourDays => "x",
    }
  }

  /// Day, 4 days, and Week draw hours down the side.
  pub const fn has_hours(self) -> bool {
    matches!(self, Self::Day | Self::Week | Self::FourDays)
  }
}

/// The whole part of a positive measure, for counting lanes and minutes from pixels.
#[expect(
  clippy::as_conversions,
  clippy::cast_possible_truncation,
  clippy::cast_sign_loss,
  reason = "a pixel measure clamped to what the type holds"
)]
pub fn whole(value: f32) -> u16 {
  value.clamp(0., f32::from(u16::MAX)) as u16
}

pub fn today() -> NaiveDate {
  Local::now().date_naive()
}

pub fn now_micros() -> Micros {
  Local::now().timestamp_micros()
}

pub fn local(time: Micros) -> DateTime<Local> {
  Local.timestamp_micros(time).single().unwrap_or_else(Local::now)
}

pub fn date_of(time: Micros) -> NaiveDate {
  local(time).date_naive()
}

/// Local midnight at the start of `date`.
pub fn day_start(date: NaiveDate) -> Micros {
  local_midnight(date).unwrap_or_default()
}

pub fn add_days(date: NaiveDate, days: u64) -> NaiveDate {
  date.checked_add_days(Days::new(days)).unwrap_or(date)
}

pub fn sub_days(date: NaiveDate, days: u64) -> NaiveDate {
  date.checked_sub_days(Days::new(days)).unwrap_or(date)
}

/// The Sunday on or before `date`.
pub fn week_start(date: NaiveDate) -> NaiveDate {
  sub_days(date, u64::from(date.weekday().num_days_from_sunday()))
}

pub fn month_start(date: NaiveDate) -> NaiveDate {
  date.with_day(1).unwrap_or(date)
}

/// `date` moved by whole months, on the same day when that month has it.
pub fn add_months(date: NaiveDate, months: u32, forward: bool) -> NaiveDate {
  let moved = if forward {
    date.checked_add_months(Months::new(months))
  } else {
    date.checked_sub_months(Months::new(months))
  };
  moved.unwrap_or(date)
}

/// The weeks a month's grid shows: whole weeks from the Sunday on or before the 1st through the month's last day.
pub fn month_weeks(anchor: NaiveDate) -> Vec<Vec<NaiveDate>> {
  let first = month_start(anchor);
  let next = add_months(first, 1, true);
  let mut weeks = Vec::new();
  let mut day = week_start(first);
  while day < next {
    weeks.push((0..7).map(|offset| add_days(day, offset)).collect());
    day = add_days(day, 7);
  }
  weeks
}

/// The days a view shows, in order. The Year view answers its whole year.
pub fn days(mode: Mode, anchor: NaiveDate) -> Vec<NaiveDate> {
  let run = |first: NaiveDate, count: u64| (0..count).map(|offset| add_days(first, offset)).collect();
  match mode {
    Mode::Day => vec![anchor],
    Mode::FourDays => run(anchor, 4),
    Mode::Week => run(week_start(anchor), 7),
    Mode::Month => month_weeks(anchor).into_iter().flatten().collect(),
    Mode::Schedule => run(anchor, SCHEDULE_DAYS),
    Mode::Year => {
      let first = NaiveDate::from_ymd_opt(anchor.year(), 1, 1).unwrap_or(anchor);
      let count = if first.leap_year() { 366 } else { 365 };
      run(first, count)
    },
  }
}

/// The time a view covers: from the start of its first day to the end of its last.
pub fn range(mode: Mode, anchor: NaiveDate) -> (Micros, Micros) {
  let days = days(mode, anchor);
  let first = days.first().copied().unwrap_or(anchor);
  let last = days.last().copied().unwrap_or(anchor);
  (day_start(first), day_start(add_days(last, 1)))
}

/// The date the previous or next button goes to.
pub fn step(mode: Mode, anchor: NaiveDate, forward: bool) -> NaiveDate {
  let by_days = |days: u64| {
    if forward {
      add_days(anchor, days)
    } else {
      sub_days(anchor, days)
    }
  };
  match mode {
    Mode::Day => by_days(1),
    Mode::FourDays => by_days(4),
    Mode::Week => by_days(7),
    Mode::Month | Mode::Schedule => add_months(month_start(anchor), 1, forward),
    Mode::Year => add_months(anchor, 12, forward),
  }
}

/// `November 2026`, or `Nov – Dec 2026` and `Dec 2026 – Jan 2027` for days in two months.
fn months_title(first: NaiveDate, last: NaiveDate) -> String {
  if (first.year(), first.month()) == (last.year(), last.month()) {
    first.format("%B %Y").to_string()
  } else if first.year() == last.year() {
    format!("{} – {}", first.format("%b"), last.format("%b %Y"))
  } else {
    format!("{} – {}", first.format("%b %Y"), last.format("%b %Y"))
  }
}

/// What the toolbar names the time on screen.
pub fn title(mode: Mode, anchor: NaiveDate) -> String {
  match mode {
    Mode::Day => anchor.format("%B %-d, %Y").to_string(),
    Mode::Month => anchor.format("%B %Y").to_string(),
    Mode::Year => anchor.format("%Y").to_string(),
    Mode::Week | Mode::FourDays | Mode::Schedule => {
      let days = days(mode, anchor);
      months_title(days.first().copied().unwrap_or(anchor), days.last().copied().unwrap_or(anchor))
    },
  }
}

/// Whether an event goes in the all-day strip: it is all-day, or runs for a day or more.
pub const fn is_banner(event: &CalendarEvent) -> bool {
  event.all_day || event.end - event.start >= DAY
}

/// The first and the last day an event touches. An event that ends at midnight does not touch the day after.
pub fn event_days(event: &CalendarEvent) -> (NaiveDate, NaiveDate) {
  if event.all_day {
    let first = boundary_date(event.start).unwrap_or_else(|| date_of(event.start));
    let after = boundary_date(event.end).unwrap_or(first);
    return (first, sub_days(after, 1).max(first));
  }
  let first = date_of(event.start);
  (first, date_of(event.end - 1).max(first))
}

/// A timed event in a day column: its minutes of that day, and which of how many side-by-side columns it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
  /// Index into the events given.
  pub event: usize,
  pub start: u16,
  /// At least `MIN_BLOCK_MINUTES` after `start`.
  pub end: u16,
  pub column: u16,
  pub columns: u16,
}

fn minute_of(time: Micros, day: Micros) -> u16 {
  let minutes = ((time - day) / MINUTE).clamp(0, i64::from(DAY_MINUTES));
  u16::try_from(minutes).unwrap_or(DAY_MINUTES)
}

/// The timed events of `day`, laid out so that overlapping ones sit side by side.
pub fn day_blocks(events: &[CalendarEvent], day: NaiveDate) -> Vec<Block> {
  let (from, until) = (day_start(day), day_start(add_days(day, 1)));
  let mut blocks: Vec<Block> = events
    .iter()
    .enumerate()
    .filter(|(_, event)| !is_banner(event) && event.start < until && event.end.max(event.start + 1) > from)
    .map(|(index, event)| {
      let start = minute_of(event.start, from).min(DAY_MINUTES - MIN_BLOCK_MINUTES);
      let end = minute_of(event.end, from).max(start + MIN_BLOCK_MINUTES);
      Block {
        event: index,
        start,
        end,
        column: 0,
        columns: 1,
      }
    })
    .collect();
  blocks.sort_by_key(|block| (block.start, std::cmp::Reverse(block.end)));

  // A cluster is a run of blocks that overlap one another in a chain; its blocks share the column count.
  let mut cluster_from = 0;
  let mut cluster_end = 0;
  let mut column_ends: Vec<u16> = Vec::new();
  let close = |blocks: &mut [Block], from: usize, columns: usize| {
    let columns = u16::try_from(columns).unwrap_or(u16::MAX).max(1);
    for block in blocks.iter_mut().skip(from) {
      block.columns = columns;
    }
  };
  for index in 0..blocks.len() {
    let Some(block) = blocks.get(index).copied() else {
      break;
    };
    if block.start >= cluster_end && index > cluster_from {
      let (done, _) = blocks.split_at_mut(index);
      close(done, cluster_from, column_ends.len());
      cluster_from = index;
      column_ends.clear();
    }
    let free = column_ends.iter().position(|end| *end <= block.start);
    let column = free.unwrap_or(column_ends.len());
    match column_ends.get_mut(column) {
      Some(end) => *end = block.end,
      None => column_ends.push(block.end),
    }
    if let Some(held) = blocks.get_mut(index) {
      held.column = u16::try_from(column).unwrap_or(u16::MAX);
    }
    cluster_end = cluster_end.max(block.end);
  }
  close(&mut blocks, cluster_from, column_ends.len());
  blocks
}

/// An event across consecutive days of one row: the day columns it covers and the lane it sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
  /// Index into the events given.
  pub event: usize,
  pub first: u8,
  pub last: u8,
  pub lane: u16,
  /// The event starts before this run, or goes on after it.
  pub open_start: bool,
  pub open_end: bool,
  /// Drawn as a filled bar (an all-day strip event) rather than as a line with a time.
  pub banner: bool,
}

/// A row of days with its events placed in lanes.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Row {
  pub runs: Vec<Run>,
  /// Per day, how many events did not fit.
  pub more: Vec<u16>,
  /// Lanes in use, counting the one that says how many more.
  pub lanes: u16,
}

/// Places the events of a row of consecutive `days` in at most `lanes` lanes. With `timed` off, only the all-day
/// strip's events are placed. A day with more events than lanes keeps its last lane to say how many more.
pub fn row(events: &[CalendarEvent], days: &[NaiveDate], lanes: u16, timed: bool) -> Row {
  let (Some(first_day), Some(last_day)) = (days.first().copied(), days.last().copied()) else {
    return Row::default();
  };
  let column = |date: NaiveDate| {
    let offset = (date.max(first_day) - first_day).num_days();
    u8::try_from(offset).unwrap_or(u8::MAX)
  };
  // Bars first, the longer ones on top, then the timed events in order.
  let mut picks: Vec<(usize, NaiveDate, NaiveDate, bool)> = events
    .iter()
    .enumerate()
    .filter(|(_, event)| timed || is_banner(event))
    .map(|(index, event)| {
      let (first, last) = event_days(event);
      (index, first, last, is_banner(event))
    })
    .filter(|(_, first, last, _)| *first <= last_day && *last >= first_day)
    .collect();
  picks.sort_by_key(|(index, first, last, banner)| (!banner, *first, std::cmp::Reverse(*last), *index));

  let mut taken: Vec<Vec<bool>> = vec![Vec::new(); days.len()];
  let mut placed: Vec<Run> = Vec::new();
  for (event, first, last, banner) in picks {
    let (from, to) = (column(first), column(last.min(last_day)));
    let span = usize::from(from)..=usize::from(to);
    let free = |lane: usize| {
      taken
        .iter()
        .enumerate()
        .filter(|(day, _)| span.contains(day))
        .all(|(_, lanes)| !lanes.get(lane).copied().unwrap_or(false))
    };
    // Every lane in use is below this length, so the one at it is free.
    let deepest = taken.iter().map(Vec::len).max().unwrap_or(0);
    let lane = (0..=deepest).find(|lane| free(*lane)).unwrap_or(deepest);
    for lanes in taken
      .iter_mut()
      .enumerate()
      .filter(|(day, _)| span.contains(day))
      .map(|(_, l)| l)
    {
      if lanes.len() <= lane {
        lanes.resize(lane + 1, false);
      }
      if let Some(slot) = lanes.get_mut(lane) {
        *slot = true;
      }
    }
    placed.push(Run {
      event,
      first: from,
      last: to,
      lane: u16::try_from(lane).unwrap_or(u16::MAX),
      open_start: first < first_day,
      open_end: last > last_day,
      banner,
    });
  }

  // A day that needs more lanes than there are shows one fewer, and counts the rest.
  let limits: Vec<u16> = taken
    .iter()
    .map(|day| {
      if day.len() > usize::from(lanes) {
        lanes.saturating_sub(1)
      } else {
        lanes
      }
    })
    .collect();
  let limit = |day: u8| limits.get(usize::from(day)).copied().unwrap_or(lanes);
  let mut out = Row {
    runs: Vec::new(),
    more: vec![0; days.len()],
    lanes: 0,
  };
  for run in placed {
    // Split into the stretches of days where its lane shows.
    let mut open: Option<u8> = None;
    for day in run.first..=run.last {
      let shown = run.lane < limit(day);
      if !shown && let Some(count) = out.more.get_mut(usize::from(day)) {
        *count += 1;
      }
      match (shown, open) {
        (true, None) => open = Some(day),
        (false, Some(from)) => {
          out.runs.push(Run {
            first: from,
            last: day - 1,
            open_start: run.open_start || from > run.first,
            open_end: true,
            ..run
          });
          open = None;
        },
        _ => {},
      }
    }
    if let Some(from) = open {
      out.runs.push(Run {
        first: from,
        open_start: run.open_start || from > run.first,
        ..run
      });
    }
  }
  let used = taken.iter().map(Vec::len).max().unwrap_or(0);
  out.lanes = u16::try_from(used).unwrap_or(u16::MAX).min(lanes);
  out
}

/// The days of `days` that have an event, for the Year view's marks.
pub fn busy_days(events: &[CalendarEvent]) -> HashSet<NaiveDate> {
  let mut busy = HashSet::new();
  for event in events {
    let (first, last) = event_days(event);
    let mut day = first;
    while day <= last {
      busy.insert(day);
      day = add_days(day, 1);
    }
  }
  busy
}

/// The events of each day from `first`, for the Schedule view: only days that have one, an event that spans days on
/// each of them, all-day ones first.
pub fn agenda(events: &[CalendarEvent], days: &[NaiveDate]) -> Vec<(NaiveDate, Vec<usize>)> {
  days
    .iter()
    .filter_map(|day| {
      let mut held: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| {
          let (first, last) = event_days(event);
          first <= *day && last >= *day
        })
        .map(|(index, _)| index)
        .collect();
      held.sort_by_key(|index| events.get(*index).map(|event| (!is_banner(event), event.start)));
      (!held.is_empty()).then_some((*day, held))
    })
    .collect()
}

/// `7am`, `11:30am`, `12pm`: a clock time as Google Calendar writes it.
pub fn clock(time: Micros) -> String {
  let at = local(time);
  if at.minute() == 0 {
    at.format("%-I%P").to_string()
  } else {
    at.format("%-I:%M%P").to_string()
  }
}

/// `7 – 7:30am`, `11:30am – 12:30pm`: a start and an end on one day, naming the half of the day once when they share
/// it.
pub fn clock_range(start: Micros, end: Micros) -> String {
  let (from, to) = (local(start), local(end));
  let same_half = (from.hour() < 12) == (to.hour() < 12);
  let first = clock(start);
  let first = if same_half {
    first.trim_end_matches("am").trim_end_matches("pm").to_string()
  } else {
    first
  };
  format!("{first} – {}", clock(end))
}

/// `12 AM`, `1 PM`: the label of an hour line.
pub fn hour_label(hour: u8) -> String {
  let (shown, half) = match hour {
    0 => (12, "AM"),
    1..=11 => (hour, "AM"),
    12 => (12, "PM"),
    _ => (hour - 12, "PM"),
  };
  format!("{shown} {half}")
}

/// When an event is, in words: `Friday, November 13 ⋅ 11:30am – 12:30pm`, `Thursday, November 26`,
/// `Nov 30 – Dec 2, 2026`.
pub fn when_line(event: &CalendarEvent) -> String {
  let (first, last) = event_days(event);
  if event.all_day {
    if first == last {
      return first.format("%A, %B %-d").to_string();
    }
    if (first.year(), first.month()) == (last.year(), last.month()) {
      return format!("{} – {}", first.format("%b %-d"), last.format("%-d, %Y"));
    }
    return format!("{} – {}", first.format("%b %-d"), last.format("%b %-d, %Y"));
  }
  if first == last {
    return format!("{} ⋅ {}", first.format("%A, %B %-d"), clock_range(event.start, event.end));
  }
  format!(
    "{}, {} – {}, {}",
    first.format("%b %-d"),
    clock(event.start),
    last.format("%b %-d"),
    clock(event.end)
  )
}

#[cfg(test)]
mod tests {
  use gchat_core::{EventKind, Reminder, Visibility};

  use super::*;

  fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("a date")
  }

  fn timed(day: NaiveDate, from: (i64, i64), to: (i64, i64)) -> CalendarEvent {
    let base = day_start(day);
    CalendarEvent {
      id: format!("e-{}-{}", from.0, from.1),
      calendar_id: "c".into(),
      title: "Sync".into(),
      start: base + from.0 * HOUR + from.1 * MINUTE,
      end: base + to.0 * HOUR + to.1 * MINUTE,
      all_day: false,
      location: String::new(),
      description: String::new(),
      meet_url: None,
      link: String::new(),
      color: None,
      color_id: None,
      guests: Vec::new(),
      organizer: String::new(),
      series: None,
      kind: EventKind::Event,
      busy: true,
      visibility: Visibility::Default,
      reminder: Reminder::Default,
      can_edit: true,
      can_delete: true,
      my_rsvp: None,
    }
  }

  fn whole(first: NaiveDate, last: NaiveDate) -> CalendarEvent {
    CalendarEvent {
      start: day_start(first),
      end: day_start(add_days(last, 1)),
      all_day: true,
      ..timed(first, (0, 0), (0, 0))
    }
  }

  #[test]
  fn weeks_start_on_sunday_and_cover_the_month() {
    // November 2026 starts on a Sunday and ends on a Monday: five weeks, the last one into December.
    let weeks = month_weeks(date(2026, 11, 13));
    assert_eq!(weeks.len(), 5);
    assert_eq!(weeks.first().and_then(|week| week.first()), Some(&date(2026, 11, 1)));
    assert_eq!(weeks.last().and_then(|week| week.last()), Some(&date(2026, 12, 5)));
    assert_eq!(week_start(date(2026, 11, 13)), date(2026, 11, 8));
    assert_eq!(month_weeks(date(2026, 8, 10)).len(), 6);
  }

  #[test]
  fn titles_name_one_or_two_months() {
    assert_eq!(title(Mode::Day, date(2026, 11, 13)), "November 13, 2026");
    assert_eq!(title(Mode::Week, date(2026, 11, 13)), "November 2026");
    assert_eq!(title(Mode::Week, date(2026, 12, 1)), "Nov – Dec 2026");
    assert_eq!(title(Mode::FourDays, date(2026, 12, 30)), "Dec 2026 – Jan 2027");
    assert_eq!(title(Mode::Year, date(2026, 11, 13)), "2026");
  }

  #[test]
  fn steps_follow_the_view() {
    assert_eq!(step(Mode::Week, date(2026, 11, 13), true), date(2026, 11, 20));
    assert_eq!(step(Mode::FourDays, date(2026, 11, 13), false), date(2026, 11, 9));
    assert_eq!(step(Mode::Month, date(2026, 1, 31), true), date(2026, 2, 1));
    assert_eq!(step(Mode::Year, date(2028, 2, 29), true), date(2029, 2, 28));
  }

  #[test]
  fn the_year_view_covers_the_whole_year() {
    assert_eq!(days(Mode::Year, date(2028, 5, 5)).len(), 366);
    let (start, end) = range(Mode::Year, date(2026, 5, 5));
    assert_eq!((date_of(start), date_of(end)), (date(2026, 1, 1), date(2027, 1, 1)));
  }

  #[test]
  fn overlapping_events_share_the_column() {
    let day = date(2026, 11, 13);
    let events = vec![
      timed(day, (9, 0), (10, 0)),
      timed(day, (9, 30), (10, 30)),
      timed(day, (10, 0), (11, 0)),
      timed(day, (14, 0), (15, 0)),
    ];
    let blocks = day_blocks(&events, day);
    let of = |event: usize| blocks.iter().find(|block| block.event == event).copied().expect("placed");
    assert_eq!((of(0).column, of(0).columns), (0, 2));
    assert_eq!((of(1).column, of(1).columns), (1, 2));
    // Starts as the first one ends: it takes that column again.
    assert_eq!((of(2).column, of(2).columns), (0, 2));
    assert_eq!((of(3).column, of(3).columns), (0, 1));
    assert_eq!((of(3).start, of(3).end), (14 * 60, 15 * 60));
  }

  #[test]
  fn short_and_late_events_keep_a_readable_block() {
    let day = date(2026, 11, 13);
    let blocks = day_blocks(&[timed(day, (9, 0), (9, 5)), timed(day, (23, 55), (24, 30))], day);
    assert_eq!(blocks.first().map(|block| block.end - block.start), Some(MIN_BLOCK_MINUTES));
    assert_eq!(blocks.get(1).map(|block| block.end), Some(DAY_MINUTES));
    // The part after midnight is on the next day.
    assert_eq!(day_blocks(&[timed(day, (23, 55), (24, 30))], add_days(day, 1)).len(), 1);
  }

  #[test]
  fn all_day_events_end_on_their_last_day() {
    let event = whole(date(2026, 11, 26), date(2026, 11, 27));
    assert_eq!(event_days(&event), (date(2026, 11, 26), date(2026, 11, 27)));
    let night = timed(date(2026, 11, 13), (22, 0), (24, 0));
    assert_eq!(event_days(&night), (date(2026, 11, 13), date(2026, 11, 13)));
    assert!(is_banner(&event) && !is_banner(&night));
  }

  #[test]
  fn rows_stack_bars_over_timed_events_and_count_the_rest() {
    let week = days(Mode::Week, date(2026, 11, 13));
    let monday = date(2026, 11, 9);
    let events = vec![
      timed(monday, (9, 0), (10, 0)),
      whole(date(2026, 11, 7), date(2026, 11, 10)),
      timed(monday, (11, 0), (12, 0)),
      timed(monday, (13, 0), (14, 0)),
    ];
    let placed = row(&events, &week, 3, true);
    let bar = placed.runs.iter().find(|run| run.event == 1).expect("the bar");
    assert_eq!((bar.first, bar.last, bar.lane), (0, 2, 0));
    assert!(bar.open_start && !bar.open_end && bar.banner);
    // Monday holds four things in three lanes: two show, and the last lane counts the other two.
    assert_eq!(placed.more, vec![0, 2, 0, 0, 0, 0, 0]);
    assert!(placed.runs.iter().any(|run| run.event == 0 && run.lane == 1));
    assert!(placed.runs.iter().all(|run| run.event != 2 && run.event != 3));
    assert_eq!(placed.lanes, 3);

    let strip = row(&events, &week, 4, false);
    assert_eq!(strip.runs.len(), 1);
    assert_eq!(strip.lanes, 1);
  }

  #[test]
  fn a_bar_hidden_on_a_crowded_day_shows_on_the_others() {
    let week = days(Mode::Week, date(2026, 11, 13));
    let tuesday = date(2026, 11, 10);
    let events = vec![
      whole(date(2026, 11, 9), date(2026, 11, 11)),
      whole(date(2026, 11, 9), date(2026, 11, 11)),
      whole(tuesday, tuesday),
    ];
    let placed = row(&events, &week, 2, true);
    let second: Vec<(u8, u8)> = placed
      .runs
      .iter()
      .filter(|run| run.event == 1)
      .map(|run| (run.first, run.last))
      .collect();
    assert_eq!(second, vec![(1, 1), (3, 3)]);
    assert_eq!(placed.more, vec![0, 0, 2, 0, 0, 0, 0]);
  }

  #[test]
  fn the_agenda_lists_days_with_events() {
    let first = date(2026, 11, 13);
    let events = vec![
      timed(add_days(first, 1), (9, 0), (10, 0)),
      whole(add_days(first, 1), add_days(first, 2)),
    ];
    let listed = agenda(&events, &days(Mode::Schedule, first));
    assert_eq!(listed.len(), 2);
    assert_eq!(listed.first(), Some(&(add_days(first, 1), vec![1, 0])));
    assert_eq!(busy_days(&events).len(), 2);
  }

  #[test]
  fn times_read_as_google_calendar_writes_them() {
    let day = date(2026, 11, 13);
    let morning = timed(day, (7, 0), (7, 30));
    assert_eq!(clock(morning.start), "7am");
    assert_eq!(clock_range(morning.start, morning.end), "7 – 7:30am");
    let noon = timed(day, (11, 30), (12, 30));
    assert_eq!(clock_range(noon.start, noon.end), "11:30am – 12:30pm");
    assert_eq!(when_line(&noon), "Friday, November 13 ⋅ 11:30am – 12:30pm");
    assert_eq!(when_line(&whole(date(2026, 11, 30), date(2026, 12, 2))), "Nov 30 – Dec 2, 2026");
    assert_eq!(when_line(&whole(day, day)), "Friday, November 13");
    assert_eq!(
      (hour_label(0), hour_label(12), hour_label(13)),
      ("12 AM".into(), "12 PM".into(), "1 PM".into())
    );
  }
}
