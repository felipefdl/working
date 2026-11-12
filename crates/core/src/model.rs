//! Plain data the UI renders. Everything here is already resolved: names, byte offsets, absolute URLs.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Microseconds since the Unix epoch, as Google Chat reports every timestamp.
pub type Micros = i64;

/// Conversation identity: a one-to-one or group DM, or a named space.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GroupId {
  /// Direct message (one-to-one or group DM) id.
  Dm(String),
  /// Named space id.
  Space(String),
}

impl GroupId {
  /// Stable text form, safe as a file name: `dm_<id>` or `space_<id>`.
  pub fn key(&self) -> String {
    match self {
      Self::Dm(id) => format!("dm_{id}"),
      Self::Space(id) => format!("space_{id}"),
    }
  }
}

/// Kind of conversation, as the sidebar groups them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupKind {
  /// One-to-one conversation with a person.
  Dm,
  /// One-to-one conversation with an app.
  App,
  /// Unnamed conversation between several people.
  GroupDm,
  /// Named space.
  Space,
}

/// Last visible line of a conversation, for the sidebar and Home.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
  /// Sender user id.
  pub sender_id: String,
  /// Sender display name.
  pub sender_name: String,
  /// Plain text of the snippet.
  pub text: String,
  /// When it was sent.
  pub time: Micros,
}

/// One conversation the signed-in user can open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
  /// Stable identity.
  pub id: GroupId,
  /// Sidebar kind.
  pub kind: GroupKind,
  /// Display name.
  pub name: String,
  /// Image avatar: the other person in a DM, or a picture set on the space. Absent for Google's placeholders.
  pub avatar_url: Option<String>,
  /// Emoji avatar of a space. When present it is the avatar the web app shows.
  pub emoji: Option<String>,
  /// Other participants of a DM or group DM (never the signed-in user).
  pub member_ids: Vec<String>,
  /// Up to two participant avatars of a group DM, to draw stacked when there is no other avatar.
  #[serde(default)]
  pub member_avatars: Vec<String>,
  /// Sort key: last activity, microseconds since epoch.
  pub sort_time: Micros,
  /// Read cursor, microseconds since epoch.
  pub last_read_time: Micros,
  /// True when there is activity newer than `last_read_time`.
  pub unread: bool,
  /// Pinned in the sidebar.
  pub pinned: bool,
  /// Notifications muted.
  pub muted: bool,
  /// Which messages the user asked Google Chat to notify about here.
  #[serde(default)]
  pub notify: Notify,
  /// Last visible line, when Google Chat sent one.
  pub snippet: Option<Snippet>,
  /// Where the web app's sidebar lists this conversation.
  #[serde(default)]
  pub placement: Placement,
}

/// A conversation's notification setting in Google Chat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Notify {
  /// Every message.
  #[default]
  All,
  /// Messages that start a thread, and mentions.
  MainConversations,
  /// Mentions only.
  Mentions,
  /// No notifications.
  Off,
}

/// Sidebar section of a conversation, as arranged in the web app.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Placement {
  /// The section of its kind: Direct messages, Apps, or Spaces.
  #[default]
  ByKind,
  /// The Meetings section (chats of calendar meetings).
  Meeting,
  /// A section the user created, such as "Shortcut".
  Custom {
    /// Section id.
    id: String,
    /// Section title.
    name: String,
  },
}

/// A person or bot in Google Chat.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
  /// User id.
  pub id: String,
  /// Display name.
  pub name: String,
  /// First name, when Google Chat sent one.
  pub first_name: String,
  /// Email, when visible.
  pub email: String,
  /// Avatar URL, when Google Chat sent one.
  pub avatar_url: Option<String>,
  /// True for a Chat app.
  pub is_bot: bool,
  /// The account no longer exists.
  #[serde(default)]
  pub deleted: bool,
}

/// Inline formatting on `Message::text`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpanKind {
  /// Bold.
  Bold,
  /// Italic.
  Italic,
  /// Strikethrough.
  Strike,
  /// Underline.
  Underline,
  /// Inline code.
  Code,
  /// Fenced code block.
  CodeBlock,
  /// Link with this URL.
  Link(String),
  /// Mention of the user with this id.
  Mention(String),
  /// @all mention.
  MentionAll,
  /// One item of a bulleted list: its line, without a bullet character (the UI draws the bullet).
  ListItem,
}

/// Formatting over a byte range of `Message::text`, always on char boundaries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
  /// Byte range into `Message::text`.
  pub range: Range<usize>,
  /// Formatting kind.
  pub kind: SpanKind,
}

