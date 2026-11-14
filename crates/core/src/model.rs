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

/// A Chat app command the composer offers after `/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashCommand {
  /// The app's user id.
  pub bot_id: String,
  /// The app's display name.
  pub bot_name: String,
  /// The app's avatar.
  pub bot_avatar: Option<String>,
  /// The id the app registered the command under.
  pub id: i64,
  /// The name with its slash, such as `/help`.
  pub name: String,
  /// What the command does.
  pub description: String,
}

/// A part of an outgoing message's text that stands for someone or something, as one unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
  /// Byte range of the message text, on char boundaries.
  pub range: Range<usize>,
  /// What it stands for.
  pub kind: ChipKind,
}

/// What a `Chip` stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChipKind {
  /// @mention of one person or app.
  Mention {
    /// User id.
    user_id: String,
    /// True for a Chat app.
    bot: bool,
  },
  /// @all: everyone in the space.
  MentionAll,
  /// A slash command, which the app answers. It starts the message.
  Command {
    /// The app's user id.
    bot_id: String,
    /// `SlashCommand::id`.
    command_id: i64,
    /// `SlashCommand::name`.
    name: String,
  },
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

/// How far one other person has read a conversation, as Google Chat shows it: their avatar sits beside the last
/// message they read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadReceipt {
  /// User id.
  pub user_id: String,
  /// Display name; empty when no profile is known for them.
  pub name: String,
  /// Avatar URL, when Google Chat sent one.
  pub avatar_url: Option<String>,
  /// They have read every message sent at or before this time, microseconds since epoch.
  pub read_time: Micros,
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

/// What the signed-in user may do with a calendar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalendarAccess {
  /// The user's own calendar.
  Owner,
  /// Shared with the right to change events.
  Writer,
  /// Shared to read, such as a holidays calendar.
  Reader,
  /// Only free and busy times are shown.
  FreeBusy,
}

impl CalendarAccess {
  /// Whether events can be created, changed, and deleted in a calendar with this access.
  pub const fn can_write(self) -> bool {
    matches!(self, Self::Owner | Self::Writer)
  }
}

/// One calendar of the account, as Google Calendar lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Calendar {
  /// Calendar id; the primary calendar's is the account's email.
  pub id: String,
  /// Name, with the user's own name for it when they gave one.
  pub name: String,
  /// Color as `0xRRGGBB`, as the web app paints it.
  pub color: u32,
  /// What the user may do with it.
  pub access: CalendarAccess,
  /// The account's own calendar.
  pub primary: bool,
  /// Checked in Google Calendar's list, so its events show until the user hides them.
  pub selected: bool,
}

/// A guest's answer to an invitation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rsvp {
  /// Not answered yet.
  NeedsAction,
  /// Going.
  Accepted,
  /// Maybe.
  Tentative,
  /// Not going.
  Declined,
}

/// Someone invited to an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guest {
  /// Email address.
  pub email: String,
  /// Display name, empty when Google Calendar has none.
  pub name: String,
  /// Their answer.
  pub rsvp: Rsvp,
  /// They organize the event.
  pub organizer: bool,
  /// This is the signed-in user.
  pub me: bool,
  /// Invited as optional.
  pub optional: bool,
}

/// What an event is, as Google Calendar types them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EventKind {
  /// A regular event.
  #[default]
  Event,
  /// Out of office: invitations that overlap it are declined.
  OutOfOffice,
  /// Focus time: chat notifications are muted during it.
  FocusTime,
  /// Where the user works that day.
  WorkingLocation,
  /// A contact's birthday.
  Birthday,
}

/// Who can see an event's details.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Visibility {
  /// As the calendar shares its events.
  #[default]
  Default,
  /// Everyone who can see the calendar sees the details.
  Public,
  /// Only the attendees see the details.
  Private,
}

/// The notification before an event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reminder {
  /// The calendar's default notifications.
  #[default]
  Default,
  /// No notification.
  None,
  /// A notification this many minutes before.
  Minutes(u32),
}

