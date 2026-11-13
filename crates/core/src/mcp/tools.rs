//! The tools agents get, shaped so a model does not have to think: few of them, one per thing a person would
//! ask for; a conversation is named the way a person names it (the server finds it); answers are plain text
//! with names, local times and the exact value to pass to the next call; and a mistake is answered with what
//! to do instead.
#![allow(
  clippy::too_many_lines,
  clippy::as_conversions,
  clippy::cast_possible_truncation,
  clippy::cast_sign_loss,
  clippy::option_if_let_else,
  clippy::needless_pass_by_value,
  clippy::string_slice,
  clippy::indexing_slicing,
  clippy::or_fun_call,
  reason = "tool schemas are verbose JSON and answers slice text on char boundaries"
)]

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use tokio::task::JoinSet;

use crate::engine::{Engine, Page};
use crate::fuzzy::Fuzzy;
use crate::model::{AttachmentKind, Group, GroupId, GroupKind, Message, Micros, Notify, SpanKind};

/// System prompt for MCP clients: how to name conversations and that reads do not mark messages read.
pub const INSTRUCTIONS: &str = "Google Chat of the person signed in to the Working app on this Mac. \
Catch up with inbox. To answer someone the user did not name, start with recent_activity. Find a chat with find_conversations, read it with read_conversation, and find a word or \
topic across every chat with search_messages. Wherever a `conversation` is asked for, a name (\"Jordan Park\"), \
part of one, or an id (dm/…, space/…) works. Times are this Mac's local time. Reading changes nothing: \
messages stay unread.";

/// Longest answer of a tool, in bytes: about 12,000 tokens.
const MAX_ANSWER: usize = 48_000;

const MINUTE: Micros = 60_000_000;
/// How far back `recent_activity` looks.
const RECENT: Micros = 24 * 60 * MINUTE;
/// A space counts the people who wrote this recently as talking.
const TALKING: Micros = 2 * 60 * MINUTE;
/// Conversations `recent_activity` loads, most recently active first.
const SCANNED: usize = 25;
/// Messages `recent_activity` shows of each conversation or thread.
const SHOWN_EACH: usize = 6;
/// One-line spaces `recent_activity` lists at most.
const STAMPS: usize = 8;

/// Conversations read with `read_conversation` since the last send. `send_message` writes only to one of them, so
/// the model has the conversation in front of it when it writes the message.
#[derive(Default)]
pub struct Reads(Mutex<HashSet<GroupId>>);

impl Reads {
  fn mark(&self, group: &GroupId) {
    self.0.lock().unwrap_or_else(PoisonError::into_inner).insert(group.clone());
  }

  fn has(&self, group: &GroupId) -> bool {
    self.0.lock().unwrap_or_else(PoisonError::into_inner).contains(group)
  }

  fn clear(&self) {
    self.0.lock().unwrap_or_else(PoisonError::into_inner).clear();
  }
}

/// Tool descriptors. `send_message` is included only when `allow_send` is true.
pub fn list(allow_send: bool) -> Vec<Value> {
  let read_only = json!({ "readOnlyHint": true, "openWorldHint": true });
  let mut tools = vec![
    json!({
      "name": "inbox",
      "title": "Unread chats",
      "description": "Catch up on Google Chat: the unread conversations, newest first, each with its unread \
    messages. Call this first when asked what is new, what was missed, or whether someone wrote. Conversations \
    the user muted or set to mentions-only are counted but left out, unless include_quiet is true. To read more \
    of one conversation, call read_conversation with its id.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "limit": { "type": "integer", "description": "Most conversations to show. Default 10, at most 25." },
          "messages_each": { "type": "integer", "description": "Most messages to show per conversation. Default 5, at most 20." },
          "include_quiet": { "type": "boolean", "description": "Also show unread conversations that are muted or notify for mentions only. Default false." }
        },
        "additionalProperties": false
      },
      "annotations": read_only,
    }),
    json!({
      "name": "recent_activity",
      "title": "Where the user is active",
      "description": "Where the user is active in Google Chat in the last 24 hours: every conversation waiting on \
    a reply from them (a direct message or group chat whose last message is someone else's, or a mention in a space \
    they have not answered), then the ones they recently wrote or reacted in, each with its last few messages and \
    the id and thread send_message takes. Busy spaces that do not involve the user are one line each, without \
    messages. Call this first when the user says to reply, answer, or tell someone without naming the conversation \
    (\"answer back with that\", \"send them the numbers\", \"tell them yes\"), and act as if you were in the chat with \
    them: send to the conversation whose question the request answers, not the newest one, without asking. Ask only \
    when the request fits more than one conversation. Then read that conversation with read_conversation and write \
    the reply to fit it. Write the message in the language and tone that conversation already has with that person, \
    whatever language the user wrote to you in, and pick up where it left off, the way a person in the middle of \
    that chat would. Keep the user's meaning and add nothing they did not say: never add facts, numbers, or \
    promises the user did not give.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "limit": { "type": "integer", "description": "Most conversations and threads to show in full. Default 10, at most 25." }
        },
        "additionalProperties": false
      },
      "annotations": read_only,
    }),
    json!({
      "name": "find_conversations",
      "title": "Find chats",
      "description": "Find conversations (direct messages, group chats, spaces, apps) by name and get their \
    ids. The query is fuzzy: part of a name, the first letters of its words, or a person's email all work \
    (\"jor park\", \"prod team\"). Without a query it lists the most recently active conversations. Use it to \
    learn who and what exists. To read messages call read_conversation, which takes a name directly.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "query": { "type": "string", "description": "Name, part of a name, or email. Leave out to list the most recent conversations." },
          "kind": { "type": "string", "enum": ["any", "dm", "group", "space", "app"], "description": "dm: one person. group: unnamed chat of several people. space: named space. app: a bot. Default any." },
          "unread_only": { "type": "boolean", "description": "Only conversations with unread messages. Default false." },
          "limit": { "type": "integer", "description": "Most conversations to list. Default 15, at most 50." }
        },
        "additionalProperties": false
      },
      "annotations": read_only,
    }),
    json!({
      "name": "read_conversation",
      "title": "Read a chat",
      "description": "Read the messages of one conversation, oldest first, ending at the latest message. \
    Replies in a thread are indented under the message they answer, and each top-level message ends with #id \
    (what send_message takes as `thread`). To go further back, pass the `before` value given at the end of the \
    previous answer. To see what surrounds a search result, pass its `around` value. To read only one thread of a \
    space, pass its `thread`. To find a word or topic, call search_messages instead of paging back.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "conversation": { "type": "string", "description": "The conversation's id (dm/… or space/…) or name. Part of a name works when it points at one conversation." },
          "limit": { "type": "integer", "description": "About how many messages. Default 30, at most 100." },
          "before": { "type": "string", "description": "Read the messages before this point: the value a previous answer gave, or a local date or time such as 2026-11-01 or 2026-11-01T14:30." },
          "around": { "type": "string", "description": "Read the messages around this point: the value a search_messages result gave." },
          "thread": { "type": "string", "description": "Read only this thread of a space: the #id that ends its top-level message, or the thread recent_activity gives. Read the thread you are about to answer." }
        },
        "required": ["conversation"],
        "additionalProperties": false
      },
      "annotations": read_only,
    }),
    json!({
      "name": "search_messages",
      "title": "Search messages",
      "description": "Search the text of messages in every conversation, however old. It matches whole \
    words, like the search box of Google Chat: two or three distinctive words work best. Each result says who \
    wrote it, where and when, and gives the read_conversation call that shows the messages around it. To find a \
    conversation by its name, call find_conversations instead.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "query": { "type": "string", "description": "Words to look for." },
          "conversation": { "type": "string", "description": "Only results from this conversation: its id or name." },
          "from": { "type": "string", "description": "Only messages written by this person: a name or part of one, or \"me\"." },
          "limit": { "type": "integer", "description": "Most results. Default 10, at most 20." }
        },
        "required": ["query"],
        "additionalProperties": false
      },
      "annotations": read_only,
    }),
  ];
  if allow_send {
    tools.push(json!({
      "name": "send_message",
      "title": "Send a message",
      "description": "Send a message as the user to one conversation, with text, files from this Mac, or both. \
It is posted for real and the people there see it at once: send only what the user asked for. `conversation` \
has to be an id or a full name; with anything less nothing is sent and the matching conversations are listed. \
With `thread` the message is a reply inside that thread of a space; without it, it goes to the main conversation. \
Read the conversation with read_conversation first: a send to a conversation not read since the last send is refused.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "conversation": { "type": "string", "description": "The conversation's id (dm/… or space/…) or its full name." },
          "text": { "type": "string", "description": "The message. Google Chat formatting works: *bold*, _italic_, `code`." },
          "files": { "type": "array", "items": { "type": "string" }, "description": "Files to attach: absolute paths on this Mac (~ works), such as /Users/me/report.pdf." },
          "thread": { "type": "string", "description": "Reply in a thread: the #id that ends a top-level message in read_conversation." }
        },
        "required": ["conversation"],
        "additionalProperties": false
      },
      "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
    }));
  }
  tools
}

