//! The mail server (`/mcp/mail`): the signed-in account's Gmail. Reading is always offered; the tools that send,
//! reply, archive, and mark are listed and run only when the user let agents write mail (`McpConfig::mail_write`), and
//! never under `WORKING_READONLY=1`. Every tool answers only while Mail is on in the app (`Engine::mail_on`);
//! otherwise it answers that Mail is off, without contacting Gmail.
#![allow(clippy::too_many_lines, reason = "tool schemas are verbose JSON")]

use std::fmt::Write as _;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use super::shared::{
  answer, clip, keep_end, loose_count, loose_flag, loose_switch, loose_text, no_arguments, now_line, parse, read_only,
  stamp, writes,
};
use crate::engine::Engine;
use crate::model::{MailAddress, MailCompose, MailLabel, MailMessage, MailReply, MailThread, Mailbox};

/// The server's name.
pub(super) const NAME: &str = "working-mail";

/// System prompt for MCP clients of this server.
pub(super) const INSTRUCTIONS: &str = "Gmail of the person signed in to the Working app on this computer. \
mail_inbox lists a mailbox (inbox, starred, sent, all) or one of the user's labels, mail_labels names the labels, \
mail_search finds emails with Gmail's own search words, and mail_read_thread reads one thread. Reading changes \
nothing: threads stay unread. When the user allowed it, mail_send sends a new email, mail_reply answers in a thread, \
mail_archive takes a thread out of the inbox or puts it back, and mail_mark marks a thread read or unread; these are \
real and a sent email reaches its recipients at once. Times are this computer's local time. The tools answer only \
while Mail is on in Working.";

/// The tools that read, each answering only while Mail is on (`Engine::mail_on`).
const TOOLS: [&str; 4] = ["mail_inbox", "mail_search", "mail_read_thread", "mail_labels"];

/// The tools that change mail: listed and run only with `mail_write`.
const WRITE_TOOLS: [&str; 4] = ["mail_send", "mail_reply", "mail_archive", "mail_mark"];

/// What a tool answers while Mail is off. Nothing contacts Gmail then.
const MAIL_OFF: &str = "Mail is turned off in Working.";
/// What a write tool answers while the user has not let agents write mail.
const WRITE_OFF: &str = "Writing mail is turned off. The user can allow it in Working: Settings, Agents.";
/// What a write tool answers under `WORKING_READONLY=1`.
const READ_ONLY_MODE: &str = "Working runs in read-only mode: nothing was changed.";