/// One occurrence of a calendar event. Recurring events come one per occurrence.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
  clippy::struct_excessive_bools,
  reason = "each flag is one fact Google Calendar reports about the event"
)]
pub struct CalendarEvent {
  /// Event id; an occurrence of a recurring event has its own.
  pub id: String,
  /// Calendar it belongs to.
  pub calendar_id: String,
  /// Title, `(No title)` when it has none.
  pub title: String,
  /// Start. An all-day event starts at local midnight of its first day.
  pub start: Micros,
  /// End, exclusive. An all-day event ends at local midnight after its last day.
  pub end: Micros,
  /// Spans whole days, without times.
  pub all_day: bool,
  /// Location, as typed.
  pub location: String,
  /// Description as plain text.
  pub description: String,
  /// Google Meet link, when it has one.
  pub meet_url: Option<String>,
  /// The event in Google Calendar on the web.
  pub link: String,
  /// Its own color, as `0xRRGGBB`, when one was picked; otherwise it takes its calendar's.
  pub color: Option<u32>,
  /// Google Calendar's id of that color, 1 to 11.
  pub color_id: Option<u8>,
  /// Who is invited, organizer included. Empty for an event without guests.
  pub guests: Vec<Guest>,
  /// Name or email of the organizer.
  pub organizer: String,
  /// Id of the recurring event this is an occurrence of.
  pub series: Option<String>,
  /// What it is.
  pub kind: EventKind,
  /// Shows the user as busy.
  pub busy: bool,
  /// Who can see its details.
  pub visibility: Visibility,
  /// The notification before it.
  pub reminder: Reminder,
  /// The user may change it.
  pub can_edit: bool,
  /// The user may delete it from their calendar.
  pub can_delete: bool,
  /// The user's own answer, when they are a guest and not the organizer.
  pub my_rsvp: Option<Rsvp>,
}

/// How a new event repeats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
  /// Once.
  #[default]
  Never,
  /// Every day.
  Daily,
  /// Every week on the start's weekday.
  Weekly,
  /// Every month on the start's day of the month.
  Monthly,
  /// Every year on the start's date.
  Yearly,
  /// Monday to Friday.
  Weekdays,
}

/// Which occurrences of a recurring event a change or a deletion applies to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditScope {
  /// Only the occurrence picked.
  #[default]
  This,
  /// Every occurrence.
  All,
}