/// Runs a tool. `None` when there is no tool of that name.
pub async fn call(
  engine: &Arc<Engine>,
  reads: &Reads,
  allow_send: bool,
  name: &str,
  arguments: Value,
) -> Option<Value> {
  let outcome = match name {
    "inbox" => inbox(engine, arguments).await,
    "recent_activity" => recent_activity(engine, arguments).await,
    "find_conversations" => find_conversations(engine, arguments),
    "read_conversation" => read_conversation(engine, reads, arguments).await,
    "search_messages" => search_messages(engine, arguments).await,
    "send_message" if allow_send => send_message(engine, reads, arguments).await,
    "send_message" => Err(
      "Sending is turned off. The user can allow it in Working: Settings, Agents, \"Let agents send messages\".".into(),
    ),
    _ => return None,
  };
  let (text, failed) = match outcome {
    Ok(text) => (text, false),
    Err(text) => (text, true),
  };
  Some(json!({ "content": [{ "type": "text", "text": text }], "isError": failed }))
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

fn loose_count<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<usize>, D::Error> {
  match Option::<Loose>::deserialize(deserializer)? {
    None => Ok(None),
    Some(Loose::Number(number)) if number >= 0.0 => Ok(Some(number as usize)),
    Some(Loose::Text(text)) => match text.trim().parse() {
      Ok(number) => Ok(Some(number)),
      Err(_) => Err(D::Error::custom(format!("\"{text}\" is not a number"))),
    },
    Some(_) => Err(D::Error::custom("expected a number, zero or more")),
  }
}

fn loose_flag<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
  match Option::<Loose>::deserialize(deserializer)? {
    None => Ok(false),
    Some(Loose::Flag(flag)) => Ok(flag),
    Some(Loose::Text(text)) if text.eq_ignore_ascii_case("true") => Ok(true),
    Some(Loose::Text(text)) if text.eq_ignore_ascii_case("false") => Ok(false),
    Some(_) => Err(D::Error::custom("expected true or false")),
  }
}

/// Optional text; a cursor that came back as a number is taken too. Blank counts as absent.
fn loose_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
  match Option::<Loose>::deserialize(deserializer)? {
    None => Ok(None),
    Some(Loose::Text(text)) => Ok(Some(text.trim().to_string()).filter(|text| !text.is_empty())),
    Some(Loose::Number(number)) => Ok(Some(format!("{number:.0}"))),
    Some(Loose::Flag(_)) => Err(D::Error::custom("expected text")),
  }
}