/// Tool descriptors; the ones that change mail only when `writable`.
pub(super) fn list(writable: bool) -> Vec<Value> {
  let read_only = read_only();
  let mut tools = vec![
    json!({
      "name": "mail_inbox",
      "title": "Gmail threads",
      "description": "List the newest email threads of the user's Gmail: the inbox by default, or the starred, \
    sent, or all mail. Each thread says whether it is unread, who wrote it, when, its subject, and the start of its \
    newest email, and gives the mail_read_thread call that reads it. Reading changes nothing in Gmail: threads stay \
    unread. To find particular emails, call mail_search instead. Answers only while Mail is on in Working.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "mailbox": { "type": "string", "enum": ["inbox", "starred", "sent", "all"], "description": "Which threads. Default inbox. Leave out with `label`." },
          "label": { "type": "string", "description": "List one of the user's labels instead, by name as mail_labels gives it (\"Clients/Acme\"); part of a name works when it points at one label." },
          "limit": { "type": "integer", "description": "Most threads to list. Default 20, at most 50." },
          "unread_only": { "type": "boolean", "description": "Only unread threads. Default false." }
        },
        "additionalProperties": false
      },
      "annotations": read_only,
    }),
    json!({
      "name": "mail_search",
      "title": "Search Gmail",
      "description": "Search the user's Gmail with Gmail's own search words, as in its search box: plain words, \
    from:jpark@example.com, to:, subject:, has:attachment, is:unread, after:2026/11/01, older_than:7d, and \
    combinations of them. Answers the matching threads, newest first, like mail_inbox, each with the mail_read_thread \
    call that reads it. Answers only while Mail is on in Working.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "query": { "type": "string", "description": "Gmail search, such as `from:jpark@example.com has:attachment`." },
          "limit": { "type": "integer", "description": "Most threads to list. Default 20, at most 50." }
        },
        "required": ["query"],
        "additionalProperties": false
      },
      "annotations": read_only,
    }),
    json!({
      "name": "mail_read_thread",
      "title": "Read an email thread",
      "description": "Read the emails of one Gmail thread, oldest first, as plain text: who wrote each one, to and \
    copied whom, when, the names of its attached files, and its body. Take the thread id from mail_inbox or \
    mail_search. Reading leaves the thread unread. Answers only while Mail is on in Working.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "thread": { "type": "string", "description": "The thread id that mail_inbox or mail_search gave, such as thread-f:1794896454636." }
        },
        "required": ["thread"],
        "additionalProperties": false
      },
      "annotations": read_only,
    }),
    json!({
      "name": "mail_labels",
      "title": "Gmail labels",
      "description": "List the user's own Gmail labels by name, nested ones as Parent/Child, each with how many of \
    its threads are unread. To list a label's threads, call mail_inbox with its `label`. Answers only while Mail is on \
    in Working.",
      "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
      "annotations": read_only,
    }),
  ];
  if !writable {
    return tools;
  }
  let thread =
    json!({ "type": "string", "description": "The thread id that mail_inbox, mail_search, or mail_send gave." });
  let signature = json!({ "type": "boolean", "description": "Add the signature the user set in Gmail, as Gmail does. Default true." });
  tools.extend([
    json!({
      "name": "mail_send",
      "title": "Send an email",
      "description": "Send a new email as the user, from their Gmail address. It is sent for real and the recipients \
get it at once: send only what the user asked for. Give at least one address in `to` or `cc`, such as \
jpark@example.com or Jordan Park <jpark@example.com>. The text is plain: a blank line starts a paragraph. Answers the \
new thread's id. Answers only while Mail is on in Working.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "to": { "type": "array", "items": { "type": "string" }, "description": "Recipients' addresses." },
          "cc": { "type": "array", "items": { "type": "string" }, "description": "Who gets a copy." },
          "subject": { "type": "string" },
          "text": { "type": "string", "description": "The email, as plain text." },
          "signature": signature
        },
        "required": ["to", "text"],
        "additionalProperties": false
      },
      "annotations": writes(false, false),
    }),
    json!({
      "name": "mail_reply",
      "title": "Reply to an email",
      "description": "Answer an email in its thread as the user, quoting it as Gmail does. It is sent for real and \
the recipients get it at once: send only what the user asked for. It answers the thread's newest email from someone \
else unless `message` names one, and goes where Gmail's Reply (or Reply all, with reply_all) sends it: the recipients \
come from the email answered, never from the call. The text is plain: a blank line starts a paragraph. Answers who it \
went to. Answers only while Mail is on in Working.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "thread": thread,
          "text": { "type": "string", "description": "The reply, as plain text." },
          "reply_all": { "type": "boolean", "description": "Answer everyone on the email, as Reply all. Default false." },
          "message": { "type": "string", "description": "The email to answer: its id from mail_read_thread. Default the newest email from someone else." },
          "signature": signature
        },
        "required": ["thread", "text"],
        "additionalProperties": false
      },
      "annotations": writes(false, false),
    }),
    json!({
      "name": "mail_archive",
      "title": "Archive a thread",
      "description": "Take a thread out of the user's inbox, as Gmail's Archive; it stays in All Mail. With archive \
false, move it back to the inbox. The change is real and shows in Gmail and Working at once. Answers only while Mail \
is on in Working.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "thread": thread,
          "archive": { "type": "boolean", "description": "True archives, false moves it back to the inbox. Default true." }
        },
        "required": ["thread"],
        "additionalProperties": false
      },
      "annotations": writes(false, true),
    }),
    json!({
      "name": "mail_mark",
      "title": "Mark a thread read or unread",
      "description": "Mark a thread read or unread, as Gmail's Mark as read and Mark as unread. Reading a thread with \
the other tools leaves it unread; this is the way to change that. The change is real and shows in Gmail and Working \
at once. Answers only while Mail is on in Working.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "thread": thread,
          "read": { "type": "boolean", "description": "True marks it read, false unread." }
        },
        "required": ["thread", "read"],
        "additionalProperties": false
      },
      "annotations": writes(false, true),
    }),
  ]);
  tools
}

/// What decides whether a tool may run now.
#[derive(Clone, Copy)]
struct Gate {
  /// The user let agents write mail (`McpConfig::mail_write`).
  writable: bool,
  /// `WORKING_READONLY=1`.
  readonly: bool,
  /// Mail is on in the app (`Engine::mail_on`).
  mail_on: bool,
}

/// Why a tool may not run now, before anything contacts Gmail: a write the user did not allow, read-only mode, or
/// Mail off.
fn refusal(write: bool, gate: Gate) -> Option<&'static str> {
  if write && !gate.writable {
    return Some(WRITE_OFF);
  }
  if write && gate.readonly {
    return Some(READ_ONLY_MODE);
  }
  (!gate.mail_on).then_some(MAIL_OFF)
}

