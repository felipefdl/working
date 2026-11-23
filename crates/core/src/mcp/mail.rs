//! The mail tools: the signed-in account's Gmail. Reading is always offered; the tools that send, reply, draft,
//! archive, mark, star, and label are listed and run only when the user let agents write mail
//! (`McpConfig::mail_write`), and never under `WORKING_READONLY=1`. Every tool answers only while Mail is on in the
//! app (`Engine::mail_on`); otherwise it answers that Mail is off, without contacting Gmail.
#![allow(clippy::too_many_lines, reason = "tool schemas are verbose JSON")]

use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use super::shared::{
  MAX_ANSWER, answer, clip, keep_end, keep_start, loose_count, loose_flag, loose_switch, loose_text, no_arguments,
  now_line, parse, read_only, stamp, writes,
};
use crate::engine::Engine;
use crate::mail::ThreadPage;
use crate::model::{
  MailAddress, MailAttachment, MailCompose, MailDraft, MailDraftSent, MailForward, MailLabel, MailMessage, MailReply,
  MailThread, Mailbox,
};

/// The mail part of the server's instructions: what every mail tool shares, said once.
pub(super) const INSTRUCTIONS: &str = "Gmail: the mail tools answer only while Mail is on in Working. mail_inbox \
lists a mailbox or one of the user's labels and mail_search finds threads with Gmail's own search words, both 50 \
threads a page; mail_read_thread reads a thread's emails, mail_attachment opens a file of one, and mail_labels names \
the labels. Reading changes nothing: threads stay unread. When the user allowed it, the other mail tools write: \
mail_send sends a new email, mail_reply answers in a thread, mail_forward sends an email of a thread on to others \
(without its files), mail_draft saves an email in Gmail's Drafts and mail_send_draft sends it, and mail_archive, \
mail_mark, mail_star, and mail_label change threads. Writes are real and show in Gmail at once, and a sent email \
reaches its recipients at once: send only what the user asked for.";

/// The tools that read, each answering only while Mail is on (`Engine::mail_on`).
const TOOLS: [&str; 5] = [
  "mail_inbox",
  "mail_search",
  "mail_read_thread",
  "mail_labels",
  "mail_attachment",
];

/// The tools that change mail: listed and run only with `mail_write`.
const WRITE_TOOLS: [&str; 9] = [
  "mail_send",
  "mail_reply",
  "mail_forward",
  "mail_draft",
  "mail_send_draft",
  "mail_archive",
  "mail_mark",
  "mail_star",
  "mail_label",
];

/// `call` runs a tool of this name, a write tool too (it refuses while writing mail is off).
pub(super) fn owns(name: &str) -> bool {
  TOOLS.contains(&name) || WRITE_TOOLS.contains(&name)
}

/// What a tool answers while Mail is off. Nothing contacts Gmail then.
const MAIL_OFF: &str = "Mail is turned off in Working.";
/// What a write tool answers while the user has not let agents write mail.
const WRITE_OFF: &str = "Writing mail is turned off. The user can allow it in Working: Settings, Agents.";
/// What a write tool answers under `WORKING_READONLY=1`.
const READ_ONLY_MODE: &str = "Working runs in read-only mode: nothing was changed.";

/// Threads in one page of Gmail's list.
const PAGE_SIZE: usize = 50;
/// The largest file `mail_attachment` downloads, in bytes.
const MAX_FILE: u64 = 25 * 1024 * 1024;

