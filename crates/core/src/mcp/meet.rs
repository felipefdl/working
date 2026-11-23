//! The Meet server (`/mcp/meet`): what the core can do with Google Meet: make a new meeting of its own (Meet's "New
//! meeting", `Command::CreateMeeting`), and find the user's next events that have a Meet link, from the calendar.

use std::fmt::Write as _;
use std::sync::Arc;

use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{Value, json};

use super::shared::{answer, loose_count, no_arguments, now_line, parse, read_only, stamp, writes};
use crate::engine::Engine;
use crate::model::CalendarEvent;

/// The server's name.
pub(super) const NAME: &str = "working-meet";

/// System prompt for MCP clients of this server.
pub(super) const INSTRUCTIONS: &str = "Google Meet for the person signed in to the Working app on this computer. \
meet_create_meeting makes a new meeting and answers its link, to share with whoever should join; it is in no chat and \
on no calendar. meet_upcoming lists the user's next calendar events that have a Meet link. Times are this computer's \
local time.";

/// Days `meet_upcoming` looks ahead.
const UPCOMING_DAYS: i64 = 7;

/// Tool descriptors.
pub(super) fn list() -> Vec<Value> {
  vec![
    json!({
      "name": "meet_create_meeting",
      "title": "New Meet meeting",
      "description": "Make a new Google Meet meeting and answer its link (https://meet.google.com/…). The meeting \
    is in no chat and on no calendar: share the link where it is needed, such as in a message to Jordan Park.",
      "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
      "annotations": writes(false, false),
    }),
    json!({
      "name": "meet_upcoming",
      "title": "Upcoming meetings",
      "description": "List the user's calendar events in the next 7 days that have a Google Meet link, soonest \
    first, each with its start time, title, and link.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "limit": { "type": "integer", "description": "Most meetings. Default 10, at most 50." }
        },
        "additionalProperties": false
      },
      "annotations": read_only(),
    }),
  ]
}

/// Runs a tool. `None` when there is no tool of that name.
pub(super) async fn call(engine: &Arc<Engine>, name: &str, arguments: Value) -> Option<Value> {
  let outcome = match name {
    "meet_create_meeting" => create_meeting(engine, &arguments).await,
    "meet_upcoming" => upcoming(engine, arguments).await,
    _ => return None,
  };
  Some(answer(outcome))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpcomingArgs {
  #[serde(default, deserialize_with = "loose_count")]
  limit: Option<usize>,
}

async fn create_meeting(engine: &Arc<Engine>, arguments: &Value) -> Result<String, String> {
  no_arguments("meet_create_meeting", arguments)?;
  if engine.readonly() {
    return Err("Working runs in read-only mode: no meeting was created.".into());
  }
  let link = engine.create_meeting_link().await?;
  Ok(format!("New meeting: {link}"))
}

/// The meetings among `events`, soonest first, as lines.
fn meeting_lines(events: &[CalendarEvent], limit: usize) -> String {
  let meetings: Vec<&CalendarEvent> = events.iter().filter(|event| event.meet_url.is_some()).collect();
  if meetings.is_empty() {
    return "No meetings with a Meet link in the next 7 days.\n".into();
  }
  let mut out = String::new();
  for event in meetings.iter().take(limit) {
    let link = event.meet_url.as_deref().unwrap_or_default();
    let _ = writeln!(out, "- [{}] {}: {link}", stamp(event.start), event.title);
  }
  if meetings.len() > limit {
    let _ = writeln!(out, "({} more: pass a larger limit.)", meetings.len() - limit);
  }
  out
}

async fn upcoming(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: UpcomingArgs = parse(arguments)?;
  let limit = args.limit.unwrap_or(10).clamp(1, 50);
  let start = Timestamp::now().as_microsecond();
  let end = start.saturating_add(UPCOMING_DAYS * 24 * 3_600_000_000);
  let events = engine
    .calendar_events(start, end)
    .await
    .map_err(|error| format!("Google Calendar did not answer: {error:#}. Try again in a moment."))?;
  Ok(format!("{}\n{}", now_line(), meeting_lines(&events, limit)))
}

#[cfg(test)]
mod tests {
  use super::{list, meeting_lines};
  use crate::model::{CalendarEvent, EventKind, Reminder, Visibility};

  fn event(title: &str, start: i64, meet: Option<&str>) -> CalendarEvent {
    CalendarEvent {
      id: title.into(),
      calendar_id: "jpark@example.com".into(),
      title: title.into(),
      start,
      end: start + 1_800_000_000,
      all_day: false,
      location: String::new(),
      description: String::new(),
      meet_url: meet.map(str::to_string),
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

  #[test]
  fn tools_are_the_two_meet_offers() {
    let names: Vec<String> = list()
      .iter()
      .filter_map(|tool| tool["name"].as_str().map(str::to_string))
      .collect();
    assert_eq!(names, ["meet_create_meeting", "meet_upcoming"]);
  }

  #[test]
  fn upcoming_lists_only_events_with_a_link() {
    let events = [
      event(
        "Design review",
        1_794_896_400_000_000,
        Some("https://meet.google.com/abc-defg-hij"),
      ),
      event("Lunch", 1_794_900_000_000_000, None),
      event("Planning", 1_794_903_600_000_000, Some("https://meet.google.com/xyz-uvwx-yz")),
    ];
    let text = meeting_lines(&events, 1);
    assert!(text.contains("] Design review: https://meet.google.com/abc-defg-hij\n"));
    assert!(!text.contains("Lunch"));
    assert!(text.ends_with("(1 more: pass a larger limit.)\n"));
    assert_eq!(
      meeting_lines(&events[1..2], 5),
      "No meetings with a Meet link in the next 7 days.\n"
    );
  }
}