/// Runs a tool. `None` when there is no tool of that name. Mail turns on and off while the server runs, and clients
/// keep the tool list, so the tools stay listed and refuse instead.
pub(super) async fn call(engine: &Arc<Engine>, writable: bool, name: &str, arguments: Value) -> Option<Value> {
  let write = WRITE_TOOLS.contains(&name);
  if !write && !TOOLS.contains(&name) {
    return None;
  }
  let gate = Gate {
    writable,
    readonly: engine.readonly(),
    mail_on: engine.mail_on(),
  };
  if let Some(refused) = refusal(write, gate) {
    return Some(answer(Err(refused.into())));
  }
  let outcome = match name {
    "mail_inbox" => mail_inbox(engine, arguments).await,
    "mail_search" => mail_search(engine, arguments).await,
    "mail_read_thread" => mail_read_thread(engine, arguments).await,
    "mail_send" => mail_send(engine, arguments).await,
    "mail_reply" => mail_reply(engine, arguments).await,
    "mail_archive" => mail_archive(engine, arguments).await,
    "mail_mark" => mail_mark(engine, arguments).await,
    _ => mail_labels(engine, &arguments).await,
  };
  Some(answer(outcome))
}

// ---- Arguments ----

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum MailboxArg {
  #[default]
  Inbox,
  Starred,
  Sent,
  All,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailInboxArgs {
  #[serde(default)]
  mailbox: Option<MailboxArg>,
  #[serde(default, deserialize_with = "loose_text")]
  label: Option<String>,
  #[serde(default, deserialize_with = "loose_count")]
  limit: Option<usize>,
  #[serde(default, deserialize_with = "loose_flag")]
  unread_only: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailSearchArgs {
  query: String,
  #[serde(default, deserialize_with = "loose_count")]
  limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailReadArgs {
  thread: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailSendArgs {
  #[serde(default, deserialize_with = "loose_addresses")]
  to: Vec<String>,
  #[serde(default, deserialize_with = "loose_addresses")]
  cc: Vec<String>,
  #[serde(default)]
  subject: String,
  #[serde(default)]
  text: String,
  #[serde(default, deserialize_with = "loose_switch")]
  signature: Option<bool>,
}

/// `mail_reply`'s arguments. There are no recipients: they come from the email answered.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailReplyArgs {
  thread: String,
  #[serde(default)]
  text: String,
  #[serde(default, deserialize_with = "loose_flag")]
  reply_all: bool,
  #[serde(default, deserialize_with = "loose_text")]
  message: Option<String>,
  #[serde(default, deserialize_with = "loose_switch")]
  signature: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailArchiveArgs {
  thread: String,
  #[serde(default, deserialize_with = "loose_switch")]
  archive: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailMarkArgs {
  thread: String,
  #[serde(default, deserialize_with = "loose_switch")]
  read: Option<bool>,
}

/// Addresses as a list, or as one text with commas or semicolons between them.
fn loose_addresses<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
  #[derive(Deserialize)]
  #[serde(untagged)]
  enum Loose {
    One(String),
    Many(Vec<String>),
  }
  let all = match Option::<Loose>::deserialize(deserializer)? {
    None => Vec::new(),
    Some(Loose::One(text)) => vec![text],
    Some(Loose::Many(list)) => list,
  };
  Ok(
    all
      .iter()
      .flat_map(|text| text.split([',', ';']))
      .map(str::trim)
      .filter(|text| !text.is_empty())
      .map(str::to_string)
      .collect(),
  )
}

/// An address as written: `Jordan Park <jpark@example.com>`, `"Jordan Park" <jpark@example.com>`, or the address
/// alone.
fn address_of(text: &str) -> MailAddress {
  let text = text.trim();
  match text.rsplit_once('<') {
    Some((name, rest)) if rest.ends_with('>') => MailAddress {
      name: name.trim().trim_matches('"').trim().to_string(),
      email: rest.trim_end_matches('>').trim().to_string(),
    },
    _ => MailAddress {
      name: String::new(),
      email: text.to_string(),
    },
  }
}

/// A thread id as the tools take it: as given, `#` dropped, a bare number taken as Gmail's `thread-f:` id.
fn thread_arg(raw: &str) -> Result<String, String> {
  let thread = raw.trim().trim_start_matches('#');
  if thread.is_empty() {
    return Err("`thread` is empty. Pass the thread id that mail_inbox or mail_search gave.".into());
  }
  Ok(if thread.bytes().all(|byte| byte.is_ascii_digit()) {
    format!("thread-f:{thread}")
  } else {
    thread.to_string()
  })
}

// ---- Tools ----

fn mail_offline(error: &anyhow::Error) -> String {
  format!("Gmail could not be read: {error:#}. Try again in a moment.")
}

/// Someone as an email line names them: `Jordan Park <jpark@example.com>`, or the address alone.
fn person(address: &MailAddress) -> String {
  if address.name.is_empty() {
    address.email.clone()
  } else {
    format!("{} <{}>", address.name, address.email)
  }
}

fn people(addresses: &[MailAddress]) -> String {
  addresses.iter().map(person).collect::<Vec<_>>().join(", ")
}

/// A file size as people say it: `512 B`, `72 KB`, `3.4 MB`.
fn size_text(bytes: u64) -> String {
  const KB: u64 = 1024;
  const MB: u64 = 1024 * 1024;
  if bytes < KB {
    format!("{bytes} B")
  } else if bytes < MB {
    format!("{} KB", bytes.div_ceil(KB))
  } else {
    let tenths = bytes.saturating_mul(10) / MB;
    format!("{}.{} MB", tenths / 10, tenths % 10)
  }
}

/// Threads as a numbered list, newest first, each with the call that reads it.
fn thread_list(threads: &[&MailThread]) -> String {
  let mut out = String::new();
  for (index, thread) in threads.iter().enumerate() {
    let senders: Vec<&str> = thread
      .senders
      .iter()
      .map(|sender| {
        if sender.name.is_empty() {
          sender.email.as_str()
        } else {
          sender.name.as_str()
        }
      })
      .collect();
    let senders = if senders.is_empty() {
      "Someone".to_string()
    } else {
      senders.join(", ")
    };
    let mut notes = vec![if thread.messages == 1 {
      "1 email".to_string()
    } else {
      format!("{} emails", thread.messages)
    }];
    for (flag, word) in [
      (thread.unread, "unread"),
      (thread.starred, "starred"),
      (thread.attachments, "has files"),
    ] {
      if flag {
        notes.push(word.into());
      }
    }
    let _ = writeln!(
      out,
      "{}. [{}] {senders}: {} ({})",
      index + 1,
      stamp(thread.time),
      clip(&thread.subject, 200),
      notes.join(", ")
    );
    if !thread.snippet.is_empty() {
      let _ = writeln!(out, "   {}", clip(&thread.snippet, 200));
    }
    let _ = writeln!(out, "   read: mail_read_thread thread=\"{}\"", thread.id);
  }
  out
}

/// A thread's emails as plain text, oldest first.
fn email_text(messages: &[MailMessage]) -> String {
  let count = messages.len();
  let plural = if count == 1 { "" } else { "s" };
  let mut out = format!("{count} email{plural}, oldest first.\n");
  for (index, message) in messages.iter().enumerate() {
    let _ = writeln!(out, "\n--- Email {} of {count} (id {}) ---", index + 1, message.id);
    let _ = writeln!(out, "From: {}", person(&message.from));
    if !message.to.is_empty() {
      let _ = writeln!(out, "To: {}", people(&message.to));
    }
    if !message.cc.is_empty() {
      let _ = writeln!(out, "Cc: {}", people(&message.cc));
    }
    let unread = if message.unread { " (unread)" } else { "" };
    let _ = writeln!(out, "Date: {}{unread}", stamp(message.time));
    if !message.attachments.is_empty() {
      let files: Vec<String> = message
        .attachments
        .iter()
        .map(|file| format!("{} ({})", file.name, size_text(file.size)))
        .collect();
      let _ = writeln!(out, "Attached: {}", files.join(", "));
    }
    out.push('\n');
    out.push_str(if message.text.is_empty() {
      "(no text)"
    } else {
      &message.text
    });
    out.push('\n');
  }
  out
}

/// The threads of a mailbox (or a search) as an answer.
async fn mail_list(
  engine: &Arc<Engine>,
  mailbox: Mailbox,
  limit: usize,
  empty: String,
  heading: &str,
) -> Result<String, String> {
  let page = engine.mail_threads(&mailbox, 0).await.map_err(|error| mail_offline(&error))?;
  let shown: Vec<&MailThread> = page.threads.iter().take(limit).collect();
  let mut out = format!("{}\n", now_line());
  if let Some(unread) = page.inbox_unread {
    let _ = writeln!(out, "Unread threads in the inbox: {unread}.");
  }
  if shown.is_empty() {
    out.push_str(&empty);
    return Ok(out);
  }
  let _ = writeln!(out, "{} {heading}, newest first:", shown.len());
  out.push_str(&thread_list(&shown));
  if page.more || page.threads.len() > shown.len() {
    out.push_str("Older threads exist: narrow it down with mail_search.\n");
  }
  Ok(out)
}

/// The label a model means by its name or part of one: an exact name (any case) wins, then one label whose name
/// holds the words. Otherwise the answer lists what matched.
fn find_label<'a>(labels: &'a [MailLabel], wanted: &str) -> Result<&'a MailLabel, String> {
  let typed = wanted.trim();
  let wanted = typed.to_lowercase();
  if let Some(exact) = labels.iter().find(|label| label.name.to_lowercase() == wanted) {
    return Ok(exact);
  }
  let matching: Vec<&MailLabel> = labels
    .iter()
    .filter(|label| label.name.to_lowercase().contains(&wanted))
    .collect();
  match matching.as_slice() {
    [one] => Ok(*one),
    [] => Err(format!("No label is named \"{typed}\". Call mail_labels to see the labels.")),
    many => {
      let names: Vec<&str> = many.iter().map(|label| label.name.as_str()).collect();
      Err(format!(
        "\"{typed}\" matches several labels: {}. Pass one of them.",
        names.join(", ")
      ))
    },
  }
}

async fn mail_inbox(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailInboxArgs = parse(arguments)?;
  let limit = args.limit.unwrap_or(20).clamp(1, 50);
  let (mailbox, query, name) = if let Some(wanted) = &args.label {
    let labels = engine.mail_labels().await.map_err(|error| mail_offline(&error))?;
    let label = find_label(&labels, wanted)?;
    (
      Mailbox::Label(label.id.clone()),
      format!("in:{}", label.id),
      format!("label {}", label.name),
    )
  } else {
    let (mailbox, query, name) = match args.mailbox.unwrap_or_default() {
      MailboxArg::Inbox => (Mailbox::Inbox, "in:inbox", "inbox"),
      MailboxArg::Starred => (Mailbox::Starred, "is:starred", "starred mail"),
      MailboxArg::Sent => (Mailbox::Sent, "in:sent", "sent mail"),
      MailboxArg::All => (Mailbox::All, "in:all", "all mail"),
    };
    (mailbox, query.to_string(), name.to_string())
  };
  let (mailbox, heading) = if args.unread_only {
    (
      Mailbox::Search(format!("{query} is:unread")),
      format!("unread threads of the {name}"),
    )
  } else {
    (mailbox, format!("threads of the {name}"))
  };
  let empty = format!("No {heading}.\n");
  mail_list(engine, mailbox, limit, empty, &heading).await
}

/// The user's labels as an answer: one per line, by name, with its unread threads.
fn label_lines(labels: &[MailLabel]) -> String {
  if labels.is_empty() {
    return "The user has no labels of their own.\n".into();
  }
  let mut out = format!("{} labels:\n", labels.len());
  for label in labels {
    let unread = match label.unread {
      0 => String::new(),
      count => format!(" ({count} unread)"),
    };
    let _ = writeln!(out, "- {}{unread}", label.name);
  }
  out.push_str("List one with mail_inbox label=\"<name>\".\n");
  out
}

async fn mail_labels(engine: &Arc<Engine>, arguments: &Value) -> Result<String, String> {
  no_arguments("mail_labels", arguments)?;
  let labels = engine.mail_labels().await.map_err(|error| mail_offline(&error))?;
  Ok(label_lines(&labels))
}

async fn mail_search(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailSearchArgs = parse(arguments)?;
  let query = args.query.trim();
  if query.is_empty() {
    return Err("`query` is empty. Pass Gmail search words, such as `from:jpark@example.com`.".into());
  }
  let limit = args.limit.unwrap_or(20).clamp(1, 50);
  let empty = format!(
    "No thread matches \"{query}\". The search works as Gmail's search box: try fewer words, or operators such as \
     from:, subject:, or after:2026/11/01.\n"
  );
  let heading = format!("threads match \"{query}\"");
  mail_list(engine, Mailbox::Search(query.to_string()), limit, empty, &heading).await
}

async fn mail_read_thread(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailReadArgs = parse(arguments)?;
  let thread = thread_arg(&args.thread)?;
  let messages = thread_messages(engine, &thread).await?;
  Ok(keep_end(email_text(&messages)))
}

/// A thread's emails read now, with an error that says what to pass instead.
async fn thread_messages(engine: &Arc<Engine>, thread: &str) -> Result<Vec<MailMessage>, String> {
  let messages = engine.mail_messages(thread).await.map_err(|error| {
    format!(
      "Thread {thread} could not be read: {error:#}. Pass a thread id exactly as mail_inbox or mail_search gave it."
    )
  })?;
  if messages.is_empty() {
    return Err(format!(
      "Thread {thread} has no emails. Pass a thread id that mail_inbox or mail_search gave."
    ));
  }
  Ok(messages)
}

/// The text of an email to send: what the agent wrote, which may not be empty.
fn send_text(text: &str) -> Result<String, String> {
  if text.trim().is_empty() {
    return Err("`text` is empty: write what the email says. Nothing was sent.".into());
  }
  Ok(text.to_string())
}

/// What a write tool answers when Gmail did not take it.
fn not_written(what: &str, error: &str) -> String {
  format!("{what} failed: {error}. Nothing was changed; try again in a moment.")
}

async fn mail_send(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailSendArgs = parse(arguments)?;
  let to: Vec<MailAddress> = args.to.iter().map(|text| address_of(text)).collect();
  let cc: Vec<MailAddress> = args.cc.iter().map(|text| address_of(text)).collect();
  crate::mail::check_recipients(&to, &cc).map_err(|error| {
    format!("{error} Give addresses such as jpark@example.com or Jordan Park <jpark@example.com>. Nothing was sent.")
  })?;
  let text = send_text(&args.text)?;
  let compose = MailCompose {
    to: to.clone(),
    cc: cc.clone(),
    subject: args.subject.trim().to_string(),
    text,
    signature: args.signature.unwrap_or(true),
  };
  let thread = engine
    .agent_compose(compose)
    .await
    .map_err(|error| not_written("Sending", &error))?;
  let mut out = format!("Sent to {}", people(&to));
  if !cc.is_empty() {
    let _ = write!(out, ", copy to {}", people(&cc));
  }
  let _ = write!(out, ".\nThread: {thread}\nread: mail_read_thread thread=\"{thread}\"\n");
  Ok(out)
}

/// The email a reply answers and who the reply goes to.
struct ReplyPlan<'a> {
  answered: &'a MailMessage,
  to: Vec<MailAddress>,
  cc: Vec<MailAddress>,
}

/// The email a reply answers and who it goes to: `wanted` by its id, else the thread's newest email from someone
/// else, else its newest. The recipients are Gmail's Reply (or Reply all) of that email, never anyone else.
fn reply_plan<'a>(
  messages: &'a [MailMessage],
  me: &str,
  wanted: Option<&str>,
  all: bool,
) -> Result<ReplyPlan<'a>, String> {
  let answered = match wanted {
    Some(id) => messages.iter().find(|message| message.id == id).ok_or_else(|| {
      format!(
        "Email {id} is not in this thread. Pass an email id that mail_read_thread gave, or leave `message` out to \
         answer the newest email from someone else. Nothing was sent."
      )
    })?,
    None => messages
      .iter()
      .rev()
      .find(|message| !message.from.email.eq_ignore_ascii_case(me))
      .or_else(|| messages.last())
      .ok_or("The thread has no emails to answer. Nothing was sent.")?,
  };
  let (to, cc) = crate::mail::reply_recipients(answered, me, all);
  if to.is_empty() && cc.is_empty() {
    return Err("The email answered names nobody to send a reply to. Nothing was sent.".into());
  }
  Ok(ReplyPlan { answered, to, cc })
}

