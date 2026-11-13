//! The demo's calendar: made-up calendars and events around today, kept in memory. Recurring events are held one
//! per occurrence, as Google Calendar answers them.

#![allow(clippy::unreadable_literal, reason = "colors are 0xRRGGBB")]

use std::collections::HashMap;

use chrono::{Datelike, Days, Local, NaiveDate, TimeZone, Weekday};

use crate::calendar::{event_color, local_midnight, sort_events};
use crate::model::{
  Calendar, CalendarAccess, CalendarEvent, EditScope, EventDraft, EventKind, Guest, Micros, Reminder, Repeat, Rsvp,
  User, Visibility,
};

/// The signed-in user's own calendar.
const OWN: &str = "amorgan@example.com";
const TEAM: &str = "demo-team-calendar";
const HOLIDAYS: &str = "demo-holidays";
/// Days before and after today that hold the recurring events.
const PAST_DAYS: u64 = 60;
const FUTURE_DAYS: u64 = 180;

pub struct DemoCalendar {
  pub calendars: Vec<Calendar>,
  events: Vec<CalendarEvent>,
  /// The range `Command::LoadEvents` last asked for.
  pub range: Option<(Micros, Micros)>,
  /// Names of the workspace's people, by email.
  names: HashMap<String, String>,
  me: User,
  serial: usize,
}

/// `hour:minute` on `date`, local time.
fn at(date: NaiveDate, hour: u32, minute: u32) -> Micros {
  let midnight = local_midnight(date).unwrap_or_default();
  date
    .and_hms_opt(hour, minute, 0)
    .and_then(|time| Local.from_local_datetime(&time).earliest())
    .map_or(midnight, |time| time.timestamp_micros())
}

fn date_of(time: Micros) -> NaiveDate {
  Local
    .timestamp_micros(time)
    .single()
    .map(|time| time.date_naive())
    .unwrap_or_default()
}