/// File, image, Meet link, or unfurled URL on a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentKind {
  /// Raster or animated image.
  Image,
  /// Video file.
  Video,
  /// Generic file.
  File,
  /// Google Drive link.
  Drive,
  /// Meet link.
  Meet,
  /// Unfurled URL preview.
  LinkPreview,
}

/// File, image, Meet link, or unfurled URL on a message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
  /// Kind, for layout.
  pub kind: AttachmentKind,
  /// File or link title.
  pub name: String,
  /// MIME type.
  pub mime: String,
  /// Where the attachment opens in a browser.
  pub url: String,
  /// Image to show inline. Load it with `Core::fetch`: most of these need the session.
  pub thumb_url: Option<String>,
  /// Pixel width when known.
  pub width: Option<u32>,
  /// Pixel height when known.
  pub height: Option<u32>,
}

/// One emoji reaction on a message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reaction {
  /// Emoji characters.
  pub emoji: String,
  /// How many people used it.
  pub count: u32,
  /// True when the signed-in user is among them.
  pub mine: bool,
}

/// Quoted parent of a message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
  /// Original sender display name.
  pub sender_name: String,
  /// Quoted text.
  pub text: String,
}

/// App card, its widgets reduced to text lines and buttons per section.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
  /// The id the app gave the card (the attachment id); a button click names it.
  pub id: String,
  /// Card title.
  pub title: String,
  /// Card subtitle.
  pub subtitle: String,
  /// Sections in order; Google Chat draws a divider between them.
  pub sections: Vec<CardSection>,
}

/// One section of a card.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardSection {
  /// Section header, when it has one.
  pub header: String,
  /// Text lines.
  pub lines: Vec<String>,
  /// Buttons, in order.
  pub buttons: Vec<CardButton>,
}

/// A card button.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardButton {
  /// Label.
  pub label: String,
  /// The link it opens.
  pub url: Option<String>,
  /// What it sends back to its app, for a button that calls the app instead of opening a link.
  pub action: Option<CardAction>,
}

/// A card button's form action, as the card carries it: sent back unchanged in `Command::ClickCard`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardAction {
  /// Method name; for HTTP apps, the URL the app receives events on.
  pub method: String,
  /// Key and value pairs the app attached.
  pub parameters: Vec<(String, String)>,
  /// Load indicator (0 spinner, 1 none).
  pub load_indicator: i32,
  /// Keep form values after the action.
  pub persist_values: bool,
  /// Interaction (0 unspecified, 1 open dialog).
  pub interaction: i32,
  /// Every widget must have a value.
  pub all_widgets_are_required: bool,
}

impl Card {
  /// Whether the card has nothing to show.
  pub fn is_empty(&self) -> bool {
    self.title.is_empty()
      && self.subtitle.is_empty()
      && self
        .sections
        .iter()
        .all(|section| section.header.is_empty() && section.lines.is_empty() && section.buttons.is_empty())
  }
}

/// One Chat message, already resolved for the UI.
#[allow(
  clippy::struct_excessive_bools,
  reason = "flags come independently from the wire"
)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
  /// Message id. Local while `pending`.
  pub id: String,
  /// Topic id: the first message of the thread, or this message if it started one.
  pub topic_id: String,
  /// Conversation this belongs to.
  pub group: GroupId,
  /// Sender user id.
  pub sender_id: String,
  /// Sender display name.
  pub sender_name: String,
  /// Sender avatar URL.
  pub sender_avatar: Option<String>,
  /// Plain text body.
  pub text: String,
  /// Formatting ranges into `text`.
  pub spans: Vec<Span>,
  /// When it was sent.
  pub time: Micros,
  /// When the server last changed it; zero when unknown. Quoting a message refers to this.
  #[serde(default)]
  pub update_time: Micros,
  /// True after the sender edited it.
  pub edited: bool,
  /// True for a system/event message.
  pub system: bool,
  /// Emoji reactions.
  pub reactions: Vec<Reaction>,
  /// Files, images, Meet links, previews.
  pub attachments: Vec<Attachment>,
  /// App cards flattened to text.
  pub cards: Vec<Card>,
  /// Quoted parent, when this message quotes one.
  pub quote: Option<Quote>,
  /// The signed-in user starred it.
  #[serde(default)]
  pub starred: bool,
  /// Set while a sent message waits for the server; `id` is the local id until then.
  pub pending: bool,
  /// True when sending failed.
  pub failed: bool,
}

impl Message {
  /// True for a reply inside a thread: it follows the message that started the topic, whose id is the topic id.
  pub fn is_thread_reply(&self) -> bool {
    !self.topic_id.is_empty() && self.id != self.topic_id
  }
}