async fn mail_reply(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailReplyArgs = parse(arguments).map_err(|error| {
    if error.contains("unknown field") {
      format!("{error} mail_reply takes no recipients: they come from the email answered (reply_all for everyone).")
    } else {
      error
    }
  })?;
  let thread = thread_arg(&args.thread)?;
  let text = send_text(&args.text)?;
  let me = engine
    .mail_me()
    .await
    .map_err(|error| format!("Gmail could not be reached: {error:#}. Try again in a moment."))?
    .ok_or("Gmail did not say which address sends mail. Nothing was sent.")?;
  let messages = thread_messages(engine, &thread).await?;
  let ReplyPlan { answered, to, cc } = reply_plan(&messages, &me.email, args.message.as_deref(), args.reply_all)?;
  let answered_line = format!("the email from {} of {}", person(&answered.from), stamp(answered.time));
  let reply = MailReply {
    thread: thread.clone(),
    message: answered.id.clone(),
    to: to.clone(),
    cc: cc.clone(),
    text,
    signature: args.signature.unwrap_or(true),
  };
  engine
    .agent_reply(reply)
    .await
    .map_err(|error| not_written("Replying", &error))?;
  let mut out = format!("Replied in thread {thread} to {}", people(&to));
  if !cc.is_empty() {
    let _ = write!(out, ", copy to {}", people(&cc));
  }
  let _ = writeln!(out, ", answering {answered_line}.");
  Ok(out)
}