/// Tool descriptors; the ones that change mail only when `writable`.
pub(super) fn list(writable: bool) -> Vec<Value> {
  let read_only = read_only();
  let page = json!({ "type": "integer", "description": "Which page of 50 threads, 1 the newest. Default 1. Gmail's later pages can repeat a few threads of the page before." });
  let limit = json!({ "type": "integer", "description": "Most threads of the page to list. Default 20, at most 50." });
  let mut tools = vec![
    json!({
      "name": "mail_inbox",
      "title": "Gmail threads",
      "description": "List the newest email threads of the user's Gmail: the inbox by default, the starred, sent, \
    or all mail, or one of the user's labels. Each thread says whether it is unread, who wrote it, when, its subject, \
    and the start of its newest email, with the mail_read_thread call that reads it. When older threads exist the \
    answer ends with the call that reads the next page. To find particular emails, call mail_search instead.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "mailbox": { "type": "string", "enum": ["inbox", "starred", "sent", "all"], "description": "Which threads. Default inbox. Leave out with `label`." },
          "label": { "type": "string", "description": "List one of the user's labels instead, by name as mail_labels gives it (\"Clients/Acme\"); part of a name works when it points at one label." },
          "limit": limit,
          "unread_only": { "type": "boolean", "description": "Only unread threads. Default false." },
          "page": page
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
    call that reads it, and the call for the next page when there are more.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "query": { "type": "string", "description": "Gmail search, such as `from:jpark@example.com has:attachment`." },
          "limit": limit,
          "page": page
        },
        "required": ["query"],
        "additionalProperties": false
      },
      "annotations": read_only,
    }),
    json!({
      "name": "mail_read_thread",
      "title": "Read an email thread",
      "description": "Read the emails of one Gmail thread, oldest first, as plain text: each one's id, who wrote it, \
    to and copied whom, when, its files numbered (mail_attachment opens one), and its text. Each email's text leaves \
    out the earlier emails it quotes unless `quotes` is true. For a long thread, `last` reads only the newest emails \
    and `message` one email by its id. A draft not sent yet is marked as one.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "thread": { "type": "string", "description": "The thread id that mail_inbox or mail_search gave, such as thread-f:1794896454636." },
          "message": { "type": "string", "description": "Read only this email: its id from an earlier mail_read_thread." },
          "last": { "type": "integer", "description": "Read only the newest this many emails." },
          "quotes": { "type": "boolean", "description": "Keep the earlier emails each one quotes. Default false." }
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
    its threads are unread. To list a label's threads, call mail_inbox with its `label`.",
      "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
      "annotations": read_only,
    }),
    json!({
      "name": "mail_attachment",
      "title": "Open an email's file",
      "description": "Download one file of an email and save it on this computer, in Working's own folder, which \
    Working empties when it starts. Answers the file's name, type, size, and where it was saved; a text file (plain \
    text, CSV, JSON, XML, HTML) also as its text. Files larger than 25 MB are refused.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "thread": { "type": "string", "description": "The thread id, as mail_read_thread was called with." },
          "message": { "type": "string", "description": "The email's id, as mail_read_thread gave it." },
          "file": { "anyOf": [{ "type": "string" }, { "type": "integer" }], "description": "The file's name as mail_read_thread lists it, or its number in that email (1 the first)." }
        },
        "required": ["thread", "message", "file"],
        "additionalProperties": false
      },
      "annotations": read_only,
    }),
  ];
  if !writable {
    return tools;
  }
  let thread =
    json!({ "type": "string", "description": "The thread id that mail_inbox, mail_search, or mail_send gave." });
  let threads = json!({
    "anyOf": [{ "type": "string" }, { "type": "array", "items": { "type": "string" } }],
    "description": "A thread id that mail_inbox, mail_search, or mail_send gave, or a list of them."
  });
  let names = json!({
    "anyOf": [{ "type": "string" }, { "type": "array", "items": { "type": "string" } }],
    "description": "A label's name as mail_labels gives it, or a list of them."
  });
  let addresses = |what: &str| json!({ "type": "array", "items": { "type": "string" }, "description": what });
  let signature = json!({ "type": "boolean", "description": "Add the signature the user set in Gmail, as Gmail does. Default true." });
  let text = json!({ "type": "string", "description": "The email, as plain text: a blank line starts a paragraph." });
  let reply_all =
    json!({ "type": "boolean", "description": "Answer everyone on the email, as Reply all. Default false." });
  let message = json!({ "type": "string", "description": "The email to answer: its id from mail_read_thread. Default the newest email from someone else." });
  tools.extend([
    json!({
      "name": "mail_send",
      "title": "Send an email",
      "description": "Send a new email as the user, from their Gmail address. Give at least one address in `to`, \
`cc`, or `bcc`, such as jpark@example.com or Jordan Park <jpark@example.com>. Answers the new thread's id. For the \
user to look it over first, call mail_draft instead.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "to": addresses("Recipients' addresses."),
          "cc": addresses("Who gets a copy."),
          "bcc": addresses("Who gets a hidden copy: the other recipients do not see them."),
          "subject": { "type": "string" },
          "text": text,
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
      "description": "Answer an email in its thread as the user, quoting it as Gmail does. It answers the thread's \
newest email from someone else unless `message` names one, and goes where Gmail's Reply (or Reply all, with \
reply_all) sends it: the recipients come from the email answered, never from the call. Answers who it went to.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "thread": thread,
          "text": text,
          "reply_all": reply_all,
          "message": message,
          "signature": signature
        },
        "required": ["thread", "text"],
        "additionalProperties": false
      },
      "annotations": writes(false, false),
    }),
    json!({
      "name": "mail_forward",
      "title": "Forward an email",
      "description": "Forward an email of a thread as the user: it is sent on, at once, as a new email that quotes \
it, on a new thread of its own. The email's attached files are NOT sent with it, and pictures inside the email may \
not show for the recipients. It forwards the thread's newest email that is not a draft unless `message` names one. \
Give at least one address in `to`, `cc`, or `bcc`; `text` is what to say above the forwarded email and may be empty. \
Answers who it went to, how many files were left out, and the new thread's id.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "thread": thread,
          "message": { "type": "string", "description": "The email to forward: its id from mail_read_thread. Default the thread's newest email that is not a draft." },
          "to": addresses("Recipients' addresses."),
          "cc": addresses("Who gets a copy."),
          "bcc": addresses("Who gets a hidden copy: the other recipients do not see them."),
          "text": { "type": "string", "description": "What to say above the forwarded email, as plain text. May be empty." },
          "signature": signature
        },
        "required": ["thread", "to"],
        "additionalProperties": false
      },
      "annotations": writes(false, false),
    }),
    json!({
      "name": "mail_draft",
      "title": "Save an email as a draft",
      "description": "Save an email in the user's Gmail Drafts without sending it, where the user can review it: a \
new email (`to`, `cc`, `bcc`, `subject`, `text`, as mail_send) or, with `thread`, a reply (`text`, `reply_all`, \
`message`, as mail_reply: the recipients come from the email answered). Each call makes a new draft, and an earlier \
one stays in Gmail's Drafts until the user discards it there: get the wording right before saving. Answers the \
draft's id, who it \
would go to, and the mail_send_draft call that sends it. Working holds a draft for mail_send_draft only until Working \
restarts; after that the user sends it from Gmail's Drafts.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "to": addresses("A new email's recipients."),
          "cc": addresses("Who gets a copy of a new email."),
          "bcc": addresses("Who gets a hidden copy of a new email."),
          "subject": { "type": "string", "description": "A new email's subject." },
          "text": text,
          "thread": { "type": "string", "description": "Make a reply in this thread, its id as mail_inbox or mail_search gave it." },
          "reply_all": reply_all,
          "message": message,
          "signature": signature
        },
        "required": ["text"],
        "additionalProperties": false
      },
      "annotations": writes(false, false),
    }),
    json!({
      "name": "mail_send_draft",
      "title": "Send a draft",
      "description": "Send a draft that mail_draft saved, exactly as saved; it leaves Gmail's Drafts. Answers who it \
went to and its thread. A draft saved before Working restarted is not held: the user sends it from Gmail's Drafts.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "draft": { "type": "string", "description": "The draft's id that mail_draft gave." }
        },
        "required": ["draft"],
        "additionalProperties": false
      },
      "annotations": writes(false, false),
    }),
    json!({
      "name": "mail_archive",
      "title": "Archive threads",
      "description": "Take threads out of the user's inbox, as Gmail's Archive; they stay in All Mail. With archive \
false, move them back to the inbox. Each thread is changed in turn; the answer lists the ones done and each one that \
failed, with why.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "thread": threads,
          "archive": { "type": "boolean", "description": "True archives, false moves back to the inbox. Default true." }
        },
        "required": ["thread"],
        "additionalProperties": false
      },
      "annotations": writes(false, true),
    }),
    json!({
      "name": "mail_mark",
      "title": "Mark threads read or unread",
      "description": "Mark threads read or unread, as Gmail's Mark as read and Mark as unread. Reading a thread with \
the other tools leaves it unread; this is the way to change that. Each thread is changed in turn; the answer lists \
the ones done and each one that failed, with why.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "thread": threads,
          "read": { "type": "boolean", "description": "True marks read, false unread." }
        },
        "required": ["thread", "read"],
        "additionalProperties": false
      },
      "annotations": writes(false, true),
    }),
    json!({
      "name": "mail_star",
      "title": "Star threads",
      "description": "Star threads as Gmail's star does (on the newest email), or with starred false take the star \
off every email of them. Each thread is changed in turn; the answer lists the ones done and each one that failed, \
with why.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "thread": threads,
          "starred": { "type": "boolean", "description": "True stars, false takes the star off. Default true." }
        },
        "required": ["thread"],
        "additionalProperties": false
      },
      "annotations": writes(false, true),
    }),
    json!({
      "name": "mail_label",
      "title": "Label threads",
      "description": "Put the user's own labels on threads and take others off, as Gmail's Labels menu, on every \
email of each thread. Name labels as mail_labels gives them; part of a name works when it points at one label. Give \
`add`, `remove`, or both. Making a new label is not offered: the user makes labels in Gmail. Each thread is changed \
in turn; the answer lists the ones done and each one that failed, with why.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "thread": threads,
          "add": names,
          "remove": names
        },
        "required": ["thread"],
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
    "mail_attachment" => mail_attachment(engine, arguments).await,
    "mail_send" => mail_send(engine, arguments).await,
    "mail_reply" => mail_reply(engine, arguments).await,
    "mail_forward" => mail_forward(engine, arguments).await,
    "mail_draft" => mail_draft(engine, arguments).await,
    "mail_send_draft" => mail_send_draft(engine, arguments).await,
    "mail_archive" => mail_archive(engine, arguments).await,
    "mail_mark" => mail_mark(engine, arguments).await,
    "mail_star" => mail_star(engine, arguments).await,
    "mail_label" => mail_label(engine, arguments).await,
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
  #[serde(default, deserialize_with = "loose_count")]
  page: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailSearchArgs {
  query: String,
  #[serde(default, deserialize_with = "loose_count")]
  limit: Option<usize>,
  #[serde(default, deserialize_with = "loose_count")]
  page: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailReadArgs {
  thread: String,
  #[serde(default, deserialize_with = "loose_text")]
  message: Option<String>,
  #[serde(default, deserialize_with = "loose_count")]
  last: Option<usize>,
  #[serde(default, deserialize_with = "loose_flag")]
  quotes: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailAttachmentArgs {
  thread: String,
  message: String,
  /// A name, or a number (a model may send it as one).
  #[serde(default, deserialize_with = "loose_text")]
  file: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailSendArgs {
  #[serde(default, deserialize_with = "loose_addresses")]
  to: Vec<String>,
  #[serde(default, deserialize_with = "loose_addresses")]
  cc: Vec<String>,
  #[serde(default, deserialize_with = "loose_addresses")]
  bcc: Vec<String>,
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

/// `mail_forward`'s arguments.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailForwardArgs {
  thread: String,
  #[serde(default, deserialize_with = "loose_text")]
  message: Option<String>,
  #[serde(default, deserialize_with = "loose_addresses")]
  to: Vec<String>,
  #[serde(default, deserialize_with = "loose_addresses")]
  cc: Vec<String>,
  #[serde(default, deserialize_with = "loose_addresses")]
  bcc: Vec<String>,
  #[serde(default)]
  text: String,
  #[serde(default, deserialize_with = "loose_switch")]
  signature: Option<bool>,
}

/// `mail_draft`'s arguments: a new email's (`to`, `cc`, `bcc`, `subject`) or, with `thread`, a reply's (`reply_all`,
/// `message`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailDraftArgs {
  #[serde(default, deserialize_with = "loose_addresses")]
  to: Vec<String>,
  #[serde(default, deserialize_with = "loose_addresses")]
  cc: Vec<String>,
  #[serde(default, deserialize_with = "loose_addresses")]
  bcc: Vec<String>,
  #[serde(default, deserialize_with = "loose_text")]
  subject: Option<String>,
  #[serde(default)]
  text: String,
  #[serde(default, deserialize_with = "loose_text")]
  thread: Option<String>,
  #[serde(default, deserialize_with = "loose_switch")]
  reply_all: Option<bool>,
  #[serde(default, deserialize_with = "loose_text")]
  message: Option<String>,
  #[serde(default, deserialize_with = "loose_switch")]
  signature: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailSendDraftArgs {
  draft: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailArchiveArgs {
  #[serde(deserialize_with = "loose_threads")]
  thread: Vec<String>,
  #[serde(default, deserialize_with = "loose_switch")]
  archive: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailMarkArgs {
  #[serde(deserialize_with = "loose_threads")]
  thread: Vec<String>,
  #[serde(default, deserialize_with = "loose_switch")]
  read: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailStarArgs {
  #[serde(deserialize_with = "loose_threads")]
  thread: Vec<String>,
  #[serde(default, deserialize_with = "loose_switch")]
  starred: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailLabelArgs {
  #[serde(deserialize_with = "loose_threads")]
  thread: Vec<String>,
  #[serde(default, deserialize_with = "loose_names")]
  add: Vec<String>,
  #[serde(default, deserialize_with = "loose_names")]
  remove: Vec<String>,
}

/// One text or a list of them, as given.
fn loose_list<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
  #[derive(Deserialize)]
  #[serde(untagged)]
  enum Loose {
    One(String),
    Many(Vec<String>),
  }
  Ok(match Option::<Loose>::deserialize(deserializer)? {
    None => Vec::new(),
    Some(Loose::One(text)) => vec![text],
    Some(Loose::Many(list)) => list,
  })
}

/// The texts of `list` split at any of `between`, trimmed, the empty ones left out.
fn split_all(list: &[String], between: impl Fn(char) -> bool + Copy) -> Vec<String> {
  list
    .iter()
    .flat_map(|text| text.split(between))
    .map(str::trim)
    .filter(|text| !text.is_empty())
    .map(str::to_string)
    .collect()
}

/// Addresses as a list, or as one text with commas or semicolons between them.
fn loose_addresses<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
  Ok(split_all(&loose_list(deserializer)?, |found| matches!(found, ',' | ';')))
}

/// Thread ids as a list, or as one text with commas or spaces between them.
fn loose_threads<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
  Ok(split_all(&loose_list(deserializer)?, |found| {
    found == ',' || found == ';' || found.is_whitespace()
  }))
}

/// Label names as a list, or one name (names can hold commas, so one text is one name).
fn loose_names<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
  Ok(split_all(&loose_list(deserializer)?, |found| found == '\n'))
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

/// The threads a write takes, each as `thread_arg` reads it, each once, in the order given.
fn thread_args(raw: &[String]) -> Result<Vec<String>, String> {
  let mut threads: Vec<String> = Vec::with_capacity(raw.len());
  for text in raw {
    let thread = thread_arg(text)?;
    if !threads.contains(&thread) {
      threads.push(thread);
    }
  }
  if threads.is_empty() {
    return Err(
      "`thread` is empty. Pass a thread id that mail_inbox or mail_search gave, or a list of them. Nothing was \
       changed."
        .into(),
    );
  }
  Ok(threads)
}

/// The page a list tool reads: 1 the newest.
fn page_arg(page: Option<usize>) -> Result<u32, String> {
  u32::try_from(page.unwrap_or(1).max(1)).map_err(|_| "`page` is too large.".to_string())
}

// ---- Words ----

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

/// Who an email goes to, as one line: `Jordan Park <jpark@example.com>, copy to rbrooks@example.com, hidden copy to
/// amorgan@example.com`.
fn recipients(to: &[MailAddress], cc: &[MailAddress], bcc: &[MailAddress]) -> String {
  let mut parts = Vec::with_capacity(3);
  if !to.is_empty() {
    parts.push(people(to));
  }
  if !cc.is_empty() {
    parts.push(format!("copy to {}", people(cc)));
  }
  if !bcc.is_empty() {
    parts.push(format!("hidden copy to {}", people(bcc)));
  }
  parts.join(", ")
}

/// `1 thread`, `3 threads`.
fn threads_word(count: usize) -> String {
  if count == 1 {
    "1 thread".into()
  } else {
    format!("{count} threads")
  }
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

// ---- Lists ----

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

/// One page of a list as an answer: the threads shown, then what else there is and the call that reads it. `call`
/// is the call that read this page, without its `page`.
fn page_answer(page: &ThreadPage, number: u32, limit: usize, empty: &str, heading: &str, call: &str) -> String {
  let shown: Vec<&MailThread> = page.threads.iter().take(limit).collect();
  let mut out = String::new();
  if let Some(unread) = page.inbox_unread {
    let _ = writeln!(out, "Unread threads in the inbox: {unread}.");
  }
  if shown.is_empty() {
    if number > 1 {
      let _ = writeln!(out, "Page {number} has no threads: there are fewer pages than that.");
    } else {
      out.push_str(empty);
    }
    return out;
  }
  let at = if number > 1 {
    format!(" (page {number})")
  } else {
    String::new()
  };
  let _ = writeln!(out, "{} {heading}{at}, newest first:", shown.len());
  out.push_str(&thread_list(&shown));
  let left = page.threads.len() - shown.len();
  if left > 0 {
    let before = if page.more {
      ", before reading the next page"
    } else {
      ""
    };
    let _ = writeln!(
      out,
      "{} more on this page: call again with limit={PAGE_SIZE} to list them{before}.",
      threads_word(left)
    );
  }
  if page.more {
    let _ = writeln!(out, "Older threads exist. Next page: {call} page={}", number.saturating_add(1));
  }
  out
}

/// The threads of a mailbox (or a search) as an answer.
async fn mail_list(engine: &Arc<Engine>, mailbox: Mailbox, number: u32, list: ListWords<'_>) -> Result<String, String> {
  let page = engine
    .mail_threads(&mailbox, number - 1)
    .await
    .map_err(|error| mail_offline(&error))?;
  let body = page_answer(&page, number, list.limit, list.empty, list.heading, list.call);
  Ok(format!("{}\n{body}", now_line()))
}

/// What a list answer says around its threads.
#[derive(Clone, Copy)]
struct ListWords<'a> {
  limit: usize,
  empty: &'a str,
  heading: &'a str,
  call: &'a str,
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
  let limit = args.limit.unwrap_or(20).clamp(1, PAGE_SIZE);
  let number = page_arg(args.page)?;
  let mut call = String::from("mail_inbox");
  let (mailbox, query, name) = if let Some(wanted) = &args.label {
    let labels = engine.mail_labels().await.map_err(|error| mail_offline(&error))?;
    let label = find_label(&labels, wanted)?;
    let _ = write!(call, " label={}", json!(label.name));
    (
      Mailbox::Label(label.id.clone()),
      format!("in:{}", label.id),
      format!("label {}", label.name),
    )
  } else {
    let (mailbox, query, name, word) = match args.mailbox.unwrap_or_default() {
      MailboxArg::Inbox => (Mailbox::Inbox, "in:inbox", "inbox", "inbox"),
      MailboxArg::Starred => (Mailbox::Starred, "is:starred", "starred mail", "starred"),
      MailboxArg::Sent => (Mailbox::Sent, "in:sent", "sent mail", "sent"),
      MailboxArg::All => (Mailbox::All, "in:all", "all mail", "all"),
    };
    if args.mailbox.is_some() {
      let _ = write!(call, " mailbox=\"{word}\"");
    }
    (mailbox, query.to_string(), name.to_string())
  };
  if args.unread_only {
    call.push_str(" unread_only=true");
  }
  if args.limit.is_some() {
    let _ = write!(call, " limit={limit}");
  }
  let (mailbox, heading) = if args.unread_only {
    (
      Mailbox::Search(format!("{query} is:unread")),
      format!("unread threads of the {name}"),
    )
  } else {
    (mailbox, format!("threads of the {name}"))
  };
  let empty = format!("No {heading}.\n");
  let words = ListWords {
    limit,
    empty: &empty,
    heading: &heading,
    call: &call,
  };
  mail_list(engine, mailbox, number, words).await
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
  let limit = args.limit.unwrap_or(20).clamp(1, PAGE_SIZE);
  let number = page_arg(args.page)?;
  let mut call = format!("mail_search query={}", json!(query));
  if args.limit.is_some() {
    let _ = write!(call, " limit={limit}");
  }
  let empty = format!(
    "No thread matches \"{query}\". The search works as Gmail's search box: try fewer words, or operators such as \
     from:, subject:, or after:2026/11/01.\n"
  );
  let heading = format!("threads match \"{query}\"");
  let words = ListWords {
    limit,
    empty: &empty,
    heading: &heading,
    call: &call,
  };
  mail_list(engine, Mailbox::Search(query.to_string()), number, words).await
}

// ---- Reading a thread ----

/// Which emails of a thread to read, and whether with what they quote.
#[derive(Default)]
struct Reading<'a> {
  /// Only this email, by id.
  message: Option<&'a str>,
  /// Only the newest this many.
  last: Option<usize>,
  /// Keep the earlier emails each one quotes.
  quotes: bool,
}

/// What `mail_read_thread` answers: the emails `reading` asks for, oldest first, as plain text.
fn thread_text(thread: &str, messages: &[MailMessage], reading: &Reading<'_>) -> Result<String, String> {
  let count = messages.len();
  let plural = if count == 1 { "" } else { "s" };
  let (start, end, mut out) = match (reading.message, reading.last) {
    (Some(_), Some(_)) => return Err("Pass `message` or `last`, not both.".into()),
    (Some(id), None) => {
      let index = messages.iter().position(|message| message.id == id).ok_or_else(|| {
        let ids: Vec<&str> = messages.iter().map(|message| message.id.as_str()).collect();
        format!(
          "Email {id} is not in thread {thread}. Its emails, oldest first: {}.",
          ids.join(", ")
        )
      })?;
      (index, index + 1, String::new())
    },
    (None, Some(last)) if last < count => {
      let start = count - last.max(1);
      let older: Vec<&str> = messages.iter().take(start).map(|message| message.id.as_str()).collect();
      let out = format!(
        "The newest {} of {count} emails, oldest first. {} older left out: mail_read_thread thread=\"{thread}\" \
         last={count} reads them all, or message=\"<id>\" one of them ({}).\n",
        count - start,
        start,
        older.join(", ")
      );
      (start, count, out)
    },
    (None, _) => (0, count, format!("{count} email{plural}, oldest first.\n")),
  };
  for (offset, message) in messages.get(start..end).unwrap_or_default().iter().enumerate() {
    email_text(&mut out, thread, message, start + offset + 1, count, reading.quotes);
  }
  Ok(out)
}

/// One email of a thread as plain text, after a line that says which it is.
fn email_text(out: &mut String, thread: &str, message: &MailMessage, number: usize, count: usize, quotes: bool) {
  let draft = if message.draft {
    ", a draft, not sent"
  } else {
    ""
  };
  let _ = writeln!(out, "\n--- Email {number} of {count} (id {}{draft}) ---", message.id);
  let _ = writeln!(out, "From: {}", person(&message.from));
  if !message.to.is_empty() {
    let _ = writeln!(out, "To: {}", people(&message.to));
  }
  if !message.cc.is_empty() {
    let _ = writeln!(out, "Cc: {}", people(&message.cc));
  }
  if !message.bcc.is_empty() {
    let _ = writeln!(out, "Bcc: {}", people(&message.bcc));
  }
  let unread = if message.unread { " (unread)" } else { "" };
  let _ = writeln!(out, "Date: {}{unread}", stamp(message.time));
  if !message.attachments.is_empty() {
    let _ = writeln!(
      out,
      "Files (open one with mail_attachment thread=\"{thread}\" message=\"{}\" file=<number>):",
      message.id
    );
    for (index, file) in message.attachments.iter().enumerate() {
      let _ = writeln!(out, "{}. {} ({}, {})", index + 1, file.name, file.mime, size_text(file.size));
    }
  }
  let unquoted = if quotes {
    None
  } else {
    crate::mail::html_to_text_unquoted(&message.html)
  };
  let text = unquoted.as_deref().unwrap_or(&message.text);
  out.push('\n');
  out.push_str(if text.is_empty() { "(no text)" } else { text });
  out.push('\n');
  if unquoted.is_some() {
    out.push_str("(The earlier emails it quotes were left out; quotes=true shows them.)\n");
  }
}

async fn mail_read_thread(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailReadArgs = parse(arguments)?;
  let thread = thread_arg(&args.thread)?;
  let messages = thread_messages(engine, &thread).await?;
  let reading = Reading {
    message: args.message.as_deref(),
    last: args.last,
    quotes: args.quotes,
  };
  Ok(keep_end(thread_text(&thread, &messages, &reading)?))
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

// ---- Files ----

/// The file of an email a model means: its name as listed (exactly, then in any case), else its number, 1 the first.
fn pick_file<'a>(files: &'a [MailAttachment], wanted: &str) -> Result<&'a MailAttachment, String> {
  let wanted = wanted.trim();
  if files.is_empty() {
    return Err("That email has no files.".into());
  }
  let named = files
    .iter()
    .find(|file| file.name == wanted)
    .or_else(|| files.iter().find(|file| file.name.eq_ignore_ascii_case(wanted)));
  let numbered = || {
    wanted
      .parse::<usize>()
      .ok()
      .and_then(|number| number.checked_sub(1))
      .and_then(|index| files.get(index))
  };
  named.or_else(numbered).ok_or_else(|| {
    let listed: Vec<String> = files
      .iter()
      .enumerate()
      .map(|(index, file)| format!("{}. {}", index + 1, file.name))
      .collect();
    format!(
      "That email has no file \"{wanted}\". Its files: {}. Pass a name or a number.",
      listed.join(", ")
    )
  })
}

/// A file's name in `paths::scratch_dir()`: a hash of where it came from, so two files of the same name never take
/// each other's place, then its name with only letters, digits, `.`, `-`, and `_` (no path separators), its end kept
/// when it is long.
fn scratch_name(url: &str, name: &str) -> String {
  const MAX_NAME: usize = 120;
  let mut hasher = DefaultHasher::new();
  url.hash(&mut hasher);
  let safe: String = name
    .chars()
    .map(|found| {
      if found.is_alphanumeric() || matches!(found, '.' | '-' | '_') {
        found
      } else {
        '_'
      }
    })
    .collect();
  let safe = safe.trim_start_matches('.');
  let mut kept = 0;
  let mut tail: Vec<char> = safe
    .chars()
    .rev()
    .take_while(|found| {
      kept += found.len_utf8();
      kept <= MAX_NAME
    })
    .collect();
  tail.reverse();
  let tail: String = tail.into_iter().collect();
  format!(
    "{:016x}-{}",
    hasher.finish(),
    if tail.is_empty() {
      "file"
    } else {
      tail.as_str()
    }
  )
}

/// A type whose bytes read as text: `text/*`, JSON, CSV, and XML.
fn is_text(mime: &str) -> bool {
  let mime = mime.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
  mime.starts_with("text/")
    || mime.ends_with("/json")
    || mime.ends_with("+json")
    || mime.ends_with("/xml")
    || mime.ends_with("+xml")
    || mime.ends_with("/csv")
}

/// A file's text when its type is text, HTML read as `html_to_text` reads it; clipped as answers are.
fn file_text(mime: &str, bytes: &[u8]) -> Option<String> {
  if !is_text(mime) {
    return None;
  }
  let raw = String::from_utf8_lossy(bytes);
  let lower = mime.to_ascii_lowercase();
  let text = if lower.starts_with("text/html") || lower.starts_with("application/xhtml") {
    crate::mail::html_to_text(&raw)
  } else {
    raw.into_owned()
  };
  Some(keep_start(text, MAX_ANSWER - 2_000))
}

/// What `mail_attachment` answers for a file saved at `path`.
fn file_answer(file: &MailAttachment, bytes: &[u8], path: &Path) -> String {
  let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
  let mut out = format!(
    "File: {}\nType: {}\nSize: {}\nSaved at: {}\nWorking empties that folder when it starts.\n",
    file.name,
    file.mime,
    size_text(size),
    path.display()
  );
  if let Some(text) = file_text(&file.mime, bytes) {
    let _ = write!(out, "\nIts text:\n{text}\n");
  }
  out
}

/// The answer for a file over `MAX_FILE`.
fn too_big(file: &MailAttachment) -> String {
  format!(
    "{} is {}: mail_attachment opens files up to 25 MB. The user can open it in Gmail.",
    file.name,
    size_text(file.size)
  )
}

async fn mail_attachment(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailAttachmentArgs = parse(arguments)?;
  let thread = thread_arg(&args.thread)?;
  let wanted = args
    .file
    .ok_or("`file` is missing: pass the file's name as mail_read_thread lists it, or its number in the email.")?;
  let id = args.message.trim();
  let messages = thread_messages(engine, &thread).await?;
  let message = messages.iter().find(|message| message.id == id).ok_or_else(|| {
    format!("Email {id} is not in thread {thread}. Pass an email id that mail_read_thread gave for this thread.")
  })?;
  let file = pick_file(&message.attachments, &wanted)?;
  if file.size > MAX_FILE {
    return Err(too_big(file));
  }
  let bytes = engine
    .mail_file(&file.url)
    .await
    .map_err(|error| format!("{} could not be downloaded: {error:#}. Try again in a moment.", file.name))?;
  if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_FILE {
    return Err(too_big(file));
  }
  let path = crate::paths::scratch_dir().join(scratch_name(&file.url, &file.name));
  let target = path.clone();
  let bytes = tokio::task::spawn_blocking(move || crate::paths::write_private(&target, &bytes).map(|()| bytes))
    .await
    .map_err(|error| format!("The file could not be saved: {error}."))?
    .map_err(|error| format!("The file could not be saved: {error}."))?;
  Ok(file_answer(file, &bytes, &path))
}

// ---- Writing ----

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

/// The recipients as written, read as addresses and checked as `ComposeMail` checks them: someone in `to`, `cc`, or
/// `bcc`, each a valid address.
fn addresses_of(to: &[String], cc: &[String], bcc: &[String]) -> Result<[Vec<MailAddress>; 3], String> {
  let read = |list: &[String]| -> Vec<MailAddress> { list.iter().map(|text| address_of(text)).collect() };
  let (to, cc, bcc) = (read(to), read(cc), read(bcc));
  crate::mail::check_recipients(&to, &cc, &bcc).map_err(|error| {
    format!("{error} Give addresses such as jpark@example.com or Jordan Park <jpark@example.com>. Nothing was sent.")
  })?;
  Ok([to, cc, bcc])
}

/// A new email as `mail_send` and `mail_draft` take it, checked as `ComposeMail` is.
fn compose_of(
  to: &[String],
  cc: &[String],
  bcc: &[String],
  subject: &str,
  text: &str,
  signature: Option<bool>,
) -> Result<MailCompose, String> {
  let [to, cc, bcc] = addresses_of(to, cc, bcc)?;
  Ok(MailCompose {
    to,
    cc,
    bcc,
    subject: subject.trim().to_string(),
    text: send_text(text)?,
    signature: signature.unwrap_or(true),
  })
}

async fn mail_send(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailSendArgs = parse(arguments)?;
  let compose = compose_of(&args.to, &args.cc, &args.bcc, &args.subject, &args.text, args.signature)?;
  let went = recipients(&compose.to, &compose.cc, &compose.bcc);
  let thread = engine
    .agent_compose(compose)
    .await
    .map_err(|error| not_written("Sending", &error))?;
  Ok(format!(
    "Sent to {went}.\nThread: {thread}\nread: mail_read_thread thread=\"{thread}\"\n"
  ))
}

/// The email a reply answers and who the reply goes to.
struct ReplyPlan<'a> {
  answered: &'a MailMessage,
  to: Vec<MailAddress>,
  cc: Vec<MailAddress>,
}

/// The email a reply answers and who it goes to: `wanted` by its id, else the thread's newest email from someone
/// else, else its newest; never a draft. The recipients are Gmail's Reply (or Reply all) of that email, never anyone
/// else.
fn reply_plan<'a>(
  messages: &'a [MailMessage],
  me: &str,
  wanted: Option<&str>,
  all: bool,
) -> Result<ReplyPlan<'a>, String> {
  let answered = if let Some(id) = wanted {
    let found = messages.iter().find(|message| message.id == id).ok_or_else(|| {
      format!(
        "Email {id} is not in this thread. Pass an email id that mail_read_thread gave, or leave `message` out to \
         answer the newest email from someone else. Nothing was sent."
      )
    })?;
    if found.draft {
      return Err(format!(
        "Email {id} is a draft, not sent: answer an email that was sent. Nothing was sent."
      ));
    }
    found
  } else {
    let sent = || messages.iter().rev().filter(|message| !message.draft);
    sent()
      .find(|message| !message.from.email.eq_ignore_ascii_case(me))
      .or_else(|| sent().next())
      .ok_or("The thread has no sent emails to answer. Nothing was sent.")?
  };
  let (to, cc) = crate::mail::reply_recipients(answered, me, all);
  if to.is_empty() && cc.is_empty() {
    return Err("The email answered names nobody to send a reply to. Nothing was sent.".into());
  }
  Ok(ReplyPlan { answered, to, cc })
}