/// Realtime stream state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connection {
  /// Opening or reconnecting.
  Connecting,
  /// Stream is up.
  Online,
  /// Stream is down.
  Offline,
}

/// How the MCP server for AI agents runs: on loopback at `port`, for requests that carry `token`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpConfig {
  /// Loopback port.
  pub port: u16,
  /// Bearer (or path) secret.
  pub token: String,
  /// Offer the tool that sends messages. Without it agents can only read.
  pub allow_send: bool,
}

/// How another person shows right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonStatus {
  /// At the computer.
  Active,
  /// Idle or away.
  Away,
  /// Do not disturb.
  DoNotDisturb,
}

/// How the signed-in user shows, as Google Chat has it: do-not-disturb, and a custom status.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MyStatus {
  /// Do not disturb is on.
  pub dnd: bool,
  /// When do-not-disturb ends, in microseconds. None means it stays until turned off.
  pub dnd_until: Option<i64>,
  /// Custom status emoji.
  pub emoji: String,
  /// Custom status text.
  pub text: String,
  /// When the custom status clears. None means it stays until cleared.
  pub status_until: Option<i64>,
}

impl MyStatus {
  /// Whether do-not-disturb still covers this moment. An expiry in the past has already ended.
  pub fn dnd_on(&self, now: i64) -> bool {
    self.dnd && self.dnd_until.is_none_or(|until| until > now)
  }

  /// The custom status, when one is set and has not cleared.
  pub fn custom(&self, now: i64) -> Option<(&str, &str)> {
    if self.text.is_empty() && self.emoji.is_empty() {
      return None;
    }
    if self.status_until.is_some_and(|until| until <= now) {
      return None;
    }
    Some((self.emoji.as_str(), self.text.as_str()))
  }
}

/// How long a do-not-disturb from the status button lasts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DndFor {
  /// Turn do-not-disturb off.
  Off,
  /// Microseconds from now.
  For(i64),
}

/// How the user shows to other people.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
  /// Online: at the computer.
  Active,
  /// Signed in, but not at the computer: Google Chat shows away after a while.
  Idle,
  /// Shown as away no matter what.
  Away,
}

/// One do-not-disturb schedule of Google Chat: notifications are muted from `start` to `end` (minutes of the
/// day; an `end` at or before `start` runs into the next day) on the days it starts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DndSchedule {
  /// 1 is Monday, 7 is Sunday.
  pub days: Vec<u8>,
  /// Start minute of the day.
  pub start: u16,
  /// End minute of the day.
  pub end: u16,
  /// Whether this row is on.
  pub enabled: bool,
}

/// The do-not-disturb schedules as Google Chat keeps them for the account.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DndSchedules {
  /// The schedules follow the time zone of the device in use, rather than `time_zone`.
  pub match_time_zone: bool,
  /// IANA time zone when `match_time_zone` is false.
  pub time_zone: String,
  /// Schedule rows.
  pub schedules: Vec<DndSchedule>,
}

/// Whether the MCP server is listening.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum McpStatus {
  /// Not running.
  #[default]
  Off,
  /// Listening on this loopback port.
  Running {
    /// Bound port.
    port: u16,
  },
  /// It could not start (the port is taken, usually).
  Failed(String),
}

/// One message found by `Command::Search`, or listed by a shortcut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
  /// Conversation that holds the hit.
  pub group: GroupId,
  /// Message id.
  pub message_id: String,
  /// Topic id of that message.
  pub topic_id: String,
  /// Sender user id.
  pub sender_id: String,
  /// Sender display name.
  pub sender_name: String,
  /// Sender avatar URL.
  pub sender_avatar: Option<String>,
  /// Matching text.
  pub text: String,
  /// When it was sent.
  pub time: Micros,
}

/// A sidebar shortcut that lists messages from every conversation, as Google Chat's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shortcut {
  /// Messages that @mention the user.
  Mentions,
  /// Messages the user starred.
  Starred,
}

