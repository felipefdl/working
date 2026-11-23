//! The calendar server (`/mcp/calendar`): the signed-in account's Google Calendar. It reads straight from Google
//! (the app need not have opened Calendar); the tools that change events are listed and run only when the user let
//! agents change the calendar, and never under `WORKING_READONLY=1`.
#![allow(clippy::too_many_lines, reason = "tool schemas are verbose JSON")]

use std::fmt::Write as _;
use std::sync::Arc;

use jiff::civil::Date;
use jiff::{ToSpan as _, ZonedDateTime};
use serde::Deserialize;
use serde_json::{Value, json};

use super::shared::{
  answer, clip, cursor, keep_end, local, loose_count, loose_flag, loose_switch, loose_text, no_arguments, now_line,
  parse, read_only, writes,
};
use crate::engine::Engine;
use crate::model::{
  Calendar, CalendarAccess, CalendarEvent, EditScope, EventDraft, EventKind, Micros, Reminder, Rsvp, Visibility,
};

/// The server's name.
pub(super) const NAME: &str = "working-calendar";

/// System prompt for MCP clients of this server.
pub(super) const INSTRUCTIONS: &str = "Google Calendar of the person signed in to the Working app on this computer. \
calendar_events lists events in a date range (today and the next 7 days unless asked otherwise), calendar_get_event \
reads one, and calendar_list names the calendars. Times are this computer's local time, written like \
2026-11-20T14:30. When the user allowed it, calendar_create_event, calendar_update_event, calendar_delete_event, and \
calendar_respond change events; guests are emailed only when `notify` is true, and an event that repeats needs \
`scope` (\"this\" or \"all\").";

/// The tools that change events: listed and run only with `calendar_write`.
const WRITE_TOOLS: [&str; 4] = [
  "calendar_create_event",
  "calendar_update_event",
  "calendar_delete_event",
  "calendar_respond",
];

const HOUR: Micros = 3_600_000_000;
/// Events `calendar_events` shows at most.
const MAX_EVENTS: usize = 200;