async fn mail_archive(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailArchiveArgs = parse(arguments)?;
  let thread = thread_arg(&args.thread)?;
  let archive = args.archive.unwrap_or(true);
  engine.agent_archive(thread.clone(), archive).await.map_err(|error| {
    not_written(
      if archive {
        "Archiving"
      } else {
        "Moving to the inbox"
      },
      &error,
    )
  })?;
  Ok(if archive {
    format!("Archived thread {thread}: it left the inbox and stays in All Mail.\n")
  } else {
    format!("Moved thread {thread} back to the inbox.\n")
  })
}

async fn mail_mark(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailMarkArgs = parse(arguments)?;
  let thread = thread_arg(&args.thread)?;
  let read = args
    .read
    .ok_or("`read` is missing: true marks the thread read, false unread. Nothing was changed.")?;
  engine
    .agent_mark(thread.clone(), read)
    .await
    .map_err(|error| not_written("Marking", &error))?;
  Ok(format!("Marked thread {thread} {}.\n", if read { "read" } else { "unread" }))
}

#[cfg(test)]
mod tests {
  use super::{
    Gate, MAIL_OFF, MailInboxArgs, MailMarkArgs, MailReplyArgs, MailSendArgs, READ_ONLY_MODE, TOOLS, WRITE_OFF,
    WRITE_TOOLS, address_of, email_text, find_label, label_lines, list, refusal, reply_plan, send_text, size_text,
    thread_arg, thread_list,
  };
  use crate::mcp::shared::parse;
  use crate::model::{MailAddress, MailAttachment, MailLabel, MailMessage, MailThread};
  use serde_json::json;