/// Everything the core tells the UI. Lists are complete replacements unless stated otherwise.
#[derive(Clone, Debug)]
pub enum Event {
  /// Who is signed in. Sent once, from cache when available.
  Me(User),
  /// Stream connectivity.
  Connection(Connection),
  /// The full conversation list, unsorted.
  World(Arc<Vec<Group>>),
  /// One conversation changed or appeared.
  GroupUpsert(Group),
  /// A conversation's messages, oldest first. `prepended` says how many of them are new at the top
  /// (older history), so the view can hold its scroll position.
  History {
    /// Conversation these messages belong to.
    group: GroupId,
    /// Oldest first.
    messages: Arc<Vec<Arc<Message>>>,
    /// True when there is no older page.
    reached_start: bool,
    /// False while the list is a window into the past (after `Command::Jump`): newer messages exist and
    /// `Command::LoadNewer` fetches them.
    reached_end: bool,
    /// How many messages at the front are newly loaded older history.
    prepended: usize,
    /// Set by `Command::Jump`: the message to scroll to and highlight.
    focus: Option<String>,
  },
  /// Answer to `Command::Search`, best matches first.
  SearchResults {
    /// Search string that produced these hits.
    query: String,
    /// Best matches first.
    hits: Arc<Vec<SearchHit>>,
  },
  /// One page of a shortcut's messages, answering `Command::LoadShortcut`. Newest first.
  Shortcut {
    /// Which shortcut.
    shortcut: Shortcut,
    /// The messages of this page.
    hits: Arc<Vec<SearchHit>>,
    /// Whether this page follows the ones already shown (it answers a `cursor`), rather than replacing them.
    appended: bool,
    /// Asks for the next, older page when there is one.
    cursor: Option<String>,
  },
  /// The meeting asked for with `Command::StartMeet` was created and posted; this is its link.
  MeetStarted(String),
  /// A message arrived or changed. `replaces` names the local id of the pending message it confirms.
  /// `incoming` is true once per message, when someone else's message is first seen: worth a notification.
  MessageUpsert {
    /// Current message.
    message: Arc<Message>,
    /// Local pending id this confirms, when it does.
    replaces: Option<String>,
    /// True once, when someone else's message is first seen.
    incoming: bool,
  },
  /// A message was deleted.
  MessageDeleted {
    /// Conversation.
    group: GroupId,
    /// Deleted message id.
    id: String,
  },
  /// Typing indicator.
  Typing {
    /// Conversation.
    group: GroupId,
    /// Who is typing.
    user_id: String,
    /// Their display name.
    user_name: String,
    /// True when they started, false when they stopped.
    typing: bool,
  },
  /// The session is gone or was refused: the cookies need to be refreshed.
  SignedOut(String),
  /// Answer to `Command::SetMcp`.
  Mcp(McpStatus),
  /// The do-not-disturb schedules: answer to `Command::LoadDnd` and `Command::SaveDnd`.
  Dnd(DndSchedules),
  /// How the people of the one-to-one conversations show, by user id. Replaces what came before.
  Statuses(Vec<(String, PersonStatus)>),
  /// How the signed-in user shows. Replaces what came before.
  Mine(MyStatus),
  /// Directory of people the user can start a chat with. Replaces what came before.
  People(Vec<User>),
  /// Open this conversation: answer to `Command::CreateChat`, or an existing chat it found.
  OpenChat(GroupId),
  /// A command or stream step failed. The string is already safe to show.
  Error(String),
}

