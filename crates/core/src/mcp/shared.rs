//! What every server's tools share: loose argument parsing, the tool result, and words for times and text.
#![allow(
  clippy::as_conversions,
  clippy::cast_possible_truncation,
  clippy::cast_sign_loss,
  clippy::string_slice,
  reason = "a count a model sent as a float is cut to a whole number, and answers are cut on char boundaries"
)]

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use crate::model::Micros;

/// Longest answer of a tool, in bytes: about 12,000 tokens.
pub(super) const MAX_ANSWER: usize = 48_000;

/// The annotations of a tool that only reads.
pub(super) fn read_only() -> Value {
  json!({ "readOnlyHint": true, "openWorldHint": true })
}

/// The annotations of a tool that changes something, and whether running it twice changes it twice.
pub(super) fn writes(destructive: bool, idempotent: bool) -> Value {
  json!({ "readOnlyHint": false, "destructiveHint": destructive, "idempotentHint": idempotent, "openWorldHint": true })
}

/// A tool's result as MCP carries it.
pub(super) fn answer(outcome: Result<String, String>) -> Value {
  let (text, failed) = match outcome {
    Ok(text) => (text, false),
    Err(text) => (text, true),
  };
  json!({ "content": [{ "type": "text", "text": text }], "isError": failed })
}

// ---- Arguments ----
//
// Models send numbers as text and flags as "true" often enough to take both.

#[derive(Deserialize)]
#[serde(untagged)]
enum Loose {
  Flag(bool),
  Number(f64),
  Text(String),
}

pub(super) fn loose_count<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<usize>, D::Error> {
  match Option::<Loose>::deserialize(deserializer)? {
    None => Ok(None),
    Some(Loose::Number(number)) if number >= 0.0 => Ok(Some(number as usize)),
    Some(Loose::Text(text)) => text.trim().parse().map_or_else(
      |_| Err(D::Error::custom(format!("\"{text}\" is not a number"))),
      |number| Ok(Some(number)),
    ),
    Some(_) => Err(D::Error::custom("expected a number, zero or more")),
  }
}

pub(super) fn loose_flag<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
  loose_switch(deserializer).map(Option::unwrap_or_default)
}

/// A flag that may be left out, as `None`.
pub(super) fn loose_switch<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
  match Option::<Loose>::deserialize(deserializer)? {
    None => Ok(None),
    Some(Loose::Flag(flag)) => Ok(Some(flag)),
    Some(Loose::Text(text)) if text.eq_ignore_ascii_case("true") => Ok(Some(true)),
    Some(Loose::Text(text)) if text.eq_ignore_ascii_case("false") => Ok(Some(false)),
    Some(_) => Err(D::Error::custom("expected true or false")),
  }
}

/// Optional text; a cursor that came back as a number is taken too. Blank counts as absent.
pub(super) fn loose_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
  match Option::<Loose>::deserialize(deserializer)? {
    None => Ok(None),
    Some(Loose::Text(text)) => Ok(Some(text.trim().to_string()).filter(|text| !text.is_empty())),
    Some(Loose::Number(number)) => Ok(Some(format!("{number:.0}"))),
    Some(Loose::Flag(_)) => Err(D::Error::custom("expected text")),
  }
}

pub(super) fn parse<T: DeserializeOwned>(arguments: Value) -> Result<T, String> {
  let arguments = if arguments.is_null() {
    json!({})
  } else {
    arguments
  };
  serde_json::from_value(arguments).map_err(|error| format!("Invalid arguments: {error}."))
}

/// A tool that takes no arguments: nothing, or an empty object.
pub(super) fn no_arguments(name: &str, arguments: &Value) -> Result<(), String> {
  let empty = arguments.is_null() || arguments.as_object().is_some_and(serde_json::Map::is_empty);
  if empty {
    Ok(())
  } else {
    Err(format!("Invalid arguments: {name} takes none."))
  }
}

// ---- Words for things ----

pub(super) fn local(time: Micros) -> DateTime<Local> {
  DateTime::from_timestamp_micros(time).unwrap_or_default().with_timezone(&Local)
}

pub(super) fn stamp(time: Micros) -> String {
  local(time).format("%Y-%m-%d %H:%M").to_string()
}

pub(super) fn now_line() -> String {
  Local::now().format("Now: %a %Y-%m-%d %H:%M (UTC%:z).").to_string()
}

/// A point in time: the number a previous answer gave, or a local date or date and time.
pub(super) fn cursor(text: &str) -> Result<Micros, String> {
  // Anything this long is a count of microseconds, not a year.
  if text.len() > 12
    && let Ok(micros) = text.parse::<Micros>()
  {
    return Ok(micros);
  }
  let formats = ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"];
  let timed = formats
    .iter()
    .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok());
  let day = || {
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
      .ok()
      .map(|day| day.and_time(NaiveTime::MIN))
  };
  let moment = timed
    .or_else(day)
    .and_then(|naive| Local.from_local_datetime(&naive).earliest());
  moment.map(|moment| moment.timestamp_micros()).ok_or_else(|| {
    format!("\"{text}\" is not a point in time. Pass the value a previous answer gave, or a date such as 2026-11-01 or 2026-11-01T14:30.")
  })
}

/// At most `max` characters of `text` on one line.
pub(super) fn clip(text: &str, max: usize) -> String {
  let mut line = text.split_whitespace().collect::<Vec<_>>().join(" ");
  if let Some((cut, _)) = line.char_indices().nth(max) {
    line.truncate(cut);
    line.push('…');
  }
  line
}

/// Cuts an answer that grew too long at its start, where the oldest part is.
pub(super) fn keep_end(text: String) -> String {
  if text.len() <= MAX_ANSWER {
    return text;
  }
  let mut start = text.len() - MAX_ANSWER;
  while !text.is_char_boundary(start) {
    start += 1;
  }
  let start = text[start..].find('\n').map_or(start, |newline| start + newline + 1);
  format!("(the oldest part was left out to keep this short)\n{}", &text[start..])
}

#[cfg(test)]
mod tests {
  use super::{answer, clip, cursor, no_arguments, parse};
  use serde::Deserialize;
  use serde_json::json;

  #[derive(Deserialize)]
  #[serde(deny_unknown_fields)]
  struct Args {
    #[serde(default, deserialize_with = "super::loose_count")]
    limit: Option<usize>,
  }

  #[test]
  fn clip_collapses_space_and_cuts_on_a_char() {
    assert_eq!(clip("a  b\n c", 10), "a b c");
    assert_eq!(clip("ééééé", 3), "ééé…");
  }

  #[test]
  fn cursor_reads_micros_and_dates() {
    assert_eq!(cursor("1794896454636000"), Ok(1_794_896_454_636_000));
    assert!(cursor("2026-11-01").is_ok());
    assert!(cursor("2026-11-01T14:30").is_ok());
    assert!(cursor("tomorrow").is_err());
  }

  #[test]
  fn results_and_argument_checks() {
    assert_eq!(answer(Err("No.".into()))["isError"], true);
    assert_eq!(answer(Ok("Yes.".into()))["content"][0]["text"], "Yes.");
    assert!(no_arguments("x", &json!(null)).is_ok());
    assert!(no_arguments("x", &json!({})).is_ok());
    assert!(no_arguments("x", &json!({ "a": 1 })).is_err());
    assert_eq!(parse::<Args>(json!({ "limit": "4" })).map(|args| args.limit), Ok(Some(4)));
  }
}