/// Tool descriptors; the ones that change events only when `writable`.
pub(super) fn list(writable: bool) -> Vec<Value> {
  let read_only = read_only();
  let event_ref = json!({
    "calendar_id": { "type": "string", "description": "The calendar id calendar_events gave." },
    "event_id": { "type": "string", "description": "The event id calendar_events gave." }
  });
  let mut tools = vec![
    json!({
      "name": "calendar_list",
      "title": "Calendars",
      "description": "List the user's calendars with their ids and what the user may do in each: their own, shared \
    with the right to change events, read only, or free and busy times only. calendar_events and the tools that change \
    events take a calendar by its id or its name.",
      "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
      "annotations": read_only,
    }),
    json!({
      "name": "calendar_events",
      "title": "Calendar events",
      "description": "List the user's events in a date range, by start: by default today and the next 7 days. Each \
    event says its title, start and end in local time, place, Google Meet link, guests with their answers, the user's \
    own answer, and the event id and calendar id the other tools take. Filter by calendar or by words in the title, \
    place, or description (\"design review\", \"Jordan\").",
      "inputSchema": {
        "type": "object",
        "properties": {
          "start": { "type": "string", "description": "From this local date or time, such as 2026-11-20 or 2026-11-20T09:00. Default today." },
          "end": { "type": "string", "description": "Until this local date or time (a date means the start of that day). Default 7 days after today." },
          "calendar": { "type": "string", "description": "Only this calendar: its id or name." },
          "query": { "type": "string", "description": "Only events whose title, place, or description holds these words." },
          "limit": { "type": "integer", "description": "Most events. Default 50, at most 200." }
        },
        "additionalProperties": false
      },
      "annotations": read_only,
    }),
    json!({
      "name": "calendar_get_event",
      "title": "Read an event",
      "description": "Read one event in full: what calendar_events gives, with its description, organizer, and \
    whether it repeats.",
      "inputSchema": {
        "type": "object",
        "properties": event_ref,
        "required": ["calendar_id", "event_id"],
        "additionalProperties": false
      },
      "annotations": read_only,
    }),
  ];
  if !writable {
    return tools;
  }
  let scope = json!({ "type": "string", "enum": ["this", "all"], "description": "For an event that repeats: this occurrence alone, or the whole series. Required then." });
  let notify = json!({ "type": "boolean", "description": "Email the guests about it. Default false." });
  tools.extend([
    json!({
      "name": "calendar_create_event",
      "title": "Create an event",
      "description": "Create an event in the user's calendar: a title and a start, and if wanted an end (default an \
    hour later, or the next day for an all-day event), place, description, guests by email, and a Google Meet link. \
    Guests are emailed only with notify true. Answers the new event's start and the calendar it went to.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "title": { "type": "string", "description": "Title, such as \"Design review with Jordan Park\"." },
          "start": { "type": "string", "description": "Local start, such as 2026-11-20T14:30, or a date for an all-day event." },
          "end": { "type": "string", "description": "Local end (exclusive for an all-day event)." },
          "all_day": { "type": "boolean", "description": "Spans whole days. Default false." },
          "calendar": { "type": "string", "description": "Calendar id or name. Default the user's own." },
          "location": { "type": "string" },
          "description": { "type": "string" },
          "guests": { "type": "array", "items": { "type": "string" }, "description": "Guests' emails, such as jpark@example.com." },
          "add_meet": { "type": "boolean", "description": "Give it a Google Meet link. Default false." },
          "notify": notify
        },
        "required": ["title", "start"],
        "additionalProperties": false
      },
      "annotations": writes(false, false),
    }),
    json!({
      "name": "calendar_update_event",
      "title": "Change an event",
      "description": "Change an event: pass only what changes. `guests` replaces the guest list (guests already \
    invited keep their answers). `calendar` moves it to another calendar. An event that repeats needs scope.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "calendar_id": { "type": "string", "description": "The calendar id calendar_events gave." },
          "event_id": { "type": "string", "description": "The event id calendar_events gave." },
          "title": { "type": "string" },
          "start": { "type": "string", "description": "New local start." },
          "end": { "type": "string", "description": "New local end." },
          "all_day": { "type": "boolean" },
          "location": { "type": "string" },
          "description": { "type": "string" },
          "guests": { "type": "array", "items": { "type": "string" } },
          "add_meet": { "type": "boolean", "description": "Give it a Google Meet link when it has none." },
          "calendar": { "type": "string", "description": "Move it to this calendar: id or name." },
          "scope": scope,
          "notify": notify
        },
        "required": ["calendar_id", "event_id"],
        "additionalProperties": false
      },
      "annotations": writes(false, true),
    }),
    json!({
      "name": "calendar_delete_event",
      "title": "Delete an event",
      "description": "Delete an event from the user's calendar. An event that repeats needs scope. Guests are \
    emailed only with notify true.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "calendar_id": { "type": "string", "description": "The calendar id calendar_events gave." },
          "event_id": { "type": "string", "description": "The event id calendar_events gave." },
          "scope": scope,
          "notify": notify
        },
        "required": ["calendar_id", "event_id"],
        "additionalProperties": false
      },
      "annotations": writes(true, true),
    }),
    json!({
      "name": "calendar_respond",
      "title": "Answer an invitation",
      "description": "Answer an invitation as the user: accept, decline, or maybe. The organizer sees the answer. An \
    event that repeats needs scope.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "calendar_id": { "type": "string", "description": "The calendar id calendar_events gave." },
          "event_id": { "type": "string", "description": "The event id calendar_events gave." },
          "answer": { "type": "string", "enum": ["accept", "decline", "maybe"] },
          "scope": scope
        },
        "required": ["calendar_id", "event_id", "answer"],
        "additionalProperties": false
      },
      "annotations": writes(false, true),
    }),
  ]);
  tools
}

/// Runs a tool. `None` when there is no tool of that name.
pub(super) async fn call(engine: &Arc<Engine>, writable: bool, name: &str, arguments: Value) -> Option<Value> {
  if WRITE_TOOLS.contains(&name) {
    if !writable {
      return Some(answer(Err(
        "Changing the calendar is turned off. The user can allow it in Working: Settings, Agents.".into(),
      )));
    }
    if engine.readonly() {
      return Some(answer(Err("Working runs in read-only mode: nothing was changed.".into())));
    }
  }
  let outcome = match name {
    "calendar_list" => calendar_list(engine, &arguments).await,
    "calendar_events" => calendar_events(engine, arguments).await,
    "calendar_get_event" => calendar_get_event(engine, arguments).await,
    "calendar_create_event" => calendar_create_event(engine, arguments).await,
    "calendar_update_event" => calendar_update_event(engine, arguments).await,
    "calendar_delete_event" => calendar_delete_event(engine, arguments).await,
    "calendar_respond" => calendar_respond(engine, arguments).await,
    _ => return None,
  };
  Some(answer(outcome))
}