/// A new event, or the changes to one, for `Command::SaveEvent`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[expect(
  clippy::struct_excessive_bools,
  reason = "each flag is one switch of the event editor"
)]
pub struct EventDraft {
  /// Calendar to keep it in. Moves an edited event when it is not the one it is in.
  pub calendar_id: String,
  /// Calendar and id of the event edited. `None` creates an event.
  pub editing: Option<(String, String)>,
  /// For an occurrence of a recurring event: whether the change is for it alone or for the whole series.
  pub scope: EditScope,
  /// What it is. Only a new event takes it; Google Calendar keeps an event's kind.
  pub kind: EventKind,
  /// Title.
  pub title: String,
  /// Start; local midnight of the first day for an all-day event.
  pub start: Micros,
  /// End, exclusive; local midnight after the last day for an all-day event.
  pub end: Micros,
  /// Spans whole days.
  pub all_day: bool,
  /// Location.
  pub location: String,
  /// Description as plain text.
  pub description: String,
  /// Guests' emails. Guests already invited keep their answers.
  pub guests: Vec<String>,
  /// Give it a Google Meet link, when it has none.
  pub add_meet: bool,
  /// How it repeats. `None` leaves an edited event's recurrence as it is.
  pub repeat: Option<Repeat>,
  /// Shows the user as busy.
  pub busy: bool,
  /// Who can see its details.
  pub visibility: Visibility,
  /// Google Calendar's color id, 1 to 11. `None` takes the calendar's color.
  pub color_id: Option<u8>,
  /// The notification before it.
  pub reminder: Reminder,
  /// Email the guests about it.
  pub notify: bool,
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
  /// Who has read a conversation, never the signed-in user. Sent when the conversation's messages load (and when
  /// `Command::Open` or `Command::Jump` shows the ones held), just before their `History`, and again whenever the
  /// stream reports that someone read on. Replaces what came before for that conversation.
  ReadReceipts {
    /// Conversation.
    group: GroupId,
    /// False when Google Chat has read receipts off in this conversation: `receipts` is then empty.
    enabled: bool,
    /// Earliest reader first.
    receipts: Arc<Vec<ReadReceipt>>,
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
  /// Answer to `Command::CreateMeeting`: the new meeting's link (`https://meet.google.com/<code>`), or why it could
  /// not be made, already safe to show.
  MeetingCreated(Result<String, String>),
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
  /// Who can be @mentioned in a conversation, by name: answer to `Command::LoadMembers`. Never the signed-in user.
  Members {
    /// Conversation.
    group: GroupId,
    /// Joined members, people and apps.
    users: Arc<Vec<User>>,
  },
  /// The commands of the apps in a conversation, by name: answer to `Command::LoadSlashCommands`.
  SlashCommands {
    /// Conversation.
    group: GroupId,
    /// Commands that answer in the conversation. Commands that open a dialog are left out.
    commands: Arc<Vec<SlashCommand>>,
  },
  /// Open this conversation: answer to `Command::CreateChat`, or an existing chat it found.
  OpenChat(GroupId),
  /// The account's calendars, primary first: answer to `Command::LoadCalendars`. Replaces what came before.
  Calendars(Arc<Vec<Calendar>>),
  /// Every calendar's events that overlap `start..end`, by start: answer to `Command::LoadEvents`, and again after a
  /// write changed them.
  CalendarEvents {
    /// Start of the range asked for.
    start: Micros,
    /// End of the range asked for.
    end: Micros,
    /// Events by start, all-day ones first on a day.
    events: Arc<Vec<CalendarEvent>>,
  },
  /// A calendar write (save, delete, or answer) finished: `Ok` once Google Calendar has it, or why it failed, already
  /// safe to show. The calendar asked for last follows a success.
  CalendarWritten(Result<(), String>),
  /// Google Calendar could not be read. Safe to show.
  CalendarFailed(String),
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
    /// Mentions and the slash command in `text`.
    chips: Vec<Chip>,
  },
  /// Fetch who can be @mentioned in a conversation. Answers `Event::Members`.
  LoadMembers(GroupId),
  /// Fetch the slash commands of the apps in a conversation. Answers `Event::SlashCommands`.
  LoadSlashCommands(GroupId),
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
  /// Creates a Google Meet meeting of its own, in no conversation and on no calendar, as Meet's "New" does. Answers
  /// `Event::MeetingCreated`.
  CreateMeeting,
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
  /// Fetch the account's calendars. Answers `Event::Calendars`.
  LoadCalendars,
  /// Fetch the events of every calendar that overlap `start..end`. Answers `Event::CalendarEvents`; the latest range
  /// asked for is fetched again after each write.
  LoadEvents {
    /// Start of the range.
    start: Micros,
    /// End of the range, exclusive.
    end: Micros,
  },
  /// Creates an event, or changes one. Answers `Event::CalendarWritten`.
  SaveEvent(Box<EventDraft>),
  /// Deletes an event, or every occurrence of its series. Answers `Event::CalendarWritten`.
  DeleteEvent {
    /// Calendar holding it.
    calendar_id: String,
    /// The event, or the occurrence picked.
    event_id: String,
    /// For an occurrence: it alone, or the whole series.
    scope: EditScope,
    /// Email the guests about it.
    notify: bool,
  },
  /// Answers an invitation. Answers `Event::CalendarWritten`.
  RespondEvent {
    /// Calendar holding it.
    calendar_id: String,
    /// The event, or the occurrence picked.
    event_id: String,
    /// For an occurrence: it alone, or the whole series.
    scope: EditScope,
    /// The answer.
    rsvp: Rsvp,
  },
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