/// A reply as `mail_reply` and `mail_draft` make it: the thread read now, the email it answers chosen by
/// `reply_plan`, and the recipients from that email. With the words that name the email answered.
async fn plan_reply(
  engine: &Arc<Engine>,
  thread: &str,
  text: &str,
  all: bool,
  message: Option<&str>,
  signature: Option<bool>,
) -> Result<(MailReply, String), String> {
  let thread = thread_arg(thread)?;
  let text = send_text(text)?;
  let me = engine
    .mail_me()
    .await
    .map_err(|error| format!("Gmail could not be reached: {error:#}. Try again in a moment."))?
    .ok_or("Gmail did not say which address sends mail. Nothing was sent.")?;
  let messages = thread_messages(engine, &thread).await?;
  let ReplyPlan { answered, to, cc } = reply_plan(&messages, &me.email, message, all)?;
  let answering = format!("the email from {} of {}", person(&answered.from), stamp(answered.time));
  let reply = MailReply {
    thread,
    message: answered.id.clone(),
    to,
    cc,
    text,
    signature: signature.unwrap_or(true),
  };
  Ok((reply, answering))
}

/// A parse error of a tool that takes no recipients, saying where they come from.
fn no_recipients(error: String) -> String {
  if error.contains("unknown field") {
    format!("{error} mail_reply takes no recipients: they come from the email answered (reply_all for everyone).")
  } else {
    error
  }
}