fn parse<T: DeserializeOwned>(arguments: Value) -> Result<T, String> {
  let arguments = if arguments.is_null() {
    json!({})
  } else {
    arguments
  };
  serde_json::from_value(arguments).map_err(|error| format!("Invalid arguments: {error}."))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InboxArgs {
  #[serde(default, deserialize_with = "loose_count")]
  limit: Option<usize>,
  #[serde(default, deserialize_with = "loose_count")]
  messages_each: Option<usize>,
  #[serde(default, deserialize_with = "loose_flag")]
  include_quiet: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivityArgs {
  #[serde(default, deserialize_with = "loose_count")]
  limit: Option<usize>,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
  #[default]
  Any,
  Dm,
  Group,
  Space,
  App,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FindArgs {
  #[serde(default, deserialize_with = "loose_text")]
  query: Option<String>,
  #[serde(default)]
  kind: Option<Kind>,
  #[serde(default, deserialize_with = "loose_flag")]
  unread_only: bool,
  #[serde(default, deserialize_with = "loose_count")]
  limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
  conversation: String,
  #[serde(default, deserialize_with = "loose_count")]
  limit: Option<usize>,
  #[serde(default, deserialize_with = "loose_text")]
  before: Option<String>,
  #[serde(default, deserialize_with = "loose_text")]
  around: Option<String>,
  #[serde(default, deserialize_with = "loose_text")]
  thread: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
  query: String,
  #[serde(default, deserialize_with = "loose_text")]
  conversation: Option<String>,
  #[serde(default, deserialize_with = "loose_text")]
  from: Option<String>,
  #[serde(default, deserialize_with = "loose_count")]
  limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendArgs {
  conversation: String,
  #[serde(default)]
  text: String,
  #[serde(default)]
  files: Vec<String>,
  #[serde(default, deserialize_with = "loose_text")]
  thread: Option<String>,
}

// ---- Words for things ----

fn local(time: Micros) -> DateTime<Local> {
  DateTime::from_timestamp_micros(time).unwrap_or_default().with_timezone(&Local)
}

fn stamp(time: Micros) -> String {
  local(time).format("%Y-%m-%d %H:%M").to_string()
}

fn now_line() -> String {
  Local::now().format("Now: %a %Y-%m-%d %H:%M (UTC%:z).").to_string()
}

/// A point in a conversation: the number a previous answer gave, or a local date or date and time.
fn cursor(text: &str) -> Result<Micros, String> {
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

fn id_text(id: &GroupId) -> String {
  match id {
    GroupId::Dm(id) => format!("dm/{id}"),
    GroupId::Space(id) => format!("space/{id}"),
  }
}

fn parse_id(text: &str) -> Option<GroupId> {
  match text.split_once(['/', '_'])? {
    ("dm", id) => Some(GroupId::Dm(id.to_string())),
    ("space", id) => Some(GroupId::Space(id.to_string())),
    _ => None,
  }
}

const fn kind_text(kind: GroupKind) -> &'static str {
  match kind {
    GroupKind::Dm => "direct message",
    GroupKind::App => "app",
    GroupKind::GroupDm => "group chat",
    GroupKind::Space => "space",
  }
}

/// A conversation as answers name it: `Jordan Park (direct message, dm/abc)`.
fn label(group: &Group) -> String {
  format!("{} ({}, {})", group.name, kind_text(group.kind), id_text(&group.id))
}

/// At most `max` characters of `text` on one line.
fn clip(text: &str, max: usize) -> String {
  let mut line = text.split_whitespace().collect::<Vec<_>>().join(" ");
  if let Some((cut, _)) = line.char_indices().nth(max) {
    line.truncate(cut);
    line.push('…');
  }
  line
}

fn offline(error: anyhow::Error) -> String {
  format!("Google Chat did not answer: {error:#}. Try again in a moment.")
}

/// Every conversation, most recently active first, with the text it is found by.
fn directory(engine: &Engine) -> Vec<(Group, String)> {
  let mut all = engine.directory();
  all.sort_by_key(|(group, _)| Reverse(group.sort_time));
  all
}

/// Finds the conversation a model means by an id, a name, or part of a name. A match is taken only when it
/// stands alone (among equally good ones, a one-to-one chat with a person wins); otherwise the answer lists
/// what matched. `exact` takes ids and full names only, for anything that writes.
fn resolve(directory: &[(Group, String)], reference: &str, exact: bool) -> Result<Group, String> {
  let reference = reference.trim();
  if reference.is_empty() {
    return Err("`conversation` is empty. Pass the name or id of a conversation.".into());
  }
  let by_id = parse_id(reference).and_then(|id| directory.iter().find(|(group, _)| group.id == id));
  if let Some((group, _)) = by_id {
    return Ok(group.clone());
  }
  let lower = reference.to_lowercase();
  let named: Vec<&Group> = directory
    .iter()
    .map(|(group, _)| group)
    .filter(|group| group.name.to_lowercase() == lower)
    .collect();
  let candidates = match named.as_slice() {
    [one] => return Ok((*one).clone()),
    [] => {
      let hits = Fuzzy::new(reference).rank(directory.iter(), |(_, text)| text.as_str());
      let best = hits.first().map(|(score, _)| *score);
      let top: Vec<&Group> = hits
        .iter()
        .take_while(|(score, _)| Some(*score) == best)
        .map(|(_, entry)| &entry.0)
        .collect();
      let people: Vec<&Group> = top.iter().copied().filter(|group| group.kind == GroupKind::Dm).collect();
      match (top.as_slice(), people.as_slice()) {
        _ if exact => {},
        ([one], _) | (_, [one]) => return Ok((*one).clone()),
        _ => {},
      }
      hits.into_iter().map(|(_, entry)| &entry.0).collect()
    },
    _ => named,
  };
  if candidates.is_empty() {
    return Err(format!(
      "No conversation matches \"{reference}\". Call find_conversations to see what exists."
    ));
  }
  let mut answer =
    format!("\"{reference}\" does not name one conversation. Call again with one of these ids as `conversation`:");
  for group in candidates.iter().take(8) {
    let _ = write!(answer, "\n- {}", label(group));
  }
  Err(answer)
}

/// One message as a line: who, what, and what hangs on it.
fn line(message: &Message, me: &str) -> String {
  let who = match () {
    () if message.system => "*",
    () if message.sender_id == me => "You",
    () if message.sender_name.is_empty() => "Someone",
    () => &message.sender_name,
  };
  let mut out = format!("[{}] {who}:", local(message.time).format("%H:%M"));
  if let Some(quote) = &message.quote {
    let _ = write!(out, " (quoting {}: \"{}\")", quote.sender_name, clip(&quote.text, 80));
  }
  let text = message.text.trim();
  if !text.is_empty() {
    let shown: String = text.chars().take(2000).collect();
    let _ = write!(out, " {}", shown.replace('\n', "\n      "));
    if shown.len() < text.len() {
      out.push_str("… (cut)");
    }
  }
  for attachment in &message.attachments {
    let kind = match attachment.kind {
      AttachmentKind::Image => "image",
      AttachmentKind::Video => "video",
      AttachmentKind::File => "file",
      AttachmentKind::Drive => "Drive file",
      AttachmentKind::Meet => "Google Meet",
      AttachmentKind::LinkPreview => "link",
    };
    // What only opens with the session is named; what anyone signed in can open carries its link.
    match attachment.kind {
      AttachmentKind::Image | AttachmentKind::Video | AttachmentKind::File => {
        let _ = write!(out, " [{kind}: {}]", attachment.name);
      },
      _ => {
        let _ = write!(out, " [{kind}: {} {}]", attachment.name, attachment.url);
      },
    }
  }
  for card in &message.cards {
    let mut parts: Vec<String> = vec![card.title.clone(), card.subtitle.clone()];
    for section in &card.sections {
      parts.push(section.header.clone());
      parts.extend(section.lines.iter().cloned());
      parts.extend(section.buttons.iter().map(|button| match &button.url {
        Some(url) => format!("{}: {url}", button.label),
        None => format!("[{}]", button.label),
      }));
    }
    parts.retain(|part| !part.trim().is_empty());
    let _ = write!(out, " [card: {}]", clip(&parts.join(" | "), 400));
  }
  if message.edited {
    out.push_str(" (edited)");
  }
  if !message.reactions.is_empty() {
    let reactions: Vec<String> = message
      .reactions
      .iter()
      .map(|reaction| format!("{}{}", reaction.emoji, reaction.count))
      .collect();
    let _ = write!(out, " {{{}}}", reactions.join(" "));
  }
  if !message.is_thread_reply() && !message.pending {
    let _ = write!(out, " #{}", message.id);
  }
  out
}

/// Messages as text, oldest first, under a line for each day. `focus` gets `>>` in front.
fn transcript<'a>(messages: impl IntoIterator<Item = &'a Arc<Message>>, me: &str, focus: Option<&str>) -> String {
  let mut out = String::new();
  let mut day = None;
  for message in messages {
    let date = local(message.time).date_naive();
    if day != Some(date) {
      day = Some(date);
      let _ = writeln!(out, "-- {} --", date.format("%a %Y-%m-%d"));
    }
    if focus == Some(message.id.as_str()) {
      out.push_str(">> ");
    }
    if message.is_thread_reply() {
      out.push_str("    ↳ ");
    }
    out.push_str(&line(message, me));
    out.push('\n');
  }
  out
}

/// Cuts an answer that grew too long at its start, where the oldest messages are.
fn keep_end(text: String) -> String {
  if text.len() <= MAX_ANSWER {
    return text;
  }
  let mut start = text.len() - MAX_ANSWER;
  while !text.is_char_boundary(start) {
    start += 1;
  }
  let start = text[start..].find('\n').map_or(start, |newline| start + newline + 1);
  format!("(the oldest messages were left out to keep this short)\n{}", &text[start..])
}

type Topic = (Micros, Vec<Arc<Message>>);

fn held(topics: &[Topic]) -> usize {
  topics.iter().map(|(_, messages)| messages.len()).sum()
}

/// Keeps the newest topics that hold `limit` messages; a topic is never split. True when some were dropped.
fn keep_newest(topics: &mut Vec<Topic>, limit: usize) -> bool {
  let mut messages = 0;
  let mut kept = 0;
  for (_, topic) in topics.iter().rev() {
    if messages >= limit {
      break;
    }
    messages += topic.len();
    kept += 1;
  }
  let dropped = topics.len() - kept;
  topics.drain(..dropped);
  dropped > 0
}

/// The same from the other end: keeps the oldest topics that hold `limit` messages.
fn keep_oldest(topics: &mut Vec<Topic>, limit: usize) -> bool {
  let mut messages = 0;
  let mut kept = 0;
  for (_, topic) in topics.iter() {
    if messages >= limit {
      break;
    }
    messages += topic.len();
    kept += 1;
  }
  let dropped = topics.len() > kept;
  topics.truncate(kept);
  dropped
}

// ---- Tools ----

async fn inbox(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: InboxArgs = parse(arguments)?;
  let limit = args.limit.unwrap_or(10).clamp(1, 25);
  let each = args.messages_each.unwrap_or(5).clamp(1, 20);
  let me = engine.me().map(|me| me.id).unwrap_or_default();

  let unread = directory(engine)
    .into_iter()
    .map(|(group, _)| group)
    .filter(|group| group.unread);
  let (mut shown, quiet): (Vec<Group>, Vec<Group>) =
    unread.partition(|group| !group.muted && matches!(group.notify, Notify::All | Notify::MainConversations));
  let mut note = String::new();
  if args.include_quiet {
    shown.extend(quiet);
    shown.sort_by_key(|group| Reverse(group.sort_time));
  } else if !quiet.is_empty() {
    note = format!(
      " {} more are muted or notify for mentions only; include_quiet=true shows them.",
      quiet.len()
    );
  }
  if shown.is_empty() {
    return Ok(format!("{} Nothing unread.{note}", now_line()));
  }
  let total = shown.len();
  shown.truncate(limit);

  let mut loads = JoinSet::new();
  for (index, group) in shown.iter().enumerate() {
    let (engine, id) = (engine.clone(), group.id.clone());
    loads.spawn(async move { (index, engine.topics(&id, Page::Latest).await) });
  }
  let mut pages: Vec<_> = shown.iter().map(|_| None).collect();
  while let Some(loaded) = loads.join_next().await {
    if let Ok((index, page)) = loaded {
      pages[index] = Some(page);
    }
  }

  let mut out = format!("{} {total} unread conversations", now_line());
  if total > shown.len() {
    let _ = write!(out, ", the {} most recent shown", shown.len());
  }
  let _ = writeln!(out, ".{note}");
  for (group, page) in shown.iter().zip(pages) {
    let page = match page {
      Some(Ok(page)) => page,
      Some(Err(error)) => {
        let _ = writeln!(out, "\n## {}\n({})", label(group), offline(error));
        continue;
      },
      None => continue,
    };
    let messages: Vec<&Arc<Message>> = page.topics.iter().flat_map(|(_, messages)| messages).collect();
    let new: Vec<&Arc<Message>> = messages
      .iter()
      .copied()
      .filter(|message| message.time > group.last_read_time && message.sender_id != me)
      .collect();
    // Marked unread by hand: nothing is newer than what was read.
    let (pool, count) = if new.is_empty() {
      (&messages, "marked unread, latest messages".to_string())
    } else {
      let more = if new.len() == messages.len() && !page.reached_start {
        "+"
      } else {
        ""
      };
      (&new, format!("{}{more} unread", new.len()))
    };
    let tail = &pool[pool.len().saturating_sub(each)..];
    let cut = if tail.len() < pool.len() {
      format!(", the last {} shown", tail.len())
    } else {
      String::new()
    };
    let _ = write!(
      out,
      "\n## {} — {count}{cut}\n{}",
      label(group),
      transcript(tail.iter().copied(), &me, None)
    );
  }
  Ok(keep_end(out))
}

/// A conversation, or a thread of a space, that `recent_activity` shows in full.
struct Involved<'a> {
  group: &'a Group,
  /// The thread to answer in; `None` for the main conversation.
  thread: Option<String>,
  /// Someone waits on a reply from the user.
  waiting: bool,
  /// When it began waiting, or when the user last wrote or reacted.
  time: Micros,
  /// Why it is shown: `Riley Brooks wrote 4 min ago`.
  why: String,
  messages: Vec<&'a Arc<Message>>,
}

/// How long ago, the way a person says it.
fn ago(time: Micros, now: Micros) -> String {
  match (now - time) / MINUTE {
    ..1 => "just now".into(),
    minutes @ ..60 => format!("{minutes} min ago"),
    minutes @ ..1440 => format!("{} h ago", minutes / 60),
    _ => format!("on {}", stamp(time)),
  }
}

fn sender(message: &Message) -> &str {
  if message.sender_name.is_empty() {
    "Someone"
  } else {
    &message.sender_name
  }
}

fn mentions(message: &Message, me: &str) -> bool {
  message.spans.iter().any(|span| match &span.kind {
    SpanKind::Mention(user) => user == me,
    SpanKind::MentionAll => true,
    _ => false,
  })
}

fn reacted(message: &Message) -> bool {
  message.reactions.iter().any(|reaction| reaction.mine)
}

/// The first message of a thread and its latest replies, up to `SHOWN_EACH`.
fn thread_view<'a>(messages: &[&'a Arc<Message>]) -> Vec<&'a Arc<Message>> {
  match messages {
    [] => Vec::new(),
    [first, rest @ ..] => {
      let tail = &rest[rest.len().saturating_sub(SHOWN_EACH - 1)..];
      std::iter::once(*first).chain(tail.iter().copied()).collect()
    },
  }
}