// ---- Arguments ----

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventsArgs {
  #[serde(default, deserialize_with = "loose_text")]
  start: Option<String>,
  #[serde(default, deserialize_with = "loose_text")]
  end: Option<String>,
  #[serde(default, deserialize_with = "loose_text")]
  calendar: Option<String>,
  #[serde(default, deserialize_with = "loose_text")]
  query: Option<String>,
  #[serde(default, deserialize_with = "loose_count")]
  limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventRef {
  calendar_id: String,
  event_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
  title: String,
  start: String,
  #[serde(default, deserialize_with = "loose_text")]
  end: Option<String>,
  #[serde(default, deserialize_with = "loose_flag")]
  all_day: bool,
  #[serde(default, deserialize_with = "loose_text")]
  calendar: Option<String>,
  #[serde(default)]
  location: String,
  #[serde(default)]
  description: String,
  #[serde(default)]
  guests: Vec<String>,
  #[serde(default, deserialize_with = "loose_flag")]
  add_meet: bool,
  #[serde(default, deserialize_with = "loose_flag")]
  notify: bool,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ScopeArg {
  This,
  All,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
  calendar_id: String,
  event_id: String,
  #[serde(default, deserialize_with = "loose_text")]
  title: Option<String>,
  #[serde(default, deserialize_with = "loose_text")]
  start: Option<String>,
  #[serde(default, deserialize_with = "loose_text")]
  end: Option<String>,
  #[serde(default, deserialize_with = "loose_switch")]
  all_day: Option<bool>,
  #[serde(default)]
  location: Option<String>,
  #[serde(default)]
  description: Option<String>,
  #[serde(default)]
  guests: Option<Vec<String>>,
  #[serde(default, deserialize_with = "loose_flag")]
  add_meet: bool,
  #[serde(default, deserialize_with = "loose_text")]
  calendar: Option<String>,
  #[serde(default)]
  scope: Option<ScopeArg>,
  #[serde(default, deserialize_with = "loose_flag")]
  notify: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteArgs {
  calendar_id: String,
  event_id: String,
  #[serde(default)]
  scope: Option<ScopeArg>,
  #[serde(default, deserialize_with = "loose_flag")]
  notify: bool,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum AnswerArg {
  Accept,
  Decline,
  Maybe,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RespondArgs {
  calendar_id: String,
  event_id: String,
  answer: AnswerArg,
  #[serde(default)]
  scope: Option<ScopeArg>,
}

// ---- Words for things ----

fn offline(error: &anyhow::Error) -> String {
  format!("Google Calendar did not answer: {error:#}. Try again in a moment.")
}

const fn access_text(access: CalendarAccess) -> &'static str {
  match access {
    CalendarAccess::Owner => "the user's own",
    CalendarAccess::Writer => "shared, can change events",
    CalendarAccess::Reader => "read only",
    CalendarAccess::FreeBusy => "free and busy times only",
  }
}

const fn rsvp_text(rsvp: Rsvp) -> &'static str {
  match rsvp {
    Rsvp::NeedsAction => "not answered",
    Rsvp::Accepted => "going",
    Rsvp::Tentative => "maybe",
    Rsvp::Declined => "not going",
  }
}

/// When an event is, in local time: `2026-11-20 14:30–15:00`, or `2026-11-20 all day` (to the last day it spans).
fn when_text(event: &CalendarEvent) -> String {
  let start = local(event.start);
  if event.all_day {
    let last = crate::boundary_date(event.end.saturating_sub(1)).unwrap_or_else(|| start.date());
    let first = start.date();
    return if last > first {
      format!("{first} to {last}, all day")
    } else {
      format!("{first}, all day")
    };
  }
  let end = local(event.end);
  if end.date() == start.date() {
    format!("{}–{}", start.strftime("%Y-%m-%d %H:%M"), end.strftime("%H:%M"))
  } else {
    format!("{} to {}", start.strftime("%Y-%m-%d %H:%M"), end.strftime("%Y-%m-%d %H:%M"))
  }
}

/// An event as answers show it. `full` adds the description, organizer, and whether it repeats.
fn event_text(event: &CalendarEvent, calendar: Option<&Calendar>, full: bool) -> String {
  let mut out = format!("- [{}] {}\n", when_text(event), event.title);
  let calendar_name = calendar.map_or(event.calendar_id.as_str(), |calendar| calendar.name.as_str());
  let _ = writeln!(
    out,
    "  event_id: {}  calendar_id: {} ({calendar_name})",
    event.id, event.calendar_id
  );
  if !event.location.is_empty() {
    let _ = writeln!(out, "  Where: {}", clip(&event.location, 200));
  }
  if let Some(url) = &event.meet_url {
    let _ = writeln!(out, "  Meet: {url}");
  }
  if !event.guests.is_empty() {
    let guests: Vec<String> = event
      .guests
      .iter()
      .map(|guest| {
        let who = if guest.name.is_empty() {
          &guest.email
        } else {
          &guest.name
        };
        let role = if guest.organizer { ", organizer" } else { "" };
        format!("{who} ({}{role})", rsvp_text(guest.rsvp))
      })
      .collect();
    let _ = writeln!(out, "  Guests: {}", guests.join(", "));
  }
  if let Some(rsvp) = event.my_rsvp {
    let _ = writeln!(out, "  Your answer: {}", rsvp_text(rsvp));
  }
  if full {
    if !event.organizer.is_empty() {
      let _ = writeln!(out, "  Organizer: {}", event.organizer);
    }
    if event.series.is_some() {
      out.push_str("  Repeats: yes (changing it needs scope \"this\" or \"all\")\n");
    }
    if !event.description.is_empty() {
      let _ = writeln!(out, "  Description:\n{}", event.description);
    }
  }
  out
}

/// The calendar a model means by its id or name: an id or exact name (any case), else one name that holds the words.
fn find_calendar<'a>(calendars: &'a [Calendar], wanted: &str) -> Result<&'a Calendar, String> {
  let wanted = wanted.trim();
  let lower = wanted.to_lowercase();
  if let Some(exact) = calendars
    .iter()
    .find(|calendar| calendar.id == wanted || calendar.name.to_lowercase() == lower)
  {
    return Ok(exact);
  }
  let matching: Vec<&Calendar> = calendars
    .iter()
    .filter(|calendar| calendar.name.to_lowercase().contains(&lower))
    .collect();
  match matching.as_slice() {
    [one] => Ok(*one),
    [] => Err(format!("No calendar is named \"{wanted}\". Call calendar_list to see them.")),
    many => {
      let names: Vec<&str> = many.iter().map(|calendar| calendar.name.as_str()).collect();
      Err(format!(
        "\"{wanted}\" matches several calendars: {}. Pass one id.",
        names.join(", ")
      ))
    },
  }
}

/// Local midnight of `date`.
fn midnight(date: Date) -> Result<Micros, String> {
  crate::local_midnight(date).ok_or_else(|| format!("{date} has no midnight here"))
}

/// The scope a change of `event` needs: an event that repeats must name one.
fn scope_of(event: &CalendarEvent, scope: Option<ScopeArg>) -> Result<EditScope, String> {
  match (scope, event.series.is_some()) {
    (Some(ScopeArg::All), true) => Ok(EditScope::All),
    (Some(_), _) | (None, false) => Ok(EditScope::This),
    (None, true) => Err(
      "This event repeats: pass scope \"this\" to change this occurrence alone, or \"all\" for the whole series."
        .into(),
    ),
  }
}

/// A start and end as Google Calendar takes them: an all-day event from local midnight of its first day to local
/// midnight after its last; a timed one an hour long unless `end` says.
fn span(start: &str, end: Option<&str>, all_day: bool) -> Result<(Micros, Micros), String> {
  let start = cursor(start)?;
  if all_day {
    let first = local(start).date();
    let start = midnight(first)?;
    let end = match end {
      Some(end) => midnight(local(cursor(end)?).date())?,
      None => midnight(first.saturating_add(1.day()))?,
    };
    return Ok((start, end.max(start)));
  }
  let end = match end {
    Some(end) => cursor(end)?,
    None => start.saturating_add(HOUR),
  };
  if end < start {
    return Err("`end` is before `start`.".into());
  }
  Ok((start, end))
}

// ---- Tools ----

async fn calendar_list(engine: &Arc<Engine>, arguments: &Value) -> Result<String, String> {
  no_arguments("calendar_list", arguments)?;
  let calendars = engine.calendar_list().await.map_err(|error| offline(&error))?;
  let mut out = format!("{} calendars:\n", calendars.len());
  for calendar in &calendars {
    let primary = if calendar.primary { ", primary" } else { "" };
    let _ = writeln!(
      out,
      "- {} (id: {}; {}{primary})",
      calendar.name,
      calendar.id,
      access_text(calendar.access)
    );
  }
  Ok(out)
}

async fn calendar_events(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: EventsArgs = parse(arguments)?;
  let today = ZonedDateTime::now().date();
  let start = match &args.start {
    Some(start) => cursor(start)?,
    None => midnight(today)?,
  };
  let end = match &args.end {
    Some(end) => cursor(end)?,
    None => midnight(today.saturating_add(8.days()))?,
  };
  if end <= start {
    return Err("`end` has to come after `start`.".into());
  }
  let limit = args.limit.unwrap_or(50).clamp(1, MAX_EVENTS);
  let calendars = engine.calendar_list().await.map_err(|error| offline(&error))?;
  let only = match &args.calendar {
    Some(wanted) => Some(find_calendar(&calendars, wanted)?.id.clone()),
    None => None,
  };
  let words: Vec<String> = args
    .query
    .as_deref()
    .map_or_default(|query| query.split_whitespace().map(str::to_lowercase).collect());
  let events = engine.calendar_events(start, end).await.map_err(|error| offline(&error))?;
  let matching: Vec<&CalendarEvent> = events
    .iter()
    .filter(|event| only.as_ref().is_none_or(|id| event.calendar_id == *id))
    .filter(|event| {
      let haystack = format!("{} {} {}", event.title, event.location, event.description).to_lowercase();
      words.iter().all(|word| haystack.contains(word))
    })
    .collect();
  let mut out = format!(
    "{}\nEvents from {} to {}:\n",
    now_line(),
    local(start).strftime("%Y-%m-%d %H:%M"),
    local(end).strftime("%Y-%m-%d %H:%M")
  );
  if matching.is_empty() {
    out.push_str("No events.\n");
    return Ok(out);
  }
  for event in matching.iter().take(limit) {
    let calendar = calendars.iter().find(|calendar| calendar.id == event.calendar_id);
    out.push_str(&event_text(event, calendar, false));
  }
  if matching.len() > limit {
    let _ = writeln!(
      out,
      "({} more: narrow the range or pass a larger limit.)",
      matching.len() - limit
    );
  }
  Ok(keep_end(out))
}

async fn calendar_get_event(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: EventRef = parse(arguments)?;
  let event = engine
    .calendar_event(&args.calendar_id, &args.event_id)
    .await
    .map_err(|error| offline(&error))?;
  let calendars = engine.calendar_list().await.unwrap_or_default();
  let calendar = calendars.iter().find(|calendar| calendar.id == event.calendar_id);
  Ok(event_text(&event, calendar, true))
}

async fn calendar_create_event(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: CreateArgs = parse(arguments)?;
  let title = args.title.trim();
  if title.is_empty() {
    return Err("`title` is empty.".into());
  }
  let calendars = engine.calendar_list().await.map_err(|error| offline(&error))?;
  let calendar = match &args.calendar {
    Some(wanted) => find_calendar(&calendars, wanted)?,
    None => calendars
      .iter()
      .find(|calendar| calendar.primary)
      .ok_or("The user has no calendar of their own.")?,
  };
  if !calendar.access.can_write() {
    return Err(format!("{} is read only: pick a calendar the user can change.", calendar.name));
  }
  let (start, end) = span(&args.start, args.end.as_deref(), args.all_day)?;
  let draft = EventDraft {
    calendar_id: calendar.id.clone(),
    editing: None,
    scope: EditScope::This,
    kind: EventKind::Event,
    title: title.to_string(),
    start,
    end,
    all_day: args.all_day,
    location: args.location,
    description: args.description,
    guests: args.guests,
    add_meet: args.add_meet,
    repeat: None,
    busy: true,
    visibility: Visibility::Default,
    color_id: None,
    reminder: Reminder::Default,
    notify: args.notify,
  };
  engine.calendar_save(&draft).await.map_err(|error| offline(&error))?;
  let mut created = format!(
    "Created \"{title}\" in {} on {}",
    calendar.name,
    local(start).strftime("%Y-%m-%d %H:%M")
  );
  if args.all_day {
    created.push_str(" (all day)");
  }
  created.push_str(". Call calendar_events to see it with its event id.");
  Ok(created)
}

/// The draft of an existing event, as it is: an update changes only what it names.
fn draft_of(event: &CalendarEvent) -> EventDraft {
  EventDraft {
    calendar_id: event.calendar_id.clone(),
    editing: Some((event.calendar_id.clone(), event.id.clone())),
    scope: EditScope::This,
    kind: event.kind,
    title: event.title.clone(),
    start: event.start,
    end: event.end,
    all_day: event.all_day,
    location: event.location.clone(),
    description: event.description.clone(),
    guests: event.guests.iter().map(|guest| guest.email.clone()).collect(),
    add_meet: false,
    repeat: None,
    busy: event.busy,
    visibility: event.visibility,
    color_id: event.color_id,
    reminder: event.reminder,
    notify: false,
  }
}

async fn calendar_update_event(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: UpdateArgs = parse(arguments)?;
  let event = engine
    .calendar_event(&args.calendar_id, &args.event_id)
    .await
    .map_err(|error| offline(&error))?;
  if !event.can_edit {
    return Err("The user may not change this event (it is someone else's, or the calendar is read only).".into());
  }
  let mut draft = draft_of(&event);
  draft.scope = scope_of(&event, args.scope)?;
  draft.notify = args.notify;
  draft.add_meet = args.add_meet;
  if let Some(title) = args.title.as_deref().map(str::trim).filter(|title| !title.is_empty()) {
    draft.title = title.to_string();
  }
  if let Some(all_day) = args.all_day {
    draft.all_day = all_day;
  }
  if args.start.is_some() || args.end.is_some() || args.all_day.is_some() {
    let length = event.end.saturating_sub(event.start);
    let start = match &args.start {
      Some(start) => cursor(start)?,
      None => event.start,
    };
    let end = match &args.end {
      Some(end) => Some(cursor(end)?),
      None if args.start.is_some() => Some(start.saturating_add(length)),
      None => None,
    };
    let start_text = local(start).strftime("%Y-%m-%dT%H:%M").to_string();
    let end_text = end.map(|end| local(end).strftime("%Y-%m-%dT%H:%M").to_string());
    let (start, end) = span(&start_text, end_text.as_deref(), draft.all_day)?;
    draft.start = start;
    draft.end = end;
  }
  if let Some(location) = args.location {
    draft.location = location;
  }
  if let Some(description) = args.description {
    draft.description = description;
  }
  if let Some(guests) = args.guests {
    draft.guests = guests;
  }
  if let Some(wanted) = &args.calendar {
    let calendars = engine.calendar_list().await.map_err(|error| offline(&error))?;
    let calendar = find_calendar(&calendars, wanted)?;
    if !calendar.access.can_write() {
      return Err(format!("{} is read only: pick a calendar the user can change.", calendar.name));
    }
    draft.calendar_id = calendar.id.clone();
  }
  engine.calendar_save(&draft).await.map_err(|error| offline(&error))?;
  Ok(format!("Changed \"{}\".", draft.title))
}

async fn calendar_delete_event(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: DeleteArgs = parse(arguments)?;
  let event = engine
    .calendar_event(&args.calendar_id, &args.event_id)
    .await
    .map_err(|error| offline(&error))?;
  if !event.can_delete {
    return Err("The user may not delete this event from this calendar.".into());
  }
  let scope = scope_of(&event, args.scope)?;
  engine
    .calendar_delete(&args.calendar_id, &args.event_id, scope, args.notify)
    .await
    .map_err(|error| offline(&error))?;
  let which = if scope == EditScope::All {
    " and every occurrence"
  } else {
    ""
  };
  Ok(format!("Deleted \"{}\"{which}.", event.title))
}

async fn calendar_respond(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: RespondArgs = parse(arguments)?;
  let event = engine
    .calendar_event(&args.calendar_id, &args.event_id)
    .await
    .map_err(|error| offline(&error))?;
  if event.my_rsvp.is_none() {
    return Err("The user is not a guest of this event, or organizes it: there is nothing to answer.".into());
  }
  let scope = scope_of(&event, args.scope)?;
  let rsvp = match args.answer {
    AnswerArg::Accept => Rsvp::Accepted,
    AnswerArg::Decline => Rsvp::Declined,
    AnswerArg::Maybe => Rsvp::Tentative,
  };
  engine
    .calendar_respond(&args.calendar_id, &args.event_id, scope, rsvp)
    .await
    .map_err(|error| offline(&error))?;
  Ok(format!("Answered \"{}\": {}.", event.title, rsvp_text(rsvp)))
}

#[cfg(test)]
mod tests {
  use super::{WRITE_TOOLS, find_calendar, list, scope_of, span, when_text};
  use crate::model::{Calendar, CalendarAccess, CalendarEvent, EditScope};

  fn names(writable: bool) -> Vec<String> {
    list(writable)
      .iter()
      .filter_map(|tool| tool["name"].as_str().map(str::to_string))
      .collect()
  }

  #[test]
  fn write_tools_are_listed_only_when_allowed() {
    let read = names(false);
    assert_eq!(read, ["calendar_list", "calendar_events", "calendar_get_event"]);
    let all = names(true);
    assert!(WRITE_TOOLS.iter().all(|name| all.iter().any(|listed| listed == name)));
    for tool in list(true) {
      let writes = WRITE_TOOLS.contains(&tool["name"].as_str().unwrap_or_default());
      assert_eq!(tool["annotations"]["readOnlyHint"], !writes);
    }
  }

  fn calendar(id: &str, name: &str) -> Calendar {
    Calendar {
      id: id.into(),
      name: name.into(),
      color: 0x0042_85f4,
      access: CalendarAccess::Owner,
      primary: false,
      selected: true,
    }
  }

  #[test]
  fn calendars_are_found_by_id_or_name() {
    let calendars = [
      calendar("jpark@example.com", "Jordan Park"),
      calendar("team@group.example.com", "Team"),
      calendar("teamoffsite@group.example.com", "Team offsite"),
    ];
    assert_eq!(find_calendar(&calendars, "team").unwrap().id, "team@group.example.com");
    assert_eq!(find_calendar(&calendars, "jpark@example.com").unwrap().name, "Jordan Park");
    assert_eq!(find_calendar(&calendars, "offsite").unwrap().name, "Team offsite");
    assert!(find_calendar(&calendars, "Holidays").unwrap_err().contains("calendar_list"));
  }

  fn event(series: Option<&str>) -> CalendarEvent {
    CalendarEvent {
      id: "e1".into(),
      calendar_id: "jpark@example.com".into(),
      title: "Design review".into(),
      start: 0,
      end: 0,
      all_day: false,
      location: String::new(),
      description: String::new(),
      meet_url: None,
      link: String::new(),
      color: None,
      color_id: None,
      guests: Vec::new(),
      organizer: String::new(),
      series: series.map(str::to_string),
      kind: crate::model::EventKind::Event,
      busy: true,
      visibility: crate::model::Visibility::Default,
      reminder: crate::model::Reminder::Default,
      can_edit: true,
      can_delete: true,
      my_rsvp: None,
    }
  }

  #[test]
  fn a_repeating_event_needs_a_scope() {
    assert_eq!(scope_of(&event(None), None), Ok(EditScope::This));
    assert!(scope_of(&event(Some("s")), None).is_err());
    assert_eq!(scope_of(&event(Some("s")), Some(super::ScopeArg::All)), Ok(EditScope::All));
    assert_eq!(scope_of(&event(Some("s")), Some(super::ScopeArg::This)), Ok(EditScope::This));
  }

  #[test]
  fn spans_default_to_an_hour_or_a_day() {
    let (start, end) = span("2026-11-20T14:30", None, false).unwrap();
    assert_eq!(end - start, 3_600_000_000);
    let (start, end) = span("2026-11-20T14:30", None, true).unwrap();
    assert!(end - start >= 23 * 3_600_000_000 && end - start <= 25 * 3_600_000_000);
    assert!(span("2026-11-20T14:30", Some("2026-11-20T13:00"), false).is_err());
    let mut timed = event(None);
    let (start, end) = span("2026-11-20T14:30", Some("2026-11-20T15:00"), false).unwrap();
    timed.start = start;
    timed.end = end;
    assert_eq!(when_text(&timed), "2026-11-20 14:30–15:00");
  }
}