async fn mail_reply(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailReplyArgs = parse(arguments).map_err(no_recipients)?;
  let (reply, answering) = plan_reply(
    engine,
    &args.thread,
    &args.text,
    args.reply_all,
    args.message.as_deref(),
    args.signature,
  )
  .await?;
  let out = format!(
    "Replied in thread {} to {}, answering {answering}.\n",
    reply.thread,
    recipients(&reply.to, &reply.cc, &[])
  );
  engine
    .agent_reply(reply)
    .await
    .map_err(|error| not_written("Replying", &error))?;
  Ok(out)
}

/// The email `mail_forward` sends on: `wanted` by its id, else the thread's newest email that is not a draft.
fn forwarded<'a>(messages: &'a [MailMessage], wanted: Option<&str>) -> Result<&'a MailMessage, String> {
  match wanted {
    Some(id) => {
      let found = messages.iter().find(|message| message.id == id).ok_or_else(|| {
        format!(
          "Email {id} is not in this thread. Pass an email id that mail_read_thread gave, or leave `message` out to \
           forward the newest email. Nothing was sent."
        )
      })?;
      if found.draft {
        return Err(format!(
          "Email {id} is a draft, not sent: forward an email that was sent, or send the draft with mail_send_draft. \
           Nothing was sent."
        ));
      }
      Ok(found)
    },
    None => messages
      .iter()
      .rev()
      .find(|message| !message.draft)
      .ok_or_else(|| "The thread has no sent emails to forward. Nothing was sent.".to_string()),
  }
}