/// What in one conversation involves the user since `since`. A direct message or group chat waits on them when
/// its last message is someone else's that they did not react to. A space waits on them per thread: a mention
/// of them (or of everyone) with no reply or reaction of theirs after it. The rest of a space only counts in the
/// threads they wrote or reacted in; with none, the space is only a one-line stamp.
fn involvement<'a>(group: &'a Group, topics: &'a [Topic], me: &str, since: Micros, now: Micros) -> Vec<Involved<'a>> {
  let topics: Vec<Vec<&Arc<Message>>> = topics
    .iter()
    .map(|(_, messages)| messages.iter().filter(|message| !message.system).collect::<Vec<_>>())
    .filter(|messages| !messages.is_empty())
    .collect();
  let mine = |message: &Message| message.sender_id == me;
  if group.kind != GroupKind::Space {
    let messages: Vec<&Arc<Message>> = topics.iter().flatten().copied().collect();
    let Some(last) = messages.last().filter(|last| last.time >= since) else {
      return Vec::new();
    };
    let thread = last.is_thread_reply().then(|| last.topic_id.clone());
    let shown = messages[messages.len().saturating_sub(SHOWN_EACH)..].to_vec();
    if !mine(last) && !reacted(last) {
      let why = format!("{} wrote {}, no reply from you", sender(last), ago(last.time, now));
      return vec![Involved {
        group,
        thread,
        waiting: true,
        time: last.time,
        why,
        messages: shown,
      }];
    }
    let why = if mine(last) {
      format!("you wrote {}", ago(last.time, now))
    } else {
      format!("you reacted to the last message, {}", ago(last.time, now))
    };
    return vec![Involved {
      group,
      thread,
      waiting: false,
      time: last.time,
      why,
      messages: shown,
    }];
  }
  let mut found = Vec::new();
  for messages in &topics {
    let Some(first) = messages.first() else {
      continue;
    };
    let thread = Some(if first.topic_id.is_empty() {
      first.id.clone()
    } else {
      first.topic_id.clone()
    });
    let call = messages
      .iter()
      .rposition(|message| message.time >= since && !mine(message) && mentions(message, me));
    let answered =
      call.is_some_and(|at| reacted(messages[at]) || messages[at + 1..].iter().any(|message| mine(message)));
    if let Some(at) = call.filter(|_| !answered) {
      let message = messages[at];
      let why = format!(
        "{} mentioned you {}, no reply from you",
        sender(message),
        ago(message.time, now)
      );
      found.push(Involved {
        group,
        thread,
        waiting: true,
        time: message.time,
        why,
        messages: thread_view(messages),
      });
      continue;
    }
    let last_mine = messages
      .iter()
      .rev()
      .find(|message| message.time >= since && (mine(message) || reacted(message)));
    if let Some(message) = last_mine {
      let did = if mine(message) {
        "you wrote"
      } else {
        "you reacted"
      };
      let why = format!("{did} {}", ago(message.time, now));
      found.push(Involved {
        group,
        thread,
        waiting: false,
        time: message.time,
        why,
        messages: thread_view(messages),
      });
    }
  }
  found
}

