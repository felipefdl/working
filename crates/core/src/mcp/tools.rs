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
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use tokio::task::JoinSet;

use crate::engine::{Engine, Page};
use crate::fuzzy::Fuzzy;
use crate::model::{AttachmentKind, Group, GroupId, GroupKind, Message, Micros, Notify};

/// System prompt for MCP clients: how to name conversations and that reads do not mark messages read.
pub const INSTRUCTIONS: &str = "Google Chat of the person signed in to the Working app on this Mac. \
Catch up with inbox. Find a chat with find_conversations, read it with read_conversation, and find a word or \
topic across every chat with search_messages. Wherever a `conversation` is asked for, a name (\"Ana Lima\"), \
part of one, or an id (dm/…, space/…) works. Times are this Mac's local time. Reading changes nothing: \
messages stay unread.";

/// Longest answer of a tool, in bytes: about 12,000 tokens.
const MAX_ANSWER: usize = 48_000;

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
      "name": "find_conversations",
      "title": "Find chats",
      "description": "Find conversations (direct messages, group chats, spaces, apps) by name and get their \
    ids. The query is fuzzy: part of a name, the first letters of its words, or a person's email all work \
    (\"fab eger\", \"prod team\"). Without a query it lists the most recently active conversations. Use it to \
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
    previous answer. To see what surrounds a search result, pass its `around` value. To find a word or topic, \
    call search_messages instead of paging back.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "conversation": { "type": "string", "description": "The conversation's id (dm/… or space/…) or name. Part of a name works when it points at one conversation." },
          "limit": { "type": "integer", "description": "About how many messages. Default 30, at most 100." },
          "before": { "type": "string", "description": "Read the messages before this point: the value a previous answer gave, or a local date or time such as 2026-11-01 or 2026-11-01T14:30." },
          "around": { "type": "string", "description": "Read the messages around this point: the value a search_messages result gave." }
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
With `thread` the message is a reply inside that thread of a space; without it, it goes to the main conversation.",
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
pub async fn call(engine: &Arc<Engine>, allow_send: bool, name: &str, arguments: Value) -> Option<Value> {
  let outcome = match name {
    "inbox" => inbox(engine, arguments).await,
    "find_conversations" => find_conversations(engine, arguments),
    "read_conversation" => read_conversation(engine, arguments).await,
    "search_messages" => search_messages(engine, arguments).await,
    "send_message" if allow_send => send_message(engine, arguments).await,
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

/// A conversation as answers name it: `Ana Lima (direct message, dm/abc)`.
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
    parts.extend(card.lines.iter().cloned());
    parts.extend(card.links.iter().map(|(label, url)| format!("{label}: {url}")));
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

async fn read_conversation(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: ReadArgs = parse(arguments)?;
  let group = resolve(&directory(engine), &args.conversation, false)?;
  let limit = args.limit.unwrap_or(30).clamp(1, 100);
  let me = engine.me().map(|me| me.id).unwrap_or_default();

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

async fn send_message(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
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
  use super::{InboxArgs, clip, cursor, id_text, keep_newest, keep_oldest, list, parse, parse_id};
  use crate::model::{GroupId, Message};
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
          pending: false,
          failed: false,
        })
      })
      .collect();
    (time, messages)
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