/// What `mail_forward` answers: who the email went to, the files left out, and the new thread.
fn forward_answer(email: &MailMessage, went: &str, thread: &str) -> String {
  let mut out = format!(
    "Forwarded the email from {} of {} to {went}.\n",
    person(&email.from),
    stamp(email.time)
  );
  match email.attachments.len() {
    0 => {},
    1 => out.push_str("Its 1 attached file was not sent with it.\n"),
    count => {
      let _ = writeln!(out, "Its {count} attached files were not sent with it.");
    },
  }
  let _ = write!(out, "New thread: {thread}\nread: mail_read_thread thread=\"{thread}\"\n");
  out
}

async fn mail_forward(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailForwardArgs = parse(arguments)?;
  let thread = thread_arg(&args.thread)?;
  let [to, cc, bcc] = addresses_of(&args.to, &args.cc, &args.bcc)?;
  let messages = thread_messages(engine, &thread).await?;
  let email = forwarded(&messages, args.message.as_deref())?;
  let went = recipients(&to, &cc, &bcc);
  let forward = MailForward {
    thread,
    message: email.id.clone(),
    to,
    cc,
    bcc,
    text: args.text,
    signature: args.signature.unwrap_or(true),
  };
  let new_thread = engine
    .agent_forward(forward)
    .await
    .map_err(|error| not_written("Forwarding", &error))?;
  Ok(forward_answer(email, &went, &new_thread))
}

/// What `mail_draft` answers for the draft saved as `id`.
fn draft_answer(id: &str, draft: &MailDraft, answering: Option<&str>) -> String {
  let (to, cc, bcc): (&[MailAddress], &[MailAddress], &[MailAddress]) = match draft {
    MailDraft::New(compose) => (compose.to.as_slice(), compose.cc.as_slice(), compose.bcc.as_slice()),
    MailDraft::Reply(reply) => (reply.to.as_slice(), reply.cc.as_slice(), &[]),
  };
  let mut out = format!("Saved in Gmail's Drafts, not sent.\nDraft: {id}\n");
  if !to.is_empty() {
    let _ = writeln!(out, "To: {}", people(to));
  }
  if !cc.is_empty() {
    let _ = writeln!(out, "Cc: {}", people(cc));
  }
  if !bcc.is_empty() {
    let _ = writeln!(out, "Bcc: {}", people(bcc));
  }
  match draft {
    MailDraft::New(compose) if !compose.subject.is_empty() => {
      let _ = writeln!(out, "Subject: {}", compose.subject);
    },
    MailDraft::New(_) => {},
    MailDraft::Reply(reply) => {
      let _ = writeln!(out, "Thread: {}", reply.thread);
      if let Some(answering) = answering {
        let _ = writeln!(out, "It answers {answering}.");
      }
    },
  }
  let _ = writeln!(out, "send: mail_send_draft draft=\"{id}\"");
  out
}

async fn mail_draft(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailDraftArgs = parse(arguments)?;
  let (draft, answering) = if let Some(thread) = &args.thread {
    if !args.to.is_empty() || !args.cc.is_empty() || !args.bcc.is_empty() || args.subject.is_some() {
      return Err(
        "A reply takes its recipients and subject from the email answered: leave out `to`, `cc`, `bcc`, and \
         `subject` with `thread` (reply_all for everyone). Nothing was saved."
          .into(),
      );
    }
    let (reply, answering) = plan_reply(
      engine,
      thread,
      &args.text,
      args.reply_all.unwrap_or_default(),
      args.message.as_deref(),
      args.signature,
    )
    .await?;
    (MailDraft::Reply(reply), Some(answering))
  } else {
    if args.reply_all.is_some() || args.message.is_some() {
      return Err("`reply_all` and `message` are for a reply: pass its `thread` too. Nothing was saved.".into());
    }
    let subject = args.subject.as_deref().unwrap_or_default();
    let compose = compose_of(&args.to, &args.cc, &args.bcc, subject, &args.text, args.signature)?;
    (MailDraft::New(compose), None)
  };
  let id = engine
    .agent_draft(draft.clone())
    .await
    .map_err(|error| format!("Saving the draft failed: {error}. Nothing was saved; try again in a moment."))?;
  Ok(draft_answer(&id, &draft, answering.as_deref()))
}

/// What `mail_send_draft` answers for the draft `id` sent.
fn sent_draft_answer(id: &str, sent: &MailDraftSent) -> String {
  format!(
    "Sent draft {id} to {}.\nThread: {}\nread: mail_read_thread thread=\"{}\"\n",
    recipients(&sent.to, &sent.cc, &sent.bcc),
    sent.thread,
    sent.thread
  )
}

async fn mail_send_draft(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailSendDraftArgs = parse(arguments)?;
  let id = args.draft.trim();
  if id.is_empty() {
    return Err("`draft` is empty. Pass the draft's id that mail_draft gave.".into());
  }
  let sent = engine
    .agent_send_draft(id)
    .await
    .map_err(|error| format!("Sending the draft failed: {error}."))?;
  Ok(sent_draft_answer(id, &sent))
}

/// What a change of several threads did: each thread done, and each that failed with why.
#[derive(Default)]
struct Batch {
  done: Vec<String>,
  failed: Vec<(String, String)>,
}