/// A space that does not involve the user, in one line: `general (space/abc) — last activity 3 min ago, 4 people talking`.
fn stamp_line(group: &Group, topics: &[Topic], me: &str, now: Micros) -> String {
  let talking: HashSet<&str> = topics
    .iter()
    .flat_map(|(_, messages)| messages)
    .filter(|message| !message.system && message.sender_id != me && message.time >= now - TALKING)
    .map(|message| message.sender_id.as_str())
    .collect();
  let mut line = format!("{} — last activity {}", label(group), ago(group.sort_time, now));
  match talking.len() {
    0 => {},
    1 => line.push_str(", 1 person talking"),
    count => {
      let _ = write!(line, ", {count} people talking");
    },
  }
  line
}

async fn recent_activity(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: ActivityArgs = parse(arguments)?;
  let limit = args.limit.unwrap_or(10).clamp(1, 25);
  let me = engine.me().map(|me| me.id).unwrap_or_default();
  let now = Local::now().timestamp_micros();
  let since = now - RECENT;

  let groups: Vec<Group> = directory(engine)
    .into_iter()
    .map(|(group, _)| group)
    .filter(|group| group.kind != GroupKind::App && group.sort_time >= since)
    .take(SCANNED)
    .collect();
  if groups.is_empty() {
    return Ok(format!("{} Nothing happened in Google Chat in the last 24 hours.", now_line()));
  }
  let mut loads = JoinSet::new();
  for (index, group) in groups.iter().enumerate() {
    let (engine, id) = (engine.clone(), group.id.clone());
    loads.spawn(async move { (index, engine.topics(&id, Page::Latest).await) });
  }
  let mut pages: HashMap<usize, Vec<Topic>> = HashMap::new();
  let mut failed = Vec::new();
  while let Some(loaded) = loads.join_next().await {
    match loaded {
      Ok((index, Ok(page))) => {
        pages.insert(index, page.topics);
      },
      Ok((index, Err(error))) => failed.push((index, offline(error))),
      Err(_) => {},
    }
  }

  let mut involved = Vec::new();
  let mut stamps = Vec::new();
  for (index, group) in groups.iter().enumerate() {
    let Some(topics) = pages.get(&index) else {
      continue;
    };
    let found = involvement(group, topics, &me, since, now);
    if found.is_empty() {
      if group.kind == GroupKind::Space && !group.muted {
        stamps.push(stamp_line(group, topics, &me, now));
      }
    } else {
      involved.extend(found);
    }
  }
  involved.sort_by_key(|entry| (!entry.waiting, Reverse(entry.time)));
  let waiting = involved.iter().filter(|entry| entry.waiting).count();

  let mut out = format!("{}\n", now_line());
  match waiting {
    0 => out.push_str("Nothing waits on a reply from the user.\n"),
    1 => out.push_str("1 conversation waits on a reply from the user.\n"),
    count => {
      let _ = writeln!(
        out,
        "{count} conversations wait on a reply from the user. Send to the one whose question the user's request \
         answers, whatever its age. If it answers more than one, do not send: ask which one, or all of them."
      );
    },
  }
  let shown = involved.len().min(limit);
  for (index, entry) in involved.iter().take(limit).enumerate() {
    if out.len() > MAX_ANSWER {
      let _ = writeln!(
        out,
        "\n({} more left out to keep this short: pass a lower `limit`.)",
        shown - index
      );
      break;
    }
    if index == 0 && entry.waiting {
      out.push_str("\n## Waiting on you\n");
    }
    if !entry.waiting && involved.get(index.wrapping_sub(1)).is_none_or(|previous| previous.waiting) {
      out.push_str("\n## You took part\n");
    }
    let send = match &entry.thread {
      Some(thread) => format!("send with thread {thread}"),
      None => "send without thread".to_string(),
    };
    let _ = write!(
      out,
      "### {} — {} ({send})\n{}",
      label(entry.group),
      entry.why,
      transcript(entry.messages.iter().copied(), &me, None)
    );
  }
  if involved.len() > limit {
    let _ = writeln!(out, "\n({} more not shown: pass a higher `limit`.)", involved.len() - limit);
  }
  if !stamps.is_empty() {
    out.push_str("\n## Other activity (not involving the user)\n");
    for stamp in stamps.iter().take(STAMPS) {
      let _ = writeln!(out, "{stamp}");
    }
  }
  for (index, error) in failed {
    if let Some(group) = groups.get(index) {
      let _ = writeln!(out, "({}: {error})", label(group));
    }
  }
  Ok(out)
}