fn event(id: &str, calendar_id: &str, title: &str, start: Micros, end: Micros) -> CalendarEvent {
  CalendarEvent {
    id: id.into(),
    calendar_id: calendar_id.into(),
    title: title.into(),
    start,
    end,
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

/// A whole-day event from `first` through `last`.
fn all_day(id: &str, calendar_id: &str, title: &str, first: NaiveDate, last: NaiveDate) -> CalendarEvent {
  let after = last.checked_add_days(Days::new(1)).unwrap_or(last);
  let start = local_midnight(first).unwrap_or_default();
  let end = local_midnight(after).unwrap_or(start);
  CalendarEvent {
    all_day: true,
    busy: false,
    ..event(id, calendar_id, title, start, end)
  }
}

impl DemoCalendar {
  pub fn new(now: Micros, users: &[User], me: &User) -> Self {
    let mut calendar = Self {
      calendars: vec![
        Calendar {
          id: OWN.into(),
          name: me.name.clone(),
          color: 0x039be5,
          access: CalendarAccess::Owner,
          primary: true,
          selected: true,
        },
        Calendar {
          id: TEAM.into(),
          name: "Team offsites".into(),
          color: 0x33b679,
          access: CalendarAccess::Writer,
          primary: false,
          selected: true,
        },
        Calendar {
          id: HOLIDAYS.into(),
          name: "Holidays".into(),
          color: 0xf4511e,
          access: CalendarAccess::Reader,
          primary: false,
          selected: true,
        },
      ],
      events: Vec::new(),
      range: None,
      names: users
        .iter()
        .filter(|user| !user.email.is_empty())
        .map(|user| (user.email.clone(), user.name.clone()))
        .collect(),
      me: me.clone(),
      serial: 0,
    };
    calendar.fill(date_of(now));
    calendar
  }

  fn guest(&self, email: &str, rsvp: Rsvp, organizer: bool) -> Guest {
    Guest {
      email: email.into(),
      name: self.names.get(email).cloned().unwrap_or_default(),
      rsvp,
      organizer,
      me: email == self.me.email,
      optional: false,
    }
  }

  /// An event someone else organizes and the user is invited to.
  fn invited(&self, mut event: CalendarEvent, organizer: &str, others: &[&str], mine: Rsvp) -> CalendarEvent {
    event.guests = vec![self.guest(organizer, Rsvp::Accepted, true), self.guest(OWN, mine, false)];
    event
      .guests
      .extend(others.iter().map(|email| self.guest(email, Rsvp::Accepted, false)));
    event.organizer = self.names.get(organizer).cloned().unwrap_or_else(|| organizer.into());
    event.my_rsvp = Some(mine);
    event.can_edit = false;
    event
  }

  fn fill(&mut self, today: NaiveDate) {
    let first = today.checked_sub_days(Days::new(PAST_DAYS)).unwrap_or(today);
    for offset in 0..=PAST_DAYS + FUTURE_DAYS {
      let Some(day) = first.checked_add_days(Days::new(offset)) else {
        continue;
      };
      self.recurring(day, today);
      self.holidays(day);
    }
    let plus = |days: u64| today.checked_add_days(Days::new(days)).unwrap_or(today);
    let minus = |days: u64| today.checked_sub_days(Days::new(days)).unwrap_or(today);

    let mut launch = event("demo-launch", OWN, "Launch readiness", at(plus(1), 13, 0), at(plus(1), 14, 0));
    launch.meet_url = Some("https://meet.google.com/demo-launch".into());
    launch.color_id = Some(11);
    launch.color = event_color(11);
    launch.description = "Go or no-go for Thursday.\n\n1. Open blockers\n2. Rollback plan\n3. Announcement".into();
    launch.guests = vec![
      self.guest(OWN, Rsvp::Accepted, true),
      self.guest("jpark@example.com", Rsvp::Accepted, false),
      self.guest("praman@example.com", Rsvp::Tentative, false),
      self.guest("dalvarez@example.com", Rsvp::NeedsAction, false),
    ];
    launch.organizer.clone_from(&self.me.name);
    self.events.push(launch);

    let mut coffee = event("demo-coffee", OWN, "Coffee with Sam", at(today, 16, 0), at(today, 16, 30));
    coffee.location = "Ground floor café".into();
    coffee.guests = vec![
      self.guest(OWN, Rsvp::Accepted, true),
      self.guest("sokafor@example.com", Rsvp::Accepted, false),
    ];
    coffee.organizer.clone_from(&self.me.name);
    self.events.push(coffee);

    let pricing = event("demo-pricing", OWN, "Pricing review", at(minus(2), 10, 0), at(minus(2), 11, 30));
    self
      .events
      .push(self.invited(pricing, "mtanaka@example.com", &["jpark@example.com"], Rsvp::Accepted));

    let mut offsite = all_day("demo-offsite", TEAM, "Offsite planning", plus(6), plus(7));
    offsite.location = "Harbor room".into();
    self.events.push(offsite);

    let mut birthday = all_day("demo-birthday", OWN, "Noah Fischer's birthday", plus(9), plus(9));
    birthday.kind = EventKind::Birthday;
    birthday.can_edit = false;
    birthday.can_delete = false;
    self.events.push(birthday);

    let mut away = event("demo-away", OWN, "Out of office", at(plus(12), 13, 0), at(plus(12), 18, 0));
    away.kind = EventKind::OutOfOffice;
    self.events.push(away);
  }

  /// The events that repeat on `day`.
  fn recurring(&mut self, day: NaiveDate, today: NaiveDate) {
    let weekday = day.weekday();
    let key = day.format("%Y%m%d");
    if !matches!(weekday, Weekday::Sat | Weekday::Sun) {
      let mut standup = event(
        &format!("demo-standup_{key}"),
        OWN,
        "Daily standup",
        at(day, 9, 30),
        at(day, 9, 45),
      );
      standup.series = Some("demo-standup".into());
      standup.meet_url = Some("https://meet.google.com/demo-standup".into());
      let others = ["jpark@example.com", "sokafor@example.com", "praman@example.com"];
      self
        .events
        .push(self.invited(standup, "rbrooks@example.com", &others, Rsvp::Accepted));
    }
    if weekday == Weekday::Tue {
      let mut one = event(
        &format!("demo-one_{key}"),
        OWN,
        "1:1 with Jordan",
        at(day, 14, 0),
        at(day, 14, 30),
      );
      one.series = Some("demo-one".into());
      one.meet_url = Some("https://meet.google.com/demo-one".into());
      one.guests = vec![
        self.guest(OWN, Rsvp::Accepted, true),
        self.guest("jpark@example.com", Rsvp::Accepted, false),
      ];
      one.organizer.clone_from(&self.me.name);
      self.events.push(one);
    }
    if weekday == Weekday::Wed {
      let mut focus = event(&format!("demo-focus_{key}"), OWN, "Focus time", at(day, 15, 0), at(day, 17, 0));
      focus.series = Some("demo-focus".into());
      focus.kind = EventKind::FocusTime;
      self.events.push(focus);
    }
    if weekday == Weekday::Thu {
      let mut review = event(
        &format!("demo-review_{key}"),
        OWN,
        "Design review",
        at(day, 11, 0),
        at(day, 12, 0),
      );
      review.series = Some("demo-review".into());
      review.meet_url = Some("https://meet.google.com/demo-review".into());
      review.location = "Atlas room".into();
      // Past ones were answered; the coming ones still wait for an answer.
      let mine = if day < today {
        Rsvp::Accepted
      } else {
        Rsvp::NeedsAction
      };
      let others = ["mtanaka@example.com", "nfischer@example.com"];
      self.events.push(self.invited(review, "praman@example.com", &others, mine));
    }
  }

  fn holidays(&mut self, day: NaiveDate) {
    let name = match (day.month(), day.day()) {
      (1, 1) => "New Year's Day",
      (7, 4) => "Independence Day",
      (12, 25) => "Christmas Day",
      // The fourth Thursday of November.
      (11, 22..=28) if day.weekday() == Weekday::Thu => "Thanksgiving Day",
      _ => return,
    };
    let mut holiday = all_day(&format!("demo-holiday_{}", day.format("%Y%m%d")), HOLIDAYS, name, day, day);
    holiday.can_edit = false;
    holiday.can_delete = false;
    self.events.push(holiday);
  }

  /// The events that overlap `start..end`, by start.
  pub fn events(&self, start: Micros, end: Micros) -> Vec<CalendarEvent> {
    let mut events: Vec<CalendarEvent> = self
      .events
      .iter()
      .filter(|event| event.start < end && event.end > start)
      .cloned()
      .collect();
    sort_events(&mut events);
    events
  }

  fn next_id(&mut self, prefix: &str) -> String {
    self.serial += 1;
    format!("demo-{prefix}-{}", self.serial)
  }

  /// Writes the fields of `draft` onto `event`, moved by `shift` from the draft's own times.
  fn apply(&self, event: &mut CalendarEvent, draft: &EventDraft, shift: Micros) {
    event.calendar_id.clone_from(&draft.calendar_id);
    event.title = if draft.title.trim().is_empty() {
      "(No title)".into()
    } else {
      draft.title.trim().to_string()
    };
    event.start = draft.start + shift;
    event.end = draft.end.max(draft.start) + shift;
    event.all_day = draft.all_day;
    event.location = draft.location.trim().to_string();
    event.description = draft.description.trim().to_string();
    event.busy = draft.busy;
    event.visibility = draft.visibility;
    event.reminder = draft.reminder;
    event.color_id = draft.color_id;
    event.color = draft.color_id.and_then(event_color);
    if draft.add_meet && event.meet_url.is_none() {
      event.meet_url = Some(format!("https://meet.google.com/demo-{}", self.serial));
    }
    let held = std::mem::take(&mut event.guests);
    if !draft.guests.is_empty() {
      let keep = |email: &str| held.iter().find(|guest| guest.email == email).cloned();
      event.guests = vec![keep(OWN).unwrap_or_else(|| self.guest(OWN, Rsvp::Accepted, true))];
      for email in &draft.guests {
        if email != OWN {
          let guest = keep(email).unwrap_or_else(|| self.guest(email, Rsvp::NeedsAction, false));
          event.guests.push(guest);
        }
      }
    }
    if event.organizer.is_empty() {
      event.organizer.clone_from(&self.me.name);
    }
  }

  /// Creates the event of `draft`, or changes the one it edits. `Err` when that event is gone.
  pub fn save(&mut self, draft: &EventDraft) -> Result<(), String> {
    let Some((_, event_id)) = &draft.editing else {
      self.create(draft);
      return Ok(());
    };
    let Some(picked) = self.events.iter().find(|event| event.id == *event_id).cloned() else {
      return Err("That event no longer exists.".into());
    };
    let series = picked.series.clone().filter(|_| draft.scope == EditScope::All);
    let mut events = std::mem::take(&mut self.events);
    for event in &mut events {
      let same_series = series.is_some() && event.series == series;
      if event.id == picked.id || same_series {
        // Every occurrence moves by what the picked one moved.
        let shift = event.start - picked.start;
        self.apply(event, draft, shift);
      }
    }
    self.events = events;
    Ok(())
  }

  fn create(&mut self, draft: &EventDraft) {
    let repeat = draft.repeat.unwrap_or_default();
    let first = date_of(draft.start);
    let series = (repeat != Repeat::Never).then(|| self.next_id("series"));
    let days = if repeat == Repeat::Never {
      0
    } else {
      FUTURE_DAYS
    };
    for offset in 0..=days {
      let Some(day) = first.checked_add_days(Days::new(offset)) else {
        continue;
      };
      let on = match repeat {
        Repeat::Never => offset == 0,
        Repeat::Daily => true,
        Repeat::Weekly => day.weekday() == first.weekday(),
        Repeat::Monthly => day.day() == first.day(),
        Repeat::Yearly => day.month() == first.month() && day.day() == first.day(),
        Repeat::Weekdays => !matches!(day.weekday(), Weekday::Sat | Weekday::Sun),
      };
      if !on {
        continue;
      }
      let id = self.next_id("event");
      let mut created = event(&id, &draft.calendar_id, "", 0, 0);
      created.kind = draft.kind;
      created.series.clone_from(&series);
      // Whole days apart, so a clock change in between keeps the time of day.
      let shift = local_midnight(day).unwrap_or_default() - local_midnight(first).unwrap_or_default();
      self.apply(&mut created, draft, shift);
      self.events.push(created);
    }
  }

  /// The ids a change with `scope` on `event_id` reaches: the event, or every occurrence of its series.
  fn reach(&self, event_id: &str, scope: EditScope) -> Vec<String> {
    let Some(picked) = self.events.iter().find(|event| event.id == event_id) else {
      return Vec::new();
    };
    match (&picked.series, scope) {
      (Some(series), EditScope::All) => self
        .events
        .iter()
        .filter(|event| event.series.as_ref() == Some(series))
        .map(|event| event.id.clone())
        .collect(),
      _ => vec![picked.id.clone()],
    }
  }

  pub fn delete(&mut self, event_id: &str, scope: EditScope) -> Result<(), String> {
    let ids = self.reach(event_id, scope);
    if ids.is_empty() {
      return Err("That event no longer exists.".into());
    }
    self.events.retain(|event| !ids.contains(&event.id));
    Ok(())
  }

  pub fn respond(&mut self, event_id: &str, scope: EditScope, rsvp: Rsvp) -> Result<(), String> {
    let ids = self.reach(event_id, scope);
    if ids.is_empty() {
      return Err("That event no longer exists.".into());
    }
    for event in self.events.iter_mut().filter(|event| ids.contains(&event.id)) {
      event.my_rsvp = Some(rsvp);
      for guest in event.guests.iter_mut().filter(|guest| guest.me) {
        guest.rsvp = rsvp;
      }
    }
    Ok(())
  }
}