/// Everything the UI asks the core to do.
#[derive(Clone, Debug)]
pub enum Command {
  /// Show a conversation: answers `History` at once from memory or disk, then again when fresh.
  /// Also makes it the conversation whose incoming messages count as read.
  Open(GroupId),
  /// No conversation is on screen (Home).
  Close,
  /// Fetch the page before the oldest message held.
  LoadOlder(GroupId),
  /// Fetch the page after the newest message held, when `History::reached_end` is false.
  LoadNewer(GroupId),
  /// Search messages across all conversations. Answers `SearchResults`.
  Search(String),
  /// Fetch a shortcut's messages: the newest page without `cursor`, the next older page with one.
  /// Answers `Event::Shortcut`.
  LoadShortcut {
    /// Which shortcut.
    shortcut: Shortcut,
    /// `Event::Shortcut::cursor` of the last page shown, for the page after it.
    cursor: Option<String>,
  },
  /// Show the conversation around one message (a search hit): answers `History` with that window and `focus`.
  /// `Open` on the same conversation returns to the latest messages.
  Jump {
    /// Conversation to open.
    group: GroupId,
    /// Message to focus.
    message_id: String,
    /// Sort time of that message.
    time: Micros,
  },
  /// Sends a message. With `topic_id`, it is a reply in that thread; without, a new top-level message, which
  /// can quote the message with id `quote`. `files` are uploaded first and go out attached to it.
  Send {
    /// Destination conversation.
    group: GroupId,
    /// Thread to reply in, when this is a reply.
    topic_id: Option<String>,
    /// Message body.
    text: String,
    /// Message id to quote, when quoting.
    quote: Option<String>,
    /// Local files to upload and attach.
    files: Vec<PathBuf>,
  },
  /// Replaces the text of one of the user's own messages.
  Edit {
    /// Conversation.
    group: GroupId,
    /// Message to replace.
    message_id: String,
    /// New body.
    text: String,
  },
  /// Deletes one of the user's own messages.
  Delete {
    /// Conversation.
    group: GroupId,
    /// Message to delete.
    message_id: String,
  },
  /// Creates a Google Meet meeting and posts its link in the conversation, as the web app's call button does.
  StartMeet(GroupId),
  /// Mark this conversation read.
  MarkRead(GroupId),
  /// Press a card button that calls its app back. The app answers by updating the message or posting a new one.
  ClickCard {
    /// Conversation.
    group: GroupId,
    /// Topic the message belongs to.
    topic_id: String,
    /// Message that holds the card.
    message_id: String,
    /// `Card::id`.
    card_id: String,
    /// The button's action.
    action: CardAction,
  },
  /// Star or unstar a message, which lists it in the Starred shortcut.
  Star {
    /// Conversation.
    group: GroupId,
    /// Topic the message belongs to.
    topic_id: String,
    /// Message to star.
    message_id: String,
    /// True to star, false to unstar.
    starred: bool,
  },
  /// Add or remove an emoji reaction.
  React {
    /// Conversation.
    group: GroupId,
    /// Topic the message belongs to.
    topic_id: String,
    /// Message to react to.
    message_id: String,
    /// Emoji characters.
    emoji: String,
    /// True to add, false to remove.
    add: bool,
  },
  /// Typing notice for the open conversation.
  SetTyping {
    /// Conversation.
    group: GroupId,
    /// True while the composer has text being typed.
    typing: bool,
  },
  /// Starts the MCP server, restarts it with a new configuration, or stops it (`None`).
  SetMcp(Option<McpConfig>),
  /// How the user shows to others from now on. The core keeps telling the server until the next one.
  SetPresence(Presence),
  /// Reads the do-not-disturb schedules from Google Chat.
  LoadDnd,
  /// Replaces the do-not-disturb schedules in Google Chat.
  SaveDnd(DndSchedules),
  /// Turns do-not-disturb on for a span, or off.
  SetDnd(DndFor),
  /// Sets the custom status. Empty emoji and text clears it. `until` is an absolute expiry.
  SetStatus {
    /// Status emoji. Empty with empty text clears it.
    emoji: String,
    /// Status text.
    text: String,
    /// Absolute expiry in microseconds. None stays until cleared.
    until: Option<i64>,
  },
  /// Reload the conversation list from the server.
  RefreshWorld,
  /// Open a chat with these people (not the signed-in user): one person is a DM, two or more a group DM.
  /// Opens an existing conversation with that set when there is one.
  CreateChat {
    /// Other participants.
    user_ids: Vec<String>,
  },
  /// The session file was written after sign-in. Load it and continue.
  Connect,
}

#[cfg(test)]
mod tests {
  use super::{GroupId, Message, MyStatus};

  fn message(id: &str, topic: &str) -> Message {
    Message {
      id: id.into(),
      topic_id: topic.into(),
      group: GroupId::Dm("g".into()),
      sender_id: "u".into(),
      sender_name: "U".into(),
      sender_avatar: None,
      text: String::new(),
      spans: Vec::new(),
      time: 0,
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
    }
  }

  #[test]
  fn group_id_key_is_a_file_name() {
    assert_eq!(GroupId::Dm("abc".into()).key(), "dm_abc");
    assert_eq!(GroupId::Space("xyz".into()).key(), "space_xyz");
  }

  #[test]
  fn thread_reply_is_not_the_starter() {
    assert!(!message("t1", "t1").is_thread_reply());
    assert!(!message("t1", "").is_thread_reply());
    assert!(message("r1", "t1").is_thread_reply());
  }

  #[test]
  fn dnd_covers_until_expiry() {
    let on = MyStatus {
      dnd: true,
      dnd_until: Some(10),
      ..MyStatus::default()
    };
    assert!(on.dnd_on(9));
    assert!(!on.dnd_on(10));
    let sticky = MyStatus {
      dnd: true,
      dnd_until: None,
      ..MyStatus::default()
    };
    assert!(sticky.dnd_on(99));
    assert!(!MyStatus::default().dnd_on(1));
  }

  #[test]
  fn custom_status_clears_when_empty_or_expired() {
    let set = MyStatus {
      emoji: "🙂".into(),
      text: "brb".into(),
      status_until: Some(10),
      ..MyStatus::default()
    };
    assert_eq!(set.custom(9), Some(("🙂", "brb")));
    assert_eq!(set.custom(10), None);
    assert_eq!(MyStatus::default().custom(1), None);
  }
}