fn find_conversations(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: FindArgs = parse(arguments)?;
  let limit = args.limit.unwrap_or(15).clamp(1, 50);
  let all = directory(engine);
  let wanted = |group: &Group| {
    let kind = match args.kind.unwrap_or_default() {
      Kind::Any => true,
      Kind::Dm => group.kind == GroupKind::Dm,
      Kind::Group => group.kind == GroupKind::GroupDm,
      Kind::Space => group.kind == GroupKind::Space,
      Kind::App => group.kind == GroupKind::App,
    };
    kind && (!args.unread_only || group.unread)
  };
  let pool = all.iter().filter(|(group, _)| wanted(group));
  let (found, order): (Vec<&Group>, _) = match &args.query {
    Some(query) => {
      let hits = Fuzzy::new(query).rank(pool, |(_, text)| text.as_str());
      (hits.into_iter().map(|(_, entry)| &entry.0).collect(), "best match first")
    },
    None => (pool.map(|(group, _)| group).collect(), "most recently active first"),
  };
  if found.is_empty() {
    return Ok(format!(
      "No conversation matches. There are {} in all: try fewer letters, or leave `query` out to list the most recent.",
      all.len()
    ));
  }
  let mut out = format!("{} of {} conversations, {order}:\n", found.len().min(limit), found.len());
  for group in found.into_iter().take(limit) {
    let _ = write!(out, "- {}", label(group));
    if group.unread {
      out.push_str(", unread");
    }
    if group.muted {
      out.push_str(", muted");
    }
    if let Some(snippet) = &group.snippet {
      let sender = if snippet.sender_name.is_empty() {
        "Someone"
      } else {
        &snippet.sender_name
      };
      let _ = write!(out, " — last {} {sender}: {}", stamp(snippet.time), clip(&snippet.text, 80));
    }
    out.push('\n');
  }
  Ok(out)
}

async fn read_conversation(engine: &Arc<Engine>, reads: &Reads, arguments: Value) -> Result<String, String> {
  let args: ReadArgs = parse(arguments)?;
  let group = resolve(&directory(engine), &args.conversation, false)?;
  let limit = args.limit.unwrap_or(30).clamp(1, 100);
  let me = engine.me().map(|me| me.id).unwrap_or_default();
  let thread = args
    .thread
    .as_deref()
    .map(|thread| thread.trim_start_matches('#'))
    .filter(|thread| !thread.is_empty());
  if let Some(thread) = thread {
    if args.before.is_some() || args.around.is_some() {
      return Err("`thread` reads one whole thread: leave out `before` and `around`.".into());
    }
    let answer = read_thread(engine, &group, thread, &me).await?;
    reads.mark(&group.id);
    return Ok(answer);
  }

  let mut around = None;
  let (topics, reached_start, reached_end) = match (&args.around, &args.before) {
    (Some(point), _) => {
      let time = cursor(point)?;
      around = Some(time);
      // The page comes centered on that point. Keep as much before it as after.
      let page = engine.topics(&group.id, Page::At(time)).await.map_err(offline)?;
      let mut topics = page.topics;
      let mut later = topics.split_off(topics.partition_point(|(sort_time, _)| *sort_time <= time));
      let cut_start = keep_newest(&mut topics, limit.div_ceil(2));
      let cut_end = keep_oldest(&mut later, limit / 2);
      topics.append(&mut later);
      (topics, page.reached_start && !cut_start, page.reached_end && !cut_end)
    },
    (None, before) => {
      let page = match before {
        Some(point) => Page::Before(cursor(point)?),
        None => Page::Latest,
      };
      let first = engine.topics(&group.id, page).await.map_err(offline)?;
      let mut reached_start = first.reached_start || (before.is_some() && first.topics.is_empty());
      let mut topics = first.topics;
      // A page of topics can hold fewer messages than asked for: read further back, a few times at most.
      for _ in 0..3 {
        let Some(oldest) = topics.first().map(|(time, _)| *time) else {
          break;
        };
        if reached_start || held(&topics) >= limit {
          break;
        }
        let mut older = engine.topics(&group.id, Page::Before(oldest)).await.map_err(offline)?;
        reached_start = older.reached_start || older.topics.is_empty();
        older.topics.append(&mut topics);
        topics = older.topics;
      }
      let cut = keep_newest(&mut topics, limit);
      (topics, reached_start && !cut, true)
    },
  };

  reads.mark(&group.id);
  let messages: Vec<&Arc<Message>> = topics.iter().flat_map(|(_, messages)| messages).collect();
  if messages.is_empty() {
    return Ok(format!(
      "{}: no messages{}.",
      label(&group),
      if args.before.is_some() {
        " before that point"
      } else {
        ""
      }
    ));
  }
  // The message asked for is the one written at that time, or the nearest to it.
  let focus = around
    .and_then(|time| messages.iter().min_by_key(|message| (message.time - time).abs()))
    .map(|message| message.id.as_str());
  let mut out = format!("{}. {} messages, oldest first. {}", label(&group), messages.len(), now_line());
  if focus.is_some() {
    out.push_str(" >> marks the message asked for.");
  }
  out.push('\n');
  out.push_str(&transcript(messages.iter().copied(), &me, focus));
  match topics.first() {
    Some((oldest, _)) if !reached_start => {
      let _ = writeln!(out, "Older messages: call read_conversation with before=\"{oldest}\".");
    },
    _ => out.push_str("This is the start of the conversation.\n"),
  }
  if let Some((newest, _)) = topics.last().filter(|_| !reached_end) {
    let _ = writeln!(
      out,
      "Newer messages: call read_conversation with around=\"{newest}\", or without `around` for the latest."
    );
  }
  Ok(keep_end(out))
}