  #[test]
  fn mail_tools_are_listed_read_only() {
    let tools = list(false);
    assert_eq!(tools.len(), TOOLS.len());
    for name in TOOLS {
      let tool = tools
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| panic!("{name} listed"));
      assert_eq!(tool["annotations"]["readOnlyHint"], true);
      assert!(tool["description"].as_str().unwrap().contains("only while Mail is on"));
    }
    let args: MailInboxArgs = parse(json!({ "mailbox": "sent", "limit": "5", "unread_only": "true" })).unwrap();
    assert_eq!(args.limit, Some(5));
    assert!(args.unread_only);
  }

  #[test]
  fn write_tools_are_listed_only_when_allowed() {
    let tools = list(true);
    assert_eq!(tools.len(), TOOLS.len() + WRITE_TOOLS.len());
    for name in WRITE_TOOLS {
      let tool = tools
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| panic!("{name} listed"));
      assert_eq!(tool["annotations"]["readOnlyHint"], false);
      let description = tool["description"].as_str().unwrap();
      assert!(description.contains("only while Mail is on"));
      assert!(description.contains("real"), "{name} says the write is real");
    }
    assert!(
      !list(false)
        .iter()
        .any(|tool| WRITE_TOOLS.contains(&tool["name"].as_str().unwrap()))
    );
    let reply = tools.iter().find(|tool| tool["name"] == "mail_reply").unwrap();
    assert!(
      reply["inputSchema"]["properties"].get("to").is_none(),
      "no recipients from the agent"
    );
  }

  #[test]
  fn writes_are_refused_when_not_allowed_read_only_or_mail_off() {
    // A write: the switch first, then read-only mode, then Mail.
    let gate = |writable, readonly, mail_on| Gate { writable, readonly, mail_on };
    assert_eq!(refusal(true, gate(false, false, true)), Some(WRITE_OFF));
    assert_eq!(refusal(true, gate(false, true, false)), Some(WRITE_OFF));
    assert_eq!(refusal(true, gate(true, true, true)), Some(READ_ONLY_MODE));
    assert_eq!(refusal(true, gate(true, false, false)), Some(MAIL_OFF));
    assert_eq!(refusal(true, gate(true, false, true)), None);
    // A read: only Mail matters.
    assert_eq!(refusal(false, gate(false, true, true)), None);
    assert_eq!(refusal(false, gate(true, false, false)), Some(MAIL_OFF));
  }

  #[test]
  fn write_arguments_are_checked() {
    let args: MailSendArgs = parse(json!({
      "to": "Jordan Park <jpark@example.com>, rbrooks@example.com",
      "cc": ["\"Riley Brooks\" <rbrooks@example.com>"],
      "text": "Noon?",
      "signature": "false"
    }))
    .unwrap();
    assert_eq!(args.to.len(), 2);
    assert_eq!(args.signature, Some(false));
    assert_eq!(address_of(&args.to[0]), jordan());
    assert_eq!(address_of(&args.cc[0]).name, "Riley Brooks");
    assert_eq!(address_of(" rbrooks@example.com ").email, "rbrooks@example.com");
    assert!(send_text("  \n").unwrap_err().contains("Nothing was sent"));
    assert_eq!(send_text("Noon?").unwrap(), "Noon?");
    let none: MailSendArgs = parse(json!({ "text": "Noon?" })).unwrap();
    assert!(none.to.is_empty() && none.signature.is_none());
    assert_eq!(thread_arg(" #1794896454636 ").unwrap(), "thread-f:1794896454636");
    assert_eq!(thread_arg("thread-a:r-12").unwrap(), "thread-a:r-12");
    assert!(thread_arg(" ").is_err());
    let mark: MailMarkArgs = parse(json!({ "thread": "thread-f:1", "read": "false" })).unwrap();
    assert_eq!(mark.read, Some(false));
    // Recipients are not the agent's to give.
    let given = parse::<MailReplyArgs>(json!({ "thread": "thread-f:1", "text": "Hi", "to": ["jpark@example.com"] }));
    assert!(given.is_err());
  }

  fn email(id: &str, from: MailAddress, to: Vec<MailAddress>, cc: Vec<MailAddress>) -> MailMessage {
    MailMessage {
      id: id.into(),
      from,
      to,
      cc,
      reply_to: Vec::new(),
      time: 1_794_896_454_636_000,
      html: String::new(),
      text: String::new(),
      unread: false,
      attachments: Vec::new(),
    }
  }

  fn avery() -> MailAddress {
    MailAddress {
      name: "Avery Morgan".into(),
      email: "amorgan@example.com".into(),
    }
  }

  #[test]
  fn a_reply_answers_the_newest_email_from_someone_else() {
    let me = "amorgan@example.com";
    let thread = vec![
      email("msg-f:1", jordan(), vec![avery()], vec![riley()]),
      email("msg-f:2", riley(), vec![avery()], vec![jordan()]),
      email("msg-f:3", avery(), vec![riley()], Vec::new()),
    ];
    let plan = reply_plan(&thread, me, None, false).unwrap();
    assert_eq!(plan.answered.id, "msg-f:2", "the user's own newest email is passed over");
    assert_eq!(plan.to, vec![riley()]);
    assert_eq!(plan.cc, Vec::<MailAddress>::new());
    let plan = reply_plan(&thread, me, None, true).unwrap();
    assert_eq!(plan.to, vec![riley()]);
    assert_eq!(plan.cc, vec![jordan()]);
    let plan = reply_plan(&thread, me, Some("msg-f:1"), false).unwrap();
    assert_eq!((plan.answered.id.as_str(), plan.to), ("msg-f:1", vec![jordan()]));
    assert!(
      reply_plan(&thread, me, Some("msg-f:9"), false)
        .err()
        .is_some_and(|error| error.contains("not in this thread"))
    );
    // A thread of the user's own: its newest, to its recipients.
    let own = vec![email("msg-f:4", avery(), vec![jordan()], Vec::new())];
    let plan = reply_plan(&own, me, None, false).unwrap();
    assert_eq!((plan.answered.id.as_str(), plan.to), ("msg-f:4", vec![jordan()]));
  }

  fn jordan() -> MailAddress {
    MailAddress {
      name: "Jordan Park".into(),
      email: "jpark@example.com".into(),
    }
  }

  fn riley() -> MailAddress {
    MailAddress {
      name: String::new(),
      email: "rbrooks@example.com".into(),
    }
  }

  #[test]
  fn mail_threads_list_flags_and_the_read_call() {
    let thread = MailThread {
      id: "thread-f:42".into(),
      subject: "Lunch on Friday".into(),
      snippet: "Want to grab lunch?".into(),
      senders: vec![jordan(), riley()],
      time: 1_794_896_454_636_000,
      messages: 3,
      unread: true,
      starred: false,
      attachments: true,
    };
    let text = thread_list(&[&thread]);
    assert!(text.contains("] Jordan Park, rbrooks@example.com: Lunch on Friday (3 emails, unread, has files)"));
    assert!(text.contains("\n   Want to grab lunch?\n"));
    assert!(text.ends_with("   read: mail_read_thread thread=\"thread-f:42\"\n"));
  }

  #[test]
  fn mail_thread_reads_as_text() {
    let message = MailMessage {
      id: "msg-f:1".into(),
      from: jordan(),
      to: vec![riley()],
      cc: Vec::new(),
      reply_to: Vec::new(),
      time: 1_794_896_454_636_000,
      html: "<div>Noon?</div>".into(),
      text: "Noon?".into(),
      unread: true,
      attachments: vec![MailAttachment {
        name: "menu.pdf".into(),
        mime: "application/pdf".into(),
        size: 73_283,
        url: "https://mail.google.com/mail/u/0/?attid=0.1".into(),
      }],
    };
    let text = email_text(&[message]);
    assert!(text.starts_with(
      "1 email, oldest first.\n\n--- Email 1 of 1 (id msg-f:1) ---\nFrom: Jordan Park <jpark@example.com>\nTo: rbrooks@example.com\nDate: "
    ));
    assert!(text.contains(" (unread)\nAttached: menu.pdf (72 KB)\n\nNoon?\n"));
    assert!(!text.contains("Cc:") && !text.contains("<div>"));
  }

  #[test]
  fn file_sizes_read_naturally() {
    assert_eq!(size_text(512), "512 B");
    assert_eq!(size_text(73_283), "72 KB");
    assert_eq!(size_text(3_670_016), "3.5 MB");
  }

  fn labels() -> Vec<MailLabel> {
    let label = |id: &str, name: &str, unread| MailLabel {
      id: id.into(),
      name: name.into(),
      color: None,
      unread,
    };
    vec![
      label("^x_1", "Clients", 0),
      label("^x_2", "Clients/Acme", 3),
      label("^x_3", "Receipts", 0),
    ]
  }

  #[test]
  fn labels_are_found_by_name_or_a_unique_part() {
    let labels = labels();
    assert_eq!(
      find_label(&labels, "clients").unwrap().id,
      "^x_1",
      "an exact name wins over a longer match"
    );
    assert_eq!(find_label(&labels, " acme ").unwrap().id, "^x_2");
    assert_eq!(find_label(&labels, "RECEIPTS").unwrap().id, "^x_3");
    assert!(find_label(&labels, "Travel").unwrap_err().contains("mail_labels"));
    assert!(find_label(&labels, "c").unwrap_err().contains("Clients/Acme"));
    let args: MailInboxArgs = parse(json!({ "label": "Clients/Acme" })).unwrap();
    assert_eq!(args.label.as_deref(), Some("Clients/Acme"));
  }

  #[test]
  fn labels_list_with_unread_counts() {
    assert_eq!(
      label_lines(&labels()),
      "3 labels:\n- Clients\n- Clients/Acme (3 unread)\n- Receipts\nList one with mail_inbox label=\"<name>\".\n"
    );
    assert_eq!(label_lines(&[]), "The user has no labels of their own.\n");
  }
}