impl Batch {
  /// Writes `threads` in turn, one at a time.
  async fn run<F, Fut>(threads: Vec<String>, mut write: F) -> Self
  where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<(), String>>,
  {
    let mut batch = Self::default();
    for thread in threads {
      match write(thread.clone()).await {
        Ok(()) => batch.done.push(thread),
        Err(error) => batch.failed.push((thread, error)),
      }
    }
    batch
  }

  /// The answer: `did` the threads done (`Archived`), with `note` after their count; each failure with why. An error
  /// only when none was done, `doing` naming the change (`Archiving`).
  fn answer(&self, did: &str, note: &str, doing: &str) -> Result<String, String> {
    let mut failures = String::new();
    for (thread, error) in &self.failed {
      let _ = writeln!(failures, "- {thread}: {error}");
    }
    if self.done.is_empty() {
      return Err(format!(
        "{doing} failed; nothing was changed. Try again in a moment.\n{failures}"
      ));
    }
    let mut out = format!("{did} {}{note}:\n", threads_word(self.done.len()));
    for thread in &self.done {
      let _ = writeln!(out, "- {thread}");
    }
    if !self.failed.is_empty() {
      let _ = write!(
        out,
        "Not done for {} (the others above were):\n{failures}",
        threads_word(self.failed.len())
      );
    }
    Ok(out)
  }
}

async fn mail_archive(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailArchiveArgs = parse(arguments)?;
  let threads = thread_args(&args.thread)?;
  let archive = args.archive.unwrap_or(true);
  let engine: &Engine = engine;
  let batch = Batch::run(threads, move |thread| engine.agent_archive(thread, archive)).await;
  if archive {
    batch.answer("Archived", " (out of the inbox, still in All Mail)", "Archiving")
  } else {
    batch.answer("Moved", " back to the inbox", "Moving to the inbox")
  }
}

async fn mail_mark(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailMarkArgs = parse(arguments)?;
  let threads = thread_args(&args.thread)?;
  let read = args
    .read
    .ok_or("`read` is missing: true marks read, false unread. Nothing was changed.")?;
  let engine: &Engine = engine;
  let batch = Batch::run(threads, move |thread| engine.agent_mark(thread, read)).await;
  batch.answer("Marked", if read { " read" } else { " unread" }, "Marking")
}

async fn mail_star(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailStarArgs = parse(arguments)?;
  let threads = thread_args(&args.thread)?;
  let starred = args.starred.unwrap_or(true);
  let engine: &Engine = engine;
  let batch = Batch::run(threads, move |thread| engine.agent_star(thread, starred)).await;
  if starred {
    batch.answer("Starred", "", "Starring")
  } else {
    batch.answer("Took the star off", "", "Taking the star off")
  }
}

/// The labels `mail_label` adds and removes, by the names given, each once. An error lists every name that points
/// at no label, or at several, and every label both added and removed.
fn label_change<'a>(
  labels: &'a [MailLabel],
  add: &[String],
  remove: &[String],
) -> Result<(Vec<&'a MailLabel>, Vec<&'a MailLabel>), String> {
  if add.is_empty() && remove.is_empty() {
    return Err("Pass `add`, `remove`, or both: label names as mail_labels gives them. Nothing was changed.".into());
  }
  let mut problems = Vec::new();
  let added = labels_named(labels, add, &mut problems);
  let removed = labels_named(labels, remove, &mut problems);
  for label in &added {
    if removed.iter().any(|other| other.id == label.id) {
      problems.push(format!("\"{}\" is both added and removed.", label.name));
    }
  }
  if problems.is_empty() {
    Ok((added, removed))
  } else {
    Err(format!("{}\nNothing was changed.", problems.join("\n")))
  }
}

/// The labels `names` point at, each once; a name that points at none or several adds why to `problems`.
fn labels_named<'a>(labels: &'a [MailLabel], names: &[String], problems: &mut Vec<String>) -> Vec<&'a MailLabel> {
  let mut found: Vec<&MailLabel> = Vec::with_capacity(names.len());
  for name in names {
    match find_label(labels, name) {
      Ok(label) if !found.iter().any(|seen| seen.id == label.id) => found.push(label),
      Ok(_) => {},
      Err(problem) => problems.push(problem),
    }
  }
  found
}

/// `added Clients/Acme; removed Receipts`.
fn label_note(added: &[&MailLabel], removed: &[&MailLabel]) -> String {
  let names = |labels: &[&MailLabel]| labels.iter().map(|label| label.name.as_str()).collect::<Vec<_>>().join(", ");
  let mut parts = Vec::new();
  if !added.is_empty() {
    parts.push(format!("added {}", names(added)));
  }
  if !removed.is_empty() {
    parts.push(format!("removed {}", names(removed)));
  }
  format!(" ({})", parts.join("; "))
}

async fn mail_label(engine: &Arc<Engine>, arguments: Value) -> Result<String, String> {
  let args: MailLabelArgs = parse(arguments)?;
  let threads = thread_args(&args.thread)?;
  let labels = engine.mail_labels().await.map_err(|error| mail_offline(&error))?;
  let (added, removed) = label_change(&labels, &args.add, &args.remove)?;
  let note = label_note(&added, &removed);
  let add: Vec<String> = added.iter().map(|label| label.id.clone()).collect();
  let remove: Vec<String> = removed.iter().map(|label| label.id.clone()).collect();
  let engine: &Engine = engine;
  let batch = Batch::run(threads, move |thread| engine.agent_label(thread, add.clone(), remove.clone())).await;
  batch.answer("Labelled", &note, "Labelling")
}

#[cfg(test)]
mod tests {
  use super::{
    Batch, Gate, MAIL_OFF, MailArchiveArgs, MailAttachmentArgs, MailDraftArgs, MailForwardArgs, MailInboxArgs,
    MailLabelArgs, MailMarkArgs, MailReadArgs, MailReplyArgs, MailSendArgs, MailStarArgs, READ_ONLY_MODE, Reading,
    TOOLS, WRITE_OFF, WRITE_TOOLS, address_of, compose_of, draft_answer, file_answer, file_text, find_label,
    forward_answer, forwarded, is_text, label_change, label_lines, label_note, list, page_answer, page_arg, pick_file,
    recipients, refusal, reply_plan, scratch_name, send_text, sent_draft_answer, size_text, thread_arg, thread_args,
    thread_list, thread_text,
  };
  use crate::mail::ThreadPage;
  use crate::mcp::shared::parse;
  use crate::model::{
    MailAddress, MailAttachment, MailDraft, MailDraftSent, MailLabel, MailMessage, MailReply, MailThread,
  };
  use serde_json::json;

  fn names(tools: &[serde_json::Value]) -> Vec<&str> {
    tools.iter().filter_map(|tool| tool["name"].as_str()).collect()
  }