/// One thread of a conversation, found among its latest pages.
async fn read_thread(engine: &Arc<Engine>, group: &Group, thread: &str, me: &str) -> Result<String, String> {
  let in_thread = |messages: &Vec<Arc<Message>>| {
    messages
      .first()
      .is_some_and(|first| first.id == thread || first.topic_id == thread)
  };
  let mut page = engine.topics(&group.id, Page::Latest).await.map_err(offline)?;
  // The thread is usually recent: look a few pages back at most.
  for _ in 0..4 {
    if let Some((_, messages)) = page.topics.iter().find(|(_, messages)| in_thread(messages)) {
      return Ok(format!(
        "{}, thread #{thread}. {} messages, oldest first. {}\n{}",
        label(group),
        messages.len(),
        now_line(),
        transcript(messages.iter(), me, None)
      ));
    }
    let Some(oldest) = page.topics.first().map(|(time, _)| *time).filter(|_| !page.reached_start) else {
      break;
    };
    page = engine.topics(&group.id, Page::Before(oldest)).await.map_err(offline)?;
  }
  Err(format!(
    "No thread #{thread} among the recent messages of {}. Call read_conversation without `thread` to see the conversation.",
    label(group)
  ))
}

async fn search_messages(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: SearchArgs = parse(arguments)?;
  let query = args.query.trim();
  if query.is_empty() {
    return Err("`query` is empty. Pass the words to look for.".into());
  }
  let limit = args.limit.unwrap_or(10).clamp(1, 20);
  let all = directory(engine);
  let only = match &args.conversation {
    Some(reference) => Some(resolve(&all, reference, false)?),
    None => None,
  };
  let me = engine.me().map(|me| me.id).unwrap_or_default();

  let mut hits = engine.search_hits(query).await.map_err(offline)?;
  let filtered = only.is_some() || args.from.is_some();
  if let Some(group) = &only {
    hits.retain(|hit| hit.group == group.id);
  }
  match args.from.as_deref() {
    Some(from) if ["me", "myself", "you"].contains(&from.to_lowercase().as_str()) => {
      hits.retain(|hit| hit.sender_id == me);
    },
    Some(from) => {
      let mut person = Fuzzy::new(from);
      hits.retain(|hit| person.score(&hit.sender_name).is_some());
    },
    None => {},
  }
  if hits.is_empty() {
    let narrowed = if filtered {
      " The filters apply to the best matches overall: add words to `query` to narrow it instead."
    } else {
      ""
    };
    return Ok(format!(
      "No message matches \"{query}\". The search matches whole words: try fewer or other words.{narrowed}"
    ));
  }
  let mut out = format!("{} messages match \"{query}\":\n", hits.len().min(limit));
  for (index, hit) in hits.iter().take(limit).enumerate() {
    let id = id_text(&hit.group);
    let place = all
      .iter()
      .find(|(group, _)| group.id == hit.group)
      .map_or(id.clone(), |(group, _)| format!("{} ({id})", group.name));
    let sender = match () {
      () if hit.sender_id == me => "You",
      () if hit.sender_name.is_empty() => "Someone",
      () => &hit.sender_name,
    };
    let _ = writeln!(
      out,
      "{}. [{}] {sender} in {place}: {}",
      index + 1,
      stamp(hit.time),
      clip(&hit.text, 300)
    );
    let _ = writeln!(
      out,
      "   around it: read_conversation conversation=\"{id}\" around=\"{}\"",
      hit.time
    );
  }
  Ok(out)
}

async fn send_message(engine: &Arc<Engine>, reads: &Reads, arguments: Value) -> Result<String, String> {
  let args: SendArgs = parse(arguments)?;
  let text = args.text.trim();
  if text.is_empty() && args.files.is_empty() {
    return Err("Nothing was sent: give `text`, `files`, or both.".into());
  }
  let mut files = Vec::with_capacity(args.files.len());
  for file in &args.files {
    let path = match (file.strip_prefix("~/"), std::env::var_os("HOME")) {
      (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
      _ => PathBuf::from(file),
    };
    if !path.is_absolute() {
      return Err(format!("Nothing was sent: \"{file}\" is not an absolute path."));
    }
    if !path.is_file() {
      return Err(format!("Nothing was sent: there is no file at \"{file}\"."));
    }
    files.push(path);
  }
  let group =
    resolve(&directory(engine), &args.conversation, true).map_err(|why| format!("Nothing was sent. {why}"))?;
  if !reads.has(&group.id) {
    return Err(format!(
      "Nothing was sent. Read {} with read_conversation first, then write the message to fit it.",
      label(&group)
    ));
  }
  let thread = args
    .thread
    .as_deref()
    .map(|thread| thread.trim_start_matches('#'))
    .filter(|thread| !thread.is_empty());
  let attached = match files.len() {
    0 => String::new(),
    1 => " with 1 file".to_string(),
    count => format!(" with {count} files"),
  };
  engine
    .send_now(&group.id, thread, text, files)
    .await
    .map_err(|error| format!("Nothing was sent: {error:#}."))?;
  reads.clear();
  Ok(format!(
    "Sent to {}{}{attached}.",
    label(&group),
    if thread.is_some() {
      ", in that thread"
    } else {
      ""
    }
  ))
}

#[cfg(test)]
mod tests {
  use super::{
    InboxArgs, MINUTE, ago, clip, cursor, id_text, involvement, keep_newest, keep_oldest, list, parse, parse_id,
  };
  use crate::model::{Group, GroupId, GroupKind, Message, Micros, Notify, Placement, Reaction, Span, SpanKind};
  use serde_json::json;
  use std::sync::Arc;

  fn topic(time: i64, n: usize) -> (i64, Vec<Arc<Message>>) {
    let messages = (0..n)
      .map(|i| {
        Arc::new(Message {
          id: format!("{time}-{i}"),
          topic_id: format!("{time}"),
          group: GroupId::Dm("g".into()),
          sender_id: "u".into(),
          sender_name: "U".into(),
          sender_avatar: None,
          text: String::new(),
          spans: Vec::new(),
          time,
          update_time: 0,
          edited: false,
          system: false,
          reactions: Vec::new(),
          attachments: Vec::new(),
          cards: Vec::new(),
          quote: None,
          starred: false,
          pending: false,
          failed: false,
        })
      })
      .collect();
    (time, messages)
  }

  const ME: &str = "me";
  const NOW: Micros = 1_000_000 * MINUTE;
  const SINCE: Micros = NOW - 24 * 60 * MINUTE;

  fn group(kind: GroupKind) -> Group {
    Group {
      id: GroupId::Space("s".into()),
      kind,
      name: "Field team".into(),
      avatar_url: None,
      emoji: None,
      member_ids: Vec::new(),
      member_avatars: Vec::new(),
      sort_time: NOW,
      last_read_time: NOW,
      unread: false,
      pinned: false,
      muted: false,
      notify: Notify::All,
      snippet: None,
      placement: Placement::ByKind,
    }
  }

  /// A message `minutes` before now, in thread `topic` (its own id starts the thread).
  fn said(id: &str, topic_id: &str, sender: &str, minutes: i64) -> Message {
    Message {
      id: id.into(),
      topic_id: topic_id.into(),
      sender_id: sender.into(),
      sender_name: if sender == ME {
        "Me".into()
      } else {
        "Jordan Park".into()
      },
      time: NOW - minutes * MINUTE,
      ..topic(0, 1).1[0].as_ref().clone()
    }
  }

  fn mentioning(mut message: Message) -> Message {
    message.text = "@Me can you check?".into();
    message.spans = vec![Span {
      range: 0..3,
      kind: SpanKind::Mention(ME.into()),
    }];
    message
  }

  fn thread(messages: Vec<Message>) -> (Micros, Vec<Arc<Message>>) {
    (
      messages.last().map_or(0, |message| message.time),
      messages.into_iter().map(Arc::new).collect(),
    )
  }

  #[test]
  fn a_direct_message_waits_until_the_user_answers_or_reacts() {
    let dm = group(GroupKind::Dm);
    let asked = vec![thread(vec![said("a", "a", ME, 30)]), thread(vec![said("b", "b", "jp", 4)])];
    let found = involvement(&dm, &asked, ME, SINCE, NOW);
    assert!(found[0].waiting);
    assert_eq!(found[0].why, "Jordan Park wrote 4 min ago, no reply from you");
    assert_eq!(found[0].thread, None);

    let mut thumbs = said("b", "b", "jp", 4);
    thumbs.reactions = vec![Reaction {
      emoji: "👍".into(), count: 1, mine: true
    }];
    let reacted = [thread(vec![thumbs])];
    let found = involvement(&dm, &reacted, ME, SINCE, NOW);
    assert!(!found[0].waiting);
  }

  #[test]
  fn a_space_waits_only_on_an_unanswered_mention() {
    let space = group(GroupKind::Space);
    let topics = vec![
      thread(vec![said("t1", "t1", "jp", 50), mentioning(said("r1", "t1", "jp", 6))]),
      thread(vec![
        said("t2", "t2", "jp", 40),
        mentioning(said("r2", "t2", "jp", 30)),
        said("r3", "t2", ME, 20),
      ]),
      thread(vec![said("t3", "t3", "jp", 3), said("r4", "t3", "rb", 2)]),
    ];
    let found = involvement(&space, &topics, ME, SINCE, NOW);
    assert_eq!(found.len(), 2, "the chatter in t3 is only a stamp");
    assert!(found[0].waiting);
    assert_eq!(found[0].thread.as_deref(), Some("t1"));
    assert!(!found[1].waiting, "the user answered in t2");
    assert_eq!(found[1].why, "you wrote 20 min ago");
  }

  #[test]
  fn a_space_without_the_user_is_not_involved() {
    let space = group(GroupKind::Space);
    let topics = vec![thread(vec![said("t1", "t1", "jp", 5), said("r1", "t1", "rb", 1)])];
    assert!(involvement(&space, &topics, ME, SINCE, NOW).is_empty());
    let old = vec![thread(vec![mentioning(said("t2", "t2", "jp", 25 * 60))])];
    assert!(involvement(&space, &old, ME, SINCE, NOW).is_empty(), "older than a day");
  }

  #[test]
  fn ago_reads_like_a_person() {
    assert_eq!(ago(NOW - 30_000_000, NOW), "just now");
    assert_eq!(ago(NOW - 4 * MINUTE, NOW), "4 min ago");
    assert_eq!(ago(NOW - 130 * MINUTE, NOW), "2 h ago");
  }

  #[test]
  fn parse_id_accepts_slash_and_underscore() {
    assert_eq!(parse_id("dm/abc"), Some(GroupId::Dm("abc".into())));
    assert_eq!(parse_id("space_xyz"), Some(GroupId::Space("xyz".into())));
    assert_eq!(parse_id("room/abc"), None);
    assert_eq!(id_text(&GroupId::Dm("abc".into())), "dm/abc");
  }

  #[test]
  fn clip_collapses_space_and_cuts_on_a_char() {
    assert_eq!(clip("hello   world", 20), "hello world");
    assert_eq!(clip("éééé", 2), "éé…");
  }

  #[test]
  fn cursor_reads_micros_and_dates() {
    assert_eq!(cursor("1700000000000000").unwrap(), 1_700_000_000_000_000);
    assert!(cursor("not-a-time").is_err());
    let day = cursor("2026-11-01").expect("date");
    assert!(day > 0);
  }

  #[test]
  fn keep_newest_drops_whole_topics() {
    let mut topics = vec![topic(1, 2), topic(2, 2), topic(3, 2)];
    assert!(keep_newest(&mut topics, 3));
    assert_eq!(topics.iter().map(|(t, _)| *t).collect::<Vec<_>>(), vec![2, 3]);
    assert!(!keep_newest(&mut topics, 50));
  }

  #[test]
  fn keep_oldest_drops_from_the_end() {
    let mut topics = vec![topic(1, 2), topic(2, 2), topic(3, 2)];
    assert!(keep_oldest(&mut topics, 3));
    assert_eq!(topics.iter().map(|(t, _)| *t).collect::<Vec<_>>(), vec![1, 2]);
  }

  #[test]
  fn send_tool_is_gated() {
    let names = |allow| {
      list(allow)
        .iter()
        .filter_map(|tool| tool.get("name").and_then(|name| name.as_str()).map(str::to_string))
        .collect::<Vec<_>>()
    };
    assert!(!names(false).iter().any(|name| name == "send_message"));
    assert!(names(true).iter().any(|name| name == "send_message"));
  }

  #[test]
  fn loose_args_take_strings() {
    let args: InboxArgs = parse(json!({ "limit": "3", "include_quiet": "true" })).unwrap();
    assert_eq!(args.limit, Some(3));
    assert!(args.include_quiet);
  }
}