  #[test]
  fn read_tools_are_always_listed_and_write_tools_only_when_allowed() {
    let reading = list(false);
    assert_eq!(names(&reading), TOOLS);
    for tool in &reading {
      assert_eq!(tool["annotations"]["readOnlyHint"], true, "{}", tool["name"]);
    }
    let all = list(true);
    let expected: Vec<&str> = TOOLS.iter().chain(WRITE_TOOLS.iter()).copied().collect();
    assert_eq!(names(&all), expected);
    for tool in all.iter().filter(|tool| WRITE_TOOLS.contains(&tool["name"].as_str().unwrap())) {
      assert_eq!(tool["annotations"]["readOnlyHint"], false, "{}", tool["name"]);
      assert_eq!(tool["annotations"]["destructiveHint"], false, "{}", tool["name"]);
    }
    // Sending twice sends twice; changing a thread twice does not.
    let idempotent = |name: &str| {
      all.iter().find(|tool| tool["name"] == name).unwrap()["annotations"]["idempotentHint"]
        .as_bool()
        .unwrap()
    };
    for name in ["mail_send", "mail_reply", "mail_forward", "mail_draft", "mail_send_draft"] {
      assert!(!idempotent(name), "{name}");
    }
    for name in ["mail_archive", "mail_mark", "mail_star", "mail_label"] {
      assert!(idempotent(name), "{name}");
    }
    let schema = |name: &str| all.iter().find(|tool| tool["name"] == name).unwrap()["inputSchema"].clone();
    assert!(
      schema("mail_reply")["properties"].get("to").is_none(),
      "no recipients from the agent"
    );
    assert_eq!(schema("mail_attachment")["required"], json!(["thread", "message", "file"]));
    assert!(schema("mail_label")["properties"].get("create").is_none());
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
  fn read_arguments_are_parsed() {
    let args: MailInboxArgs =
      parse(json!({ "mailbox": "sent", "limit": "5", "unread_only": "true", "page": "3" })).unwrap();
    assert_eq!((args.limit, args.page), (Some(5), Some(3)));
    assert!(args.unread_only);
    assert_eq!(page_arg(None), Ok(1));
    assert_eq!(page_arg(Some(0)), Ok(1));
    assert_eq!(page_arg(Some(4)), Ok(4));
    let read: MailReadArgs = parse(json!({ "thread": "thread-f:1", "last": 2, "quotes": "true" })).unwrap();
    assert_eq!((read.last, read.quotes, read.message), (Some(2), true, None));
    let file: MailAttachmentArgs = parse(json!({ "thread": "thread-f:1", "message": "msg-f:1", "file": 2 })).unwrap();
    assert_eq!(file.file.as_deref(), Some("2"));
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
    assert_eq!((mark.thread, mark.read), (vec!["thread-f:1".to_string()], Some(false)));
    // Recipients are not the agent's to give.
    let given = parse::<MailReplyArgs>(json!({ "thread": "thread-f:1", "text": "Hi", "to": ["jpark@example.com"] }));
    assert!(given.is_err());
  }

  #[test]
  fn threads_come_as_one_or_a_list() {
    let list: MailArchiveArgs = parse(json!({ "thread": ["thread-f:1", "#2", "thread-f:1"] })).unwrap();
    assert_eq!(thread_args(&list.thread).unwrap(), ["thread-f:1", "thread-f:2"]);
    let one: MailStarArgs = parse(json!({ "thread": "thread-f:1, thread-f:3", "starred": false })).unwrap();
    assert_eq!(thread_args(&one.thread).unwrap(), ["thread-f:1", "thread-f:3"]);
    assert_eq!(one.starred, Some(false));
    assert!(parse::<MailArchiveArgs>(json!({})).is_err(), "a thread is required");
    let blank: MailArchiveArgs = parse(json!({ "thread": [" "] })).unwrap();
    assert!(thread_args(&blank.thread).unwrap_err().contains("Nothing was changed"));
    let labels: MailLabelArgs =
      parse(json!({ "thread": "thread-f:1", "add": "Clients, Acme", "remove": ["Receipts"] })).unwrap();
    assert_eq!(labels.add, ["Clients, Acme"], "one text is one name");
    assert_eq!(labels.remove, ["Receipts"]);
  }

  #[test]
  fn a_batch_lists_what_was_done_and_what_failed() {
    let batch = Batch {
      done: vec!["thread-f:1".into(), "thread-f:2".into()],
      failed: vec![("thread-f:3".into(), "Gmail did not answer in 30 s".into())],
    };
    assert_eq!(
      batch.answer("Marked", " read", "Marking").unwrap(),
      "Marked 2 threads read:\n- thread-f:1\n- thread-f:2\nNot done for 1 thread (the others above were):\n\
       - thread-f:3: Gmail did not answer in 30 s\n"
    );
    let none = Batch {
      done: Vec::new(),
      failed: vec![("thread-f:3".into(), "Gmail answered 500".into())],
    };
    let error = none.answer("Starred", "", "Starring").unwrap_err();
    assert!(error.starts_with("Starring failed"));
    assert!(error.ends_with("- thread-f:3: Gmail answered 500\n"));
    let one = Batch {
      done: vec!["thread-f:1".into()],
      failed: Vec::new(),
    };
    assert_eq!(
      one.answer("Starred", "", "Starring").unwrap(),
      "Starred 1 thread:\n- thread-f:1\n"
    );
  }

  #[test]
  fn labels_to_change_are_resolved_by_name() {
    let labels = labels();
    let (added, removed) = label_change(&labels, &["acme".to_string()], &["Receipts".to_string()]).unwrap();
    assert_eq!((added[0].id.as_str(), removed[0].id.as_str()), ("^x_2", "^x_3"));
    assert_eq!(label_note(&added, &removed), " (added Clients/Acme; removed Receipts)");
    let error = label_change(&labels, &["Travel".to_string(), "c".to_string()], &[]).unwrap_err();
    assert!(error.contains("No label is named \"Travel\""));
    assert!(error.contains("matches several labels"));
    assert!(error.ends_with("Nothing was changed."));
    let both = label_change(&labels, &["Receipts".to_string()], &["receipts".to_string()]);
    assert!(both.unwrap_err().contains("both"));
    assert!(label_change(&labels, &[], &[]).unwrap_err().contains("`add`, `remove`"));
  }

  fn email(id: &str, from: MailAddress, to: Vec<MailAddress>, cc: Vec<MailAddress>) -> MailMessage {
    MailMessage {
      id: id.into(),
      from,
      to,
      cc,
      bcc: Vec::new(),
      reply_to: Vec::new(),
      time: 1_794_896_454_636_000,
      html: String::new(),
      text: String::new(),
      unread: false,
      draft: false,
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
    let mut thread = vec![
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
    // A draft is never answered.
    let mut draft = email("msg-a:r1", jordan(), vec![avery()], Vec::new());
    draft.draft = true;
    thread.push(draft);
    assert_eq!(reply_plan(&thread, me, None, false).unwrap().answered.id, "msg-f:2");
    assert!(
      reply_plan(&thread, me, Some("msg-a:r1"), false)
        .err()
        .is_some_and(|error| error.contains("draft"))
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

  fn lunch(id: &str) -> MailThread {
    MailThread {
      id: id.into(),
      subject: "Lunch on Friday".into(),
      snippet: "Want to grab lunch?".into(),
      senders: vec![jordan(), riley()],
      time: 1_794_896_454_636_000,
      messages: 3,
      unread: true,
      starred: false,
      attachments: true,
    }
  }

  #[test]
  fn mail_threads_list_flags_and_the_read_call() {
    let thread = lunch("thread-f:42");
    let text = thread_list(&[&thread]);
    assert!(text.contains("] Jordan Park, rbrooks@example.com: Lunch on Friday (3 emails, unread, has files)"));
    assert!(text.contains("\n   Want to grab lunch?\n"));
    assert!(text.ends_with("   read: mail_read_thread thread=\"thread-f:42\"\n"));
  }

  #[test]
  fn a_page_ends_with_the_next_call() {
    let page = ThreadPage {
      threads: vec![lunch("thread-f:1"), lunch("thread-f:2"), lunch("thread-f:3")],
      more: true,
      inbox_unread: None,
      labels: None,
    };
    let call = "mail_inbox mailbox=\"sent\" limit=2";
    let text = page_answer(&page, 2, 2, "No threads.\n", "threads of the sent mail", call);
    assert!(text.starts_with("2 threads of the sent mail (page 2), newest first:\n"));
    assert!(!text.contains("thread-f:3\""));
    assert!(
      text
        .contains("1 thread more on this page: call again with limit=50 to list them, before reading the next page.\n")
    );
    assert!(text.ends_with("Older threads exist. Next page: mail_inbox mailbox=\"sent\" limit=2 page=3\n"));
    let last = ThreadPage {
      threads: vec![lunch("thread-f:1")],
      more: false,
      inbox_unread: Some(4),
      labels: None,
    };
    let text = page_answer(&last, 1, 20, "No threads.\n", "threads of the inbox", "mail_inbox");
    assert!(text.starts_with("Unread threads in the inbox: 4.\n1 threads of the inbox, newest first:\n"));
    assert!(!text.contains("Next page") && !text.contains("more on this page"));
    let past = ThreadPage {
      threads: Vec::new(),
      more: false,
      inbox_unread: None,
      labels: None,
    };
    assert_eq!(
      page_answer(&past, 1, 20, "No threads.\n", "threads", "mail_inbox"),
      "No threads.\n"
    );
    assert!(page_answer(&past, 3, 20, "No threads.\n", "threads", "mail_inbox").contains("Page 3 has no threads"));
  }

  fn menu() -> MailAttachment {
    MailAttachment {
      name: "menu.pdf".into(),
      mime: "application/pdf".into(),
      size: 73_283,
      url: "https://mail.google.com/mail/u/0/?attid=0.1".into(),
    }
  }

  fn notes() -> MailAttachment {
    MailAttachment {
      name: "Notes.csv".into(),
      mime: "text/csv".into(),
      size: 120,
      url: "https://mail.google.com/mail/u/0/?attid=0.2".into(),
    }
  }

  #[test]
  fn mail_thread_reads_as_text() {
    let mut message = email("msg-f:1", jordan(), vec![riley()], Vec::new());
    message.html = "<div>Noon?</div>".into();
    message.text = "Noon?".into();
    message.unread = true;
    message.attachments = vec![menu(), notes()];
    let text = thread_text("thread-f:9", &[message], &Reading::default()).unwrap();
    assert!(text.starts_with(
      "1 email, oldest first.\n\n--- Email 1 of 1 (id msg-f:1) ---\nFrom: Jordan Park <jpark@example.com>\nTo: rbrooks@example.com\nDate: "
    ));
    assert!(text.contains(
      " (unread)\nFiles (open one with mail_attachment thread=\"thread-f:9\" message=\"msg-f:1\" file=<number>):\n\
       1. menu.pdf (application/pdf, 72 KB)\n2. Notes.csv (text/csv, 120 B)\n\nNoon?\n"
    ));
    assert!(!text.contains("Cc:") && !text.contains("<div>") && !text.contains("quotes=true"));
    // The user's own sent email names who got a hidden copy.
    let mut sent = email("msg-f:2", avery(), vec![jordan()], vec![riley()]);
    sent.bcc = vec![avery()];
    let text = thread_text("thread-f:9", &[sent], &Reading::default()).unwrap();
    assert!(text.contains("\nCc: rbrooks@example.com\nBcc: Avery Morgan <amorgan@example.com>\nDate: "));
  }

  fn conversation() -> Vec<MailMessage> {
    let mut first = email("msg-f:1", jordan(), vec![avery()], Vec::new());
    first.text = "Noon?".into();
    let mut second = email("msg-f:2", avery(), vec![jordan()], Vec::new());
    second.html =
      "<div>Sure.</div><div class=\"gmail_quote\"><blockquote class=\"gmail_quote\">Noon?</blockquote></div>".into();
    second.text = "Sure.\n\nNoon?".into();
    let mut third = email("msg-a:r7", avery(), vec![jordan()], Vec::new());
    third.text = "See you there.".into();
    third.draft = true;
    vec![first, second, third]
  }

  #[test]
  fn a_thread_reads_bounded_and_without_quotes() {
    let thread = conversation();
    let all = thread_text("thread-f:9", &thread, &Reading::default()).unwrap();
    assert!(all.starts_with("3 emails, oldest first.\n"));
    assert!(all.contains("\nSure.\n(The earlier emails it quotes were left out; quotes=true shows them.)\n"));
    assert!(all.contains("--- Email 3 of 3 (id msg-a:r7, a draft, not sent) ---"));
    let quoted = Reading { quotes: true, ..Reading::default() };
    assert!(
      thread_text("thread-f:9", &thread, &quoted)
        .unwrap()
        .contains("\nSure.\n\nNoon?\n")
    );
    let newest = Reading { last: Some(1), ..Reading::default() };
    let text = thread_text("thread-f:9", &thread, &newest).unwrap();
    assert!(text.starts_with(
      "The newest 1 of 3 emails, oldest first. 2 older left out: mail_read_thread thread=\"thread-f:9\" last=3 reads \
       them all, or message=\"<id>\" one of them (msg-f:1, msg-f:2).\n"
    ));
    assert!(text.contains("Email 3 of 3") && !text.contains("Email 2 of 3"));
    let more = Reading { last: Some(5), ..Reading::default() };
    assert!(
      thread_text("thread-f:9", &thread, &more)
        .unwrap()
        .starts_with("3 emails, oldest first.")
    );
    let one = Reading {
      message: Some("msg-f:1"),
      ..Reading::default()
    };
    let text = thread_text("thread-f:9", &thread, &one).unwrap();
    assert!(text.contains("Email 1 of 3 (id msg-f:1)") && !text.contains("Email 2"));
    let missing = Reading {
      message: Some("msg-f:8"),
      ..Reading::default()
    };
    assert!(
      thread_text("thread-f:9", &thread, &missing)
        .unwrap_err()
        .contains("msg-f:1, msg-f:2, msg-a:r7")
    );
    let both = Reading {
      message: Some("msg-f:1"),
      last: Some(1),
      quotes: false,
    };
    assert!(thread_text("thread-f:9", &thread, &both).is_err());
  }

  #[test]
  fn files_are_picked_named_and_read() {
    let files = [menu(), notes()];
    assert_eq!(pick_file(&files, "menu.pdf").unwrap().name, "menu.pdf");
    assert_eq!(pick_file(&files, "notes.CSV").unwrap().name, "Notes.csv");
    assert_eq!(pick_file(&files, "2").unwrap().name, "Notes.csv");
    assert!(pick_file(&files, "3").unwrap_err().contains("1. menu.pdf, 2. Notes.csv"));
    assert!(pick_file(&files, "0").is_err());
    assert!(pick_file(&[], "1").is_err());
    let name = scratch_name("https://mail.google.com/a", "../Q3 report/é.pdf");
    assert!(name.ends_with("-_Q3_report_é.pdf"), "{name}");
    assert!(!name.contains('/') && !name.contains(".."));
    assert_ne!(name, scratch_name("https://mail.google.com/b", "../Q3 report/é.pdf"));
    assert!(scratch_name("u", "..").ends_with("-file"));
    assert!(scratch_name("u", &"a".repeat(400)).len() < 200);
    assert!(is_text("text/plain; charset=utf-8") && is_text("application/json") && is_text("application/xml"));
    assert!(!is_text("application/pdf") && !is_text("image/png"));
    assert_eq!(file_text("text/csv", b"a,b\n1,2").as_deref(), Some("a,b\n1,2"));
    assert_eq!(file_text("text/html", b"<p>Hi</p><p>there</p>").as_deref(), Some("Hi\n\nthere"));
    assert_eq!(file_text("application/pdf", b"%PDF"), None);
    let saved = std::path::Path::new("/scratch/0123456789abcdef-menu.pdf");
    let text = file_answer(&menu(), b"%PDF", saved);
    assert_eq!(
      text,
      "File: menu.pdf\nType: application/pdf\nSize: 4 B\nSaved at: /scratch/0123456789abcdef-menu.pdf\n\
       Working empties that folder when it starts.\n"
    );
    let csv = file_answer(&notes(), b"a,b", saved);
    assert!(csv.ends_with("starts.\n\nIts text:\na,b\n"));
  }

  #[test]
  fn file_sizes_read_naturally() {
    assert_eq!(size_text(512), "512 B");
    assert_eq!(size_text(73_283), "72 KB");
    assert_eq!(size_text(3_670_016), "3.5 MB");
  }

  #[test]
  fn drafts_answer_their_id_recipients_and_send_call() {
    let args: MailDraftArgs =
      parse(json!({ "to": "jpark@example.com", "bcc": "rbrooks@example.com", "text": "Noon?" })).unwrap();
    assert!(
      parse::<MailDraftArgs>(json!({ "text": "Noon?", "draft": "msg-a:r5" })).is_err(),
      "no saving over"
    );
    let compose = compose_of(&args.to, &args.cc, &args.bcc, "Lunch", &args.text, args.signature).unwrap();
    assert!(compose.signature);
    let text = draft_answer("msg-a:r5", &MailDraft::New(compose), None);
    assert_eq!(
      text,
      "Saved in Gmail's Drafts, not sent.\nDraft: msg-a:r5\nTo: jpark@example.com\nBcc: rbrooks@example.com\n\
       Subject: Lunch\n\
       send: mail_send_draft draft=\"msg-a:r5\"\n"
    );
    assert!(
      compose_of(&[], &[], &[], "", "Noon?", None)
        .unwrap_err()
        .contains("Nothing was sent")
    );
    let reply = MailReply {
      thread: "thread-f:9".into(),
      message: "msg-f:2".into(),
      to: vec![jordan()],
      cc: vec![riley()],
      text: "Sure.".into(),
      signature: true,
    };
    let text = draft_answer("msg-a:r6", &MailDraft::Reply(reply), Some("the email from Jordan Park"));
    assert!(text.contains("To: Jordan Park <jpark@example.com>\nCc: rbrooks@example.com\nThread: thread-f:9\n"));
    assert!(text.contains("It answers the email from Jordan Park.\n"));
    let sent = MailDraftSent {
      thread: "thread-f:9".into(),
      to: vec![jordan()],
      cc: Vec::new(),
      bcc: vec![riley()],
    };
    assert_eq!(
      sent_draft_answer("msg-a:r6", &sent),
      "Sent draft msg-a:r6 to Jordan Park <jpark@example.com>, hidden copy to rbrooks@example.com.\n\
       Thread: thread-f:9\nread: mail_read_thread thread=\"thread-f:9\"\n"
    );
    // Only a copy hidden from everyone is still someone to send to.
    let hidden = compose_of(&[], &[], &["rbrooks@example.com".to_string()], "", "Noon?", None).unwrap();
    assert_eq!(hidden.bcc, vec![riley()]);
  }

  #[test]
  fn recipients_name_copies_and_hidden_copies_apart() {
    assert_eq!(recipients(&[jordan()], &[], &[]), "Jordan Park <jpark@example.com>");
    assert_eq!(
      recipients(&[jordan()], &[riley()], &[avery()]),
      "Jordan Park <jpark@example.com>, copy to rbrooks@example.com, hidden copy to Avery Morgan \
       <amorgan@example.com>"
    );
    assert_eq!(recipients(&[], &[], &[riley()]), "hidden copy to rbrooks@example.com");
  }

  #[test]
  fn a_forward_sends_the_newest_sent_email_without_its_files() {
    let args: MailForwardArgs = parse(json!({
      "thread": "thread-f:9",
      "bcc": "Jordan Park <jpark@example.com>; rbrooks@example.com"
    }))
    .unwrap();
    assert!(args.to.is_empty() && args.text.is_empty() && args.message.is_none());
    assert_eq!(args.bcc.len(), 2);
    assert!(
      parse::<MailForwardArgs>(json!({ "to": "jpark@example.com" })).is_err(),
      "a thread is required"
    );
    let mut thread = conversation();
    assert_eq!(forwarded(&thread, None).unwrap().id, "msg-f:2", "the draft is passed over");
    assert_eq!(forwarded(&thread, Some("msg-f:1")).unwrap().id, "msg-f:1");
    assert!(forwarded(&thread, Some("msg-a:r7")).unwrap_err().contains("draft"));
    assert!(forwarded(&thread, Some("msg-f:8")).unwrap_err().contains("not in this thread"));
    thread[0].attachments = vec![menu(), notes()];
    let text = forward_answer(&thread[0], "copy to rbrooks@example.com", "thread-f:10");
    assert!(text.starts_with("Forwarded the email from Jordan Park <jpark@example.com> of "));
    assert!(text.ends_with(
      " to copy to rbrooks@example.com.\nIts 2 attached files were not sent with it.\nNew thread: thread-f:10\n\
       read: mail_read_thread thread=\"thread-f:10\"\n"
    ));
    assert!(!forward_answer(&thread[1], "jpark@example.com", "thread-f:11").contains("attached"));
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
