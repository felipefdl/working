//! The one entity holding everything the core tells us, plus the precomputed lists the views render.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use gchat_core::fuzzy::Fuzzy;
use gchat_core::{
  AttachmentKind, Calendar, CalendarEvent, Command, Connection, Core, Event, Group, GroupId, GroupKind, MailCompose,
  MailLabel, MailMessage, MailReply, MailSignature, MailThread, Mailbox, Message, Micros, Notify, Placement,
  ReadReceipt, SearchHit, Shortcut, Snippet, User,
};
use gpui_kit::*;

use crate::conversation::ConversationView;
use crate::images::Images;
use crate::mail_lists::{MailLists, mailbox_kind};
use crate::settings::{Features, Layout, ListWindow, Settings, Sort};

/// Sidebar sections, laid out as the Google Chat web app does.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Section {
  /// Holds Home, Mentions, and Starred.
  Shortcuts,
  /// A section the user made in Google Chat, such as "Shortcut".
  Custom {
    id: String,
    name: String,
  },
  Dms,
  Apps,
  Spaces,
  Meetings,
}

impl Section {
  pub fn title(&self) -> &str {
    match self {
      Self::Shortcuts => "Shortcuts",
      Self::Custom { name, .. } => name,
      Self::Dms => "Direct messages",
      Self::Apps => "Apps",
      Self::Spaces => "Spaces",
      Self::Meetings => "Meetings",
    }
  }

  /// Where the sidebar lists a conversation.
  pub fn of(group: &Group) -> Self {
    match &group.placement {
      Placement::Custom { id, name } => Self::Custom { id: id.clone(), name: name.clone() },
      Placement::Meeting => Self::Meetings,
      Placement::ByKind => Self::of_kind(group.kind),
    }
  }

  /// The section of a kind of conversation, where one goes when no other section holds it.
  pub const fn of_kind(kind: GroupKind) -> Self {
    match kind {
      GroupKind::Dm | GroupKind::GroupDm => Self::Dms,
      GroupKind::App => Self::Apps,
      GroupKind::Space => Self::Spaces,
    }
  }

  /// Where `Settings::section_sort` keeps this section's order.
  pub fn key(&self) -> String {
    match self {
      Self::Shortcuts => "shortcuts".into(),
      Self::Custom { id, .. } => format!("custom:{id}"),
      Self::Dms => "dms".into(),
      Self::Apps => "apps".into(),
      Self::Spaces => "spaces".into(),
      Self::Meetings => "meetings".into(),
    }
  }

  /// Unpinned rows go A to Z in people sections and newest first elsewhere, as the web app lists them, until the
  /// user picks another order.
  pub const fn default_sort(&self) -> Sort {
    if matches!(self, Self::Custom { .. } | Self::Dms) {
      Sort::Alphabetical
    } else {
      Sort::Recent
    }
  }
}

/// A group with what its rows draw worked out once: the lowercase name for filtering, the sidebar section, the name,
/// and the preview line.
pub struct Conv {
  pub g: Group,
  pub lower: String,
  /// Where the sidebar lists it: `Section::of(&g)`.
  pub section: Section,
  /// `g.name`, shared by every row that draws it.
  pub name: SharedString,
  /// The last message on one line, led by "You" or its sender's name. Empty without one.
  pub preview: SharedString,
}

/// The people the user can start a chat with, name order. Each is shared, so lists of them hold references.
#[derive(Default)]
pub struct Directory(Vec<Arc<User>>);

impl Directory {
  /// Every person, in order.
  pub fn iter(&self) -> impl Iterator<Item = &User> {
    self.0.iter().map(AsRef::as_ref)
  }

  /// Every person, in order, as handles a list can keep.
  pub fn shared(&self) -> impl Iterator<Item = &Arc<User>> {
    self.0.iter()
  }

  /// How many people there are.
  pub const fn count(&self) -> usize {
    self.0.len()
  }
}

#[derive(Clone)]
pub enum SidebarItem {
  Home,
  Shortcut(Shortcut),
  /// Google Calendar, under the shortcuts.
  Calendar,
  /// Google Meet, under Calendar.
  Meet,
  /// Gmail, under Meet.
  Mail,
  Header(Section),
  Conv(Arc<Conv>),
  Toggle(Section),
}

/// What the Home chips, the Messages chips, and the Rail select: the conversations listed. The Unread filter
/// (`Store::unread_only`) narrows whichever is picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pane {
  All,
  /// People, group chats, spaces, or apps, wherever the sidebar places them.
  Kind(Kind),
  Section(Section),
  /// Mentions or Starred: the list shows the shortcut's messages (`Store::shortcut`) instead of conversations.
  Shortcut(Shortcut),
}

impl Pane {
  /// Whether `layout` can pick the pane: All and a kind anywhere; a section, Mentions, and Starred only in the Rail.
  pub const fn fits(&self, layout: Layout) -> bool {
    match self {
      Self::All | Self::Kind(_) => true,
      Self::Section(_) | Self::Shortcut(_) => matches!(layout, Layout::Rail),
    }
  }
}

/// What a conversation is, for the kind filters of Home, the Messages chips, and the Rail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
  /// One-to-one chats.
  People,
  /// Group chats without a space.
  Groups,
  Spaces,
  Apps,
}

impl Kind {
  pub const ALL: [Self; 4] = [Self::People, Self::Groups, Self::Spaces, Self::Apps];

  pub const fn of(kind: GroupKind) -> Self {
    match kind {
      GroupKind::Dm => Self::People,
      GroupKind::GroupDm => Self::Groups,
      GroupKind::Space => Self::Spaces,
      GroupKind::App => Self::Apps,
    }
  }

  pub const fn title(self) -> &'static str {
    match self {
      Self::People => "People",
      Self::Groups => "Groups",
      Self::Spaces => "Spaces",
      Self::Apps => "Apps",
    }
  }

  /// Where the kind sits in the IRC buffer list: spaces, as channels, first, then group chats, people, and apps.
  const fn buffer_rank(self) -> u8 {
    match self {
      Self::Spaces => 0,
      Self::Groups => 1,
      Self::People => 2,
      Self::Apps => 3,
    }
  }
}

/// One Rail entry: a pane and how many of its conversations show as unread (`shows_unread`).
#[derive(Clone)]
pub struct Tab {
  pub pane: Pane,
  pub unread: usize,
}

/// Steps Back can go, at most.
const HISTORY_LIMIT: usize = 50;

/// How a mail notification's key starts: `mail/<thread id>`. A click on it opens Mail on that thread (`open_key`).
const MAIL_KEY: &str = "mail/";

/// How an event reminder's key starts: `event/<calendar id>/<event id>`. A click opens Calendar on it (`open_key`).
const EVENT_KEY: &str = "event/";

/// How many reminders' events a click can still find.
const REMINDED_LIMIT: usize = 20;

#[derive(Clone, PartialEq, Eq)]
pub enum View {
  Home,
  /// Mentions or Starred: a message list across conversations.
  Shortcut(Shortcut),
  /// Google Calendar.
  Calendar,
  /// Google Meet.
  Meet,
  /// Gmail.
  Mail,
  Conversation(GroupId),
}

impl View {
  /// The conversation on screen, when the view is one.
  pub const fn conversation(&self) -> Option<&GroupId> {
    match self {
      Self::Conversation(id) => Some(id),
      Self::Home | Self::Shortcut(_) | Self::Calendar | Self::Meet | Self::Mail => None,
    }
  }
}

/// Google Calendar as the core last answered it, for the Calendar view.
#[derive(Default)]
pub struct CalendarData {
  pub calendars: Arc<Vec<Calendar>>,
  /// The events of `range`, once `loaded`; until then, those of the range before it.
  pub events: Arc<Vec<CalendarEvent>>,
  /// The time last asked for.
  pub range: Option<(Micros, Micros)>,
  /// `events` answers `range`.
  pub loaded: bool,
  /// Why Google Calendar could not be read, until a read works.
  pub error: Option<String>,
  /// A save, a deletion, or an answer is on its way.
  pub writing: bool,
  /// How the last write went, until the view takes it.
  written: Option<Result<(), String>>,
  /// An event whose reminder was clicked, until the view shows its card (`take_reveal`).
  reveal: Option<CalendarEvent>,
  /// The events of the reminders posted, by notification key, newest last: what a click on one shows.
  reminded: Vec<(String, CalendarEvent)>,
}

/// What a meeting being made is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Creating {
  /// A link to keep: the Meet view shows it in a card.
  ForLater,
  /// A meeting to join at once: its call opens when the link arrives.
  Instant,
}

/// How far a call started from the Calls tab has got.
enum CallStage {
  /// The chat with this person, by user id, is being made.
  Chat(String),
  /// The meeting is being posted in this chat.
  Meeting(GroupId),
}

/// A call being started from the Calls tab of the Meet view.
pub struct Calling {
  /// Who is being called.
  pub name: String,
  stage: CallStage,
}

impl Calling {
  /// A call to `name`, whose user id is `person`: in `chat` when they have one, else in the chat made for the call.
  fn new(name: String, person: String, chat: Option<GroupId>) -> Self {
    let stage = chat.map_or_else(|| CallStage::Chat(person), CallStage::Meeting);
    Self { name, stage }
  }

  /// Takes the chat the core opened (`OpenChat`) when it is the one made for this call, a chat with that person
  /// alone, and answers whether it was. The meeting is posted in it from then on.
  fn take_chat(&mut self, chat: &GroupId, members: &[String]) -> bool {
    let CallStage::Chat(person) = &self.stage else {
      return false;
    };
    if members != [person.as_str()] {
      return false;
    }
    self.stage = CallStage::Meeting(chat.clone());
    true
  }

  /// Whether a message of `group` says the meeting is posted: the call is in that chat, and the message has a meeting
  /// card.
  fn is_posted(&self, group: &GroupId, has_card: bool) -> bool {
    has_card && matches!(&self.stage, CallStage::Meeting(chat) if chat == group)
  }
}

/// What the Meet view holds: the events of the day it shows, the meeting being made, and the call being started.
#[derive(Default)]
pub struct MeetData {
  /// The time last asked for: the day on screen. It is kept apart from the Calendar's range, so each view's answers
  /// are told by its own range and neither takes the other's.
  pub range: Option<(Micros, Micros)>,
  /// The events of `range`, once `loaded`; until then, those of the range before it.
  pub events: Arc<Vec<CalendarEvent>>,
  /// `events` answers `range`.
  pub loaded: bool,
  /// A meeting is being made, and what for, until the core answers.
  pub creating: Option<Creating>,
  /// A call is being started, until its meeting is posted or the core says why not.
  pub calling: Option<Calling>,
  /// Why the last meeting or call could not be made, until the next try.
  pub error: Option<String>,
  /// The link of a meeting made for later, until the view takes it.
  made: Option<String>,
}

/// A write sent to Gmail, by what its answer is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MailWrite {
  /// A thread marked read or unread, archived or put back: a failure shows where Mail shows failures.
  Change,
  /// A reply: its box hears how it went.
  Reply,
  /// A new email: its form hears how it went.
  Compose,
}

/// Which send `MailWritten` answered, so the reply box and the new-email form each take their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailSent {
  /// The reply box.
  Reply,
  /// The new-email form.
  Compose,
}

/// Gmail as the core last answered it, for the Mail view and the Mail buttons, with the writes on their way.
#[derive(Default)]
pub struct MailData {
  /// The threads of each mailbox asked for (`mail_lists.rs`).
  pub lists: MailLists,
  /// The thread open, by Gmail's thread id.
  pub open: Option<String>,
  /// The emails of `open`, oldest first, once the core answers. Those of another thread are never kept here.
  pub messages: Option<Arc<Vec<MailMessage>>>,
  /// The emails of `open` are on their way.
  pub opening: bool,
  /// Unread inbox threads, as the core last counted them.
  pub unread: u32,
  /// Why Gmail could not be read, until a read works.
  pub error: Option<String>,
  /// A reply or a new email is on its way, until the core answers `MailWritten`.
  pub sending: bool,
  /// Why the last send did not go, until the next try: the reply box or the new-email form shows it.
  pub send_error: Option<String>,
  /// Which send went, until the view takes it (`take_sent`).
  sent: Option<MailSent>,
  /// The writes sent and not answered yet, oldest first. The core runs them one at a time in order and answers each
  /// with one `MailWritten`, so an answer is for the first of them.
  writes: VecDeque<MailWrite>,
  /// The user's labels, by name, as the core last listed them.
  pub labels: Arc<Vec<MailLabel>>,
  /// The signature Gmail adds to new emails, when it sets one.
  pub signature_new: Option<MailSignature>,
  /// The signature Gmail adds to answers, when it sets one.
  pub signature_reply: Option<MailSignature>,
  /// The inbox was asked for: the core watches it from then on, and it is not asked for again at startup.
  watching: bool,
}

#[derive(Clone)]
pub struct History {
  pub messages: Arc<Vec<Arc<Message>>>,
  pub reached_start: bool,
  /// False while the list is a window around a search hit rather than the latest messages.
  pub reached_end: bool,
}

/// Who has read the open conversation, as the core last said it.
struct OpenReceipts {
  group: GroupId,
  /// Google Chat has read receipts on in this conversation.
  enabled: bool,
  /// Earliest reader first, never the signed-in user.
  receipts: Arc<Vec<ReadReceipt>>,
}

/// A message search: `hits` is `None` until the core answers.
pub struct Search {
  pub query: String,
  pub hits: Option<Arc<Vec<SearchHit>>>,
}

/// The messages of the shortcut on screen: `hits` is `None` until the core answers the first page.
pub struct ShortcutList {
  pub shortcut: Shortcut,
  pub hits: Option<Vec<SearchHit>>,
  /// Asks for the next, older page; `None` once every page is in.
  cursor: Option<String>,
  loading_more: bool,
}

/// A day in microseconds, the unit of `Group::sort_time`.
const DAY_MICROS: Micros = 86_400_000_000;

/// Whether the list leaves a conversation behind its Show older row under `window`: a space or group chat last
/// active more than the window's days before `now`, neither pinned, shown as unread (`shows_unread`), nor `open`.
/// People and apps never are.
pub fn is_older(group: &Group, open: bool, window: ListWindow, now: Micros) -> bool {
  let Some(days) = window.days() else {
    return false;
  };
  let stale = now.saturating_sub(group.sort_time) > Micros::from(days) * DAY_MICROS;
  matches!(group.kind, GroupKind::Space | GroupKind::GroupDm) && stale && !group.pinned && !shows_unread(group) && !open
}

/// A row of the Messages, Rail, and IRC list.
pub enum ListRow<'a> {
  Conv(&'a Arc<Conv>),
  /// The button that shows or hides the older conversations.
  Older,
}

/// The Messages, Rail, and IRC list: the conversations it shows, then, when `Settings::list_window` leaves some out,
/// the Show older row and, while it is open, the older ones after it.
#[derive(Default)]
pub struct Listing {
  pub recent: Vec<Arc<Conv>>,
  pub older: Vec<Arc<Conv>>,
  /// The Show older row was clicked. Kept in memory across rebuilds; starts closed.
  pub show_older: bool,
}

impl Listing {
  /// Splits `list`, already in list order, into the conversations shown and the older ones, each keeping that order.
  /// A search holds nothing back: its results come from every conversation.
  fn set(&mut self, list: Vec<Arc<Conv>>, searching: bool, window: ListWindow, open: Option<&GroupId>, now: Micros) {
    let window = if searching { ListWindow::All } else { window };
    (self.older, self.recent) = list
      .into_iter()
      .partition(|conv| is_older(&conv.g, open == Some(&conv.g.id), window, now));
  }

  /// How many rows the list has: the conversations shown, the Show older row when any are older, and the older ones
  /// while it is open.
  pub const fn rows(&self) -> usize {
    if self.older.is_empty() {
      return self.recent.len();
    }
    self.recent.len() + 1 + if self.show_older { self.older.len() } else { 0 }
  }

  /// What row `ix` of the list holds.
  pub fn row(&self, ix: usize) -> Option<ListRow<'_>> {
    let Some(after) = ix.checked_sub(self.recent.len()) else {
      return self.recent.get(ix).map(ListRow::Conv);
    };
    match after {
      _ if self.older.is_empty() => None,
      0 => Some(ListRow::Older),
      _ if self.show_older => self.older.get(after - 1).map(ListRow::Conv),
      _ => None,
    }
  }

  /// The conversations on screen, in list order: the older ones only while shown.
  pub fn visible(&self) -> impl Iterator<Item = &Arc<Conv>> {
    let older = if self.show_older {
      self.older.as_slice()
    } else {
      &[]
    };
    self.recent.iter().chain(older)
  }

  /// The row of a conversation on screen.
  pub fn position(&self, id: &GroupId) -> Option<usize> {
    if let Some(ix) = self.recent.iter().position(|conv| conv.g.id == *id) {
      return Some(ix);
    }
    let older = self
      .older
      .iter()
      .position(|conv| conv.g.id == *id)
      .filter(|_| self.show_older)?;
    Some(self.recent.len() + 1 + older)
  }
}

/// Unpinned rows a sidebar section shows before its Show all row is clicked.
const COLLAPSED_ROWS: usize = 8;

/// Which unpinned conversations of a sidebar section get rows, in the section's order (`section_order`, muted ones
/// last): every one while the section shows all; otherwise the first `COLLAPSED_ROWS`, and past those the ones that
/// show as unread (`shows_unread`) and the `open` one.
fn sidebar_rows<'a>(
  unpinned: &[&'a Arc<Conv>],
  show_all: bool,
  open: Option<&GroupId>,
) -> impl Iterator<Item = &'a Arc<Conv>> {
  unpinned
    .iter()
    .copied()
    .enumerate()
    .filter(move |(ix, conv)| show_all || *ix < COLLAPSED_ROWS || shows_unread(&conv.g) || open == Some(&conv.g.id))
    .map(|(_, conv)| conv)
}

pub struct Store {
  pub core: Core,
  pub images: Images,
  pub me: Option<User>,
  pub connection: Connection,
  pub signed_out: Option<String>,
  groups: HashMap<GroupId, Arc<Conv>>,
  /// Every conversation by `sort_time` descending, hidden and blocked ones included: what a search looks through, so
  /// they can be found again.
  pub every: Vec<Arc<Conv>>,
  /// `every` without hidden and blocked conversations: what the browsing lists, unread counts, and the Dock badge
  /// take.
  pub all: Vec<Arc<Conv>>,
  /// What Home shows: `all` narrowed by the kind picked (`pane`) and the Unread filter; while there is search text,
  /// `every` narrowed by those and ranked by the text, best match first. Empty in the layouts with a list beside the
  /// conversation, where Home shows none.
  pub home: Vec<Arc<Conv>>,
  /// What the Messages, Rail, and IRC list shows: `all` narrowed by `pane` and the Unread filter, in the order
  /// `list_key` gives (pinned first; in IRC by kind, each kind's muted ones last), with the conversations older than
  /// `list_window` behind a row at the end; while there is search text, `every` narrowed by those and the text
  /// instead, best match first, nothing held back. Empty in the Google Chat layout.
  pub list: Listing,
  /// What the chips or the Rail picked. It always fits the layout (`Pane::fits`): `sync_settings` puts it back to
  /// All when a layout change leaves one that does not.
  pub pane: Pane,
  /// Every Rail entry: All, Mentions, Starred, the user's own sections, each kind the account has, then Meetings.
  pub tabs: Vec<Tab>,
  /// How many conversations of each sidebar section show as unread (`shows_unread`), for the sections in `all`.
  section_unread: HashMap<Section, usize>,
  /// The Google Chat layout's sidebar: each section's pinned conversations, then the others in the section's order
  /// with the muted ones last (`section_order`). Built only while that layout is on.
  pub sidebar: Vec<SidebarItem>,
  pub view: View,
  /// Views left by navigating, newest last: what Back returns to.
  back: Vec<View>,
  /// Views left by Back, newest last: what Forward reopens. Navigating anywhere else clears it.
  forward: Vec<View>,
  /// Back or Forward is moving: the view change it makes is not a new step in the history.
  moving: bool,
  pub conv: Option<Entity<ConversationView>>,
  pub query: String,
  /// The message search started by Enter in the search field, shown under the conversation matches.
  pub search: Option<Search>,
  /// What the Mentions or Starred view shows.
  pub shortcut: Option<ShortcutList>,
  /// The Unread filter of every layout: Home and the list keep only the conversations that show as unread
  /// (`shows_unread`), within whatever `pane` is picked.
  pub unread_only: bool,
  /// Copies of `Settings::list_sort`, `Settings::section_sort`, and the `show_*` switches of the Mentions, Starred,
  /// Calendar, Meet, and Mail buttons, taken by `sync_settings`.
  pub list_sort: Sort,
  /// A copy of `Settings::layout`, taken by `sync_settings` too: it says which of `sidebar`, `home`, and `list` are on
  /// screen, so only those are built, and IRC's list keeps each kind of conversation together (`Kind::buffer_rank`).
  layout: Layout,
  /// A copy of `Settings::list_window`, taken by `sync_settings`.
  list_window: ListWindow,
  section_sort: std::collections::BTreeMap<String, Sort>,
  show_mentions: bool,
  show_starred: bool,
  /// A copy of `Settings::features`: a feature that is off has no view, button, notification, or core traffic. Mail
  /// off also means Gmail is not read at all.
  features: Features,
  /// A copy of `Settings::event_reminders`, taken by `sync_settings`.
  event_reminders: bool,
  /// `Command::WatchEvents(true)` was sent last.
  watching_events: bool,
  collapsed: HashSet<Section>,
  show_all: HashSet<Section>,
  histories: HashMap<GroupId, History>,
  /// Who has read the open conversation. Each event for that conversation replaces it, and it is dropped as soon as
  /// another view takes the conversation's place: the core sends it again whenever a conversation opens.
  receipts: Option<OpenReceipts>,
  /// Recently opened conversations, newest first.
  pub recent: Vec<GroupId>,
  /// What the Dock icon shows: unread conversations, and unread inbox threads while Mail is on.
  badge: usize,
  /// The unread mail count or the account changed since `badge` was counted.
  badge_dirty: bool,
  /// Notifications still showing, by conversation: taken down when the conversation is read.
  notified: HashMap<GroupId, Vec<String>>,
  /// People the user can start a chat with, name order.
  pub people: Directory,
  pub calendar: CalendarData,
  pub meet: MeetData,
  pub mail: MailData,
}

impl Store {
  pub fn new(core: Core, images: Images) -> Self {
    Self {
      core,
      images,
      me: None,
      connection: Connection::Connecting,
      signed_out: (!gchat_core::has_session()).then(|| "Sign in to start.".into()),
      groups: HashMap::new(),
      every: Vec::new(),
      all: Vec::new(),
      home: Vec::new(),
      list: Listing::default(),
      pane: Pane::All,
      tabs: Vec::new(),
      section_unread: HashMap::new(),
      sidebar: vec![
        SidebarItem::Header(Section::Shortcuts),
        SidebarItem::Home,
        SidebarItem::Shortcut(Shortcut::Mentions),
        SidebarItem::Shortcut(Shortcut::Starred),
        SidebarItem::Calendar,
        SidebarItem::Meet,
        SidebarItem::Mail,
      ],
      view: View::Home,
      back: Vec::new(),
      forward: Vec::new(),
      moving: false,
      conv: None,
      query: String::new(),
      search: None,
      shortcut: None,
      unread_only: false,
      list_sort: Sort::default(),
      layout: Layout::default(),
      list_window: ListWindow::default(),
      section_sort: std::collections::BTreeMap::new(),
      show_mentions: true,
      show_starred: true,
      features: Features::default(),
      event_reminders: true,
      watching_events: false,
      // The web app keeps Meetings folded at the bottom.
      collapsed: HashSet::from([Section::Meetings]),
      show_all: HashSet::new(),
      histories: HashMap::new(),
      receipts: None,
      recent: Vec::new(),
      badge: 0,
      badge_dirty: false,
      notified: HashMap::new(),
      people: Directory::default(),
      calendar: CalendarData::default(),
      meet: MeetData::default(),
      mail: MailData::default(),
    }
  }

  pub fn me_id(&self) -> String {
    self.me.as_ref().map_or_default(|u| u.id.clone())
  }

  pub fn group(&self, id: &GroupId) -> Option<&Arc<Conv>> {
    self.groups.get(id)
  }

  pub fn is_collapsed(&self, section: &Section) -> bool {
    self.collapsed.contains(section)
  }

  pub fn is_expanded(&self, section: &Section) -> bool {
    self.show_all.contains(section)
  }

  // Events.

  /// Applies a drained batch of core events, then notifies once when the batch changed what the store's readers draw.
  /// Events for the open conversation alone go to its view, which redraws itself.
  pub fn apply(&mut self, events: Vec<Event>, window: &mut Window, cx: &mut Context<Self>) {
    let mut groups_dirty = false;
    let mut changed = false;
    for event in events {
      match event {
        Event::Me(user) => {
          // The previews say "You" for the signed-in user's own messages.
          if self.me.as_ref().is_none_or(|me| me.id != user.id) {
            let me = user.id.as_str();
            for held in self.groups.values_mut() {
              *held = Arc::new(conv(held.g.clone(), me));
            }
            groups_dirty = true;
          }
          self.me = Some(user.clone());
          if let Some(conv) = &self.conv {
            conv.update(cx, |conv, cx| conv.set_me(user, cx));
          }
          changed = true;
        },
        Event::Connection(connection) => {
          self.signed_out = None;
          // The schedules can change in Google Chat while this was away.
          if connection == Connection::Online && self.connection != Connection::Online {
            self.core.send(Command::LoadDnd);
          }
          self.connection = connection;
          // The session is good: Gmail is read from now on, while the Mail button is on.
          self.start_mail();
          self.sync_event_watch();
          changed = true;
        },
        Event::World(list) => {
          // A conversation that did not change keeps its `Conv`.
          let me = self.me.as_ref().map_or("", |u| u.id.as_str());
          let mut before = std::mem::take(&mut self.groups);
          self.groups = list
            .iter()
            .map(|g| {
              let same = before.remove(&g.id).filter(|kept| kept.g == *g);
              (g.id.clone(), same.unwrap_or_else(|| Arc::new(conv(g.clone(), me))))
            })
            .collect();
          groups_dirty = true;
        },
        Event::GroupUpsert(group) => {
          if let View::Conversation(open) = &self.view
            && *open == group.id
            && let Some(view) = &self.conv
          {
            let group = group.clone();
            view.update(cx, |view, cx| view.set_group(group, cx));
          }
          // Read, here or on another device: its notifications are done.
          let was_unread = self.groups.get(&group.id).is_some_and(|c| c.g.unread);
          if was_unread && !group.unread {
            self.clear_notifications(&group.id);
          }
          let id = group.id.clone();
          let upserted = Arc::new(conv(group, self.me.as_ref().map_or("", |u| u.id.as_str())));
          self.groups.insert(id, upserted);
          groups_dirty = true;
        },
        Event::History {
          group,
          messages,
          reached_start,
          reached_end,
          prepended,
          focus,
        } => {
          self.histories.insert(
            group.clone(),
            History {
              messages: messages.clone(),
              reached_start,
              reached_end,
            },
          );
          if self.is_open(&group)
            && let Some(view) = &self.conv
          {
            let new_incoming = view.update(cx, |view, cx| {
              view.set_history(messages, reached_start, reached_end, prepended, focus, window, cx)
            });
            if new_incoming && reached_end && window.is_window_active() {
              self.core.send(Command::MarkRead(group));
            }
          }
        },
        Event::ReadReceipts { group, enabled, receipts } => {
          // Only the open conversation shows them. The core sends them again whenever one opens.
          let held = self
            .receipts
            .as_ref()
            .is_some_and(|held| held.group == group && held.enabled == enabled && held.receipts == receipts);
          if self.is_open(&group) && !held {
            if let Some(view) = &self.conv {
              view.update(cx, |view, cx| view.set_readers(enabled, receipts.clone(), cx));
            }
            self.receipts = Some(OpenReceipts { group, enabled, receipts });
          }
        },
        Event::SearchResults { query, hits } => {
          if let Some(search) = &mut self.search
            && search.query == query
          {
            search.hits = Some(hits);
            changed = true;
          }
        },
        Event::Shortcut { shortcut, hits, appended, cursor } => {
          if let Some(list) = &mut self.shortcut
            && list.shortcut == shortcut
          {
            match &mut list.hits {
              Some(held) if appended => held.extend(hits.iter().cloned()),
              _ => list.hits = Some(hits.to_vec()),
            }
            list.cursor = cursor;
            list.loading_more = false;
            changed = true;
          }
        },
        Event::MessageUpsert { message, replaces, incoming } => {
          changed |= self.call_posted(&message);
          if incoming {
            self.notify_incoming(&message, window, cx);
          }
          self.upsert_message(message, replaces, window, cx);
        },
        Event::MessageDeleted { group, id } => {
          if let Some(history) = self.histories.get_mut(&group) {
            // As in `upsert_message`: the open view hands its list back so the removal does not copy it.
            let open = matches!(&self.view, View::Conversation(open) if *open == group);
            let view = self.conv.as_ref().filter(|_| open);
            if let Some(view) = view {
              drop(view.update(cx, |view, _| view.take_messages()));
            }
            Arc::make_mut(&mut history.messages).retain(|m| m.id != id);
            let snapshot = history.clone();
            if let Some(view) = view {
              view.update(cx, |view, cx| {
                view.set_history(
                  snapshot.messages,
                  snapshot.reached_start,
                  snapshot.reached_end,
                  0,
                  None,
                  window,
                  cx,
                );
              });
            }
          }
        },
        Event::Typing { group, user_id, user_name, typing } => {
          if self.is_open(&group)
            && user_id != self.me_id()
            && let Some(view) = &self.conv
          {
            view.update(cx, |view, cx| view.set_typing(user_id, user_name, typing, cx));
          }
        },
        Event::SignedOut(reason) => {
          self.signed_out = Some(reason);
          // What was asked for before is not answered now.
          self.meet.creating = None;
          self.meet.calling = None;
          // The account's mail goes with it. The next sign-in reads Gmail again from the start.
          self.mail = MailData::default();
          self.badge_dirty = true;
          // The next session watches the calendars again once it is up (`sync_event_watch`).
          self.watching_events = false;
          changed = true;
        },
        // The call button posts the link. Auto-open then joins it (built-in window or browser).
        Event::MeetStarted(url) => {
          self.meet.calling = None;
          if cx.global::<crate::settings::Settings>().meet_auto_open {
            crate::call::open(&url, cx);
          }
          changed = true;
        },
        // Their readers observe these globals.
        Event::Mcp(status) => cx.set_global(crate::settings::McpState(status)),
        Event::Dnd(schedules) => cx.set_global(crate::status::DndState(Some(schedules))),
        Event::Mine(status) => {
          cx.set_global(crate::status::Mine(status));
          changed = true;
        },
        Event::Statuses(statuses) => {
          let statuses = crate::ui::People(statuses.into_iter().collect());
          if cx.try_global::<crate::ui::People>() != Some(&statuses) {
            cx.set_global(statuses);
            cx.refresh_windows();
          }
        },
        Event::People(people) => {
          self.people = Directory(people.into_iter().map(Arc::new).collect());
          changed = true;
        },
        Event::Members { group, users } => {
          if self.is_open(&group)
            && let Some(view) = &self.conv
          {
            view.update(cx, |view, cx| view.set_members(users, cx));
          }
        },
        Event::SlashCommands { group, commands } => {
          if self.is_open(&group)
            && let Some(view) = &self.conv
          {
            view.update(cx, |view, cx| view.set_slash_commands(commands, cx));
          }
        },
        Event::OpenChat(id) => {
          if self.is_call_chat(&id) {
            self.core.send(Command::StartMeet(id));
          } else {
            self.open(id, window, cx);
          }
          changed = true;
        },
        Event::Calendars(calendars) => {
          self.calendar.calendars = calendars;
          changed = true;
        },
        Event::CalendarEvents { start, end, events } => {
          // Each view keeps the range it asked for last. An answer for a range neither is on any more is dropped.
          let range = Some((start, end));
          if self.meet.range == range {
            self.meet.events = events.clone();
            self.meet.loaded = true;
            self.calendar.error = None;
            changed = true;
          }
          if self.calendar.range == range {
            self.calendar.events = events;
            self.calendar.loaded = true;
            self.calendar.error = None;
            changed = true;
          }
        },
        Event::CalendarWritten(result) => {
          self.calendar.writing = false;
          self.calendar.written = Some(result);
          changed = true;
        },
        Event::CalendarFailed(message) => {
          tracing::warn!("calendar: {message}");
          self.calendar.error = Some(message);
          changed = true;
        },
        Event::MeetingCreated(result) => {
          match (result, self.meet.creating.take()) {
            (Ok(url), Some(Creating::Instant)) => crate::call::open(&url, cx),
            (Ok(url), Some(Creating::ForLater)) => self.meet.made = Some(url),
            (Err(message), _) => self.meet.error = Some(message),
            // An answer nobody asked for.
            (Ok(_), None) => {},
          }
          changed = true;
        },
        Event::Error(message) => {
          tracing::warn!("core error: {message}");
          // The call that was being started failed: the Meet view says why.
          if self.meet.calling.take().is_some() {
            self.meet.error = Some(message);
            changed = true;
          }
        },
        // Answers already on their way when the Mail switch went off: Gmail is not read while it is off.
        Event::MailThreads { .. }
        | Event::MailMessages { .. }
        | Event::MailUnread(_)
        | Event::MailWritten(_)
        | Event::MailIncoming(_)
        | Event::MailFailed(_)
        | Event::MailLabels(_)
        | Event::MailSignatures { .. }
          if !self.features.mail => {},
        Event::MailThreads { mailbox, threads, more } => {
          tracing::debug!(mailbox = mailbox_kind(&mailbox), threads = threads.len(), more, "mail: threads");
          self.mail.lists.answered(mailbox, threads, more);
          changed = true;
        },
        Event::MailMessages { thread, messages } => {
          let open = self.mail.open.as_ref() == Some(&thread);
          tracing::debug!(emails = messages.len(), open, "mail: thread");
          // Only the thread open: an answer for one left meanwhile is dropped.
          if open {
            // Shown to the user, with the window in front: read, as a conversation opened is. The core sends the
            // thread again read, so this goes once.
            let unread = messages.iter().any(|message| message.unread);
            if unread && self.view == View::Mail && window.is_window_active() {
              self.core.send(Command::MarkMail { thread, read: true });
              self.mail.writes.push_back(MailWrite::Change);
            }
            self.mail.messages = Some(messages);
            self.mail.opening = false;
            self.mail.error = None;
            changed = true;
          }
        },
        Event::MailUnread(count) => {
          tracing::debug!(count, "mail: unread");
          if self.mail.unread != count {
            self.mail.unread = count;
            // The Dock number counts unread mail too.
            self.badge_dirty = true;
            changed = true;
          }
        },
        Event::MailIncoming(thread) => {
          tracing::debug!("mail: incoming");
          self.notify_mail(&thread, window, cx);
          changed |= self.mail.lists.incoming(thread);
        },
        Event::MailFailed(message) => {
          tracing::warn!("mail: {message}");
          // The lists on their way say why in their place, with a Retry; a thread that did not open, in its pane.
          let lists = self.mail.lists.failed(&message);
          if self.mail.opening || !lists {
            self.mail.error = Some(message);
          }
          self.mail.opening = false;
          changed = true;
        },
        Event::MailWritten(result) => {
          let kind = self.mail.writes.pop_front();
          let send = matches!(kind, Some(MailWrite::Reply | MailWrite::Compose));
          tracing::debug!(
            reply = matches!(kind, Some(MailWrite::Reply)),
            compose = matches!(kind, Some(MailWrite::Compose)),
            ok = result.is_ok(),
            pending = self.mail.writes.len(),
            "mail: written"
          );
          if send {
            self.mail.sending = false;
          }
          match result {
            Ok(()) if send => {
              self.mail.sent = match kind {
                Some(MailWrite::Compose) => Some(MailSent::Compose),
                _ => Some(MailSent::Reply),
              };
              self.mail.send_error = None;
            },
            Ok(()) => {},
            Err(message) => {
              tracing::warn!("mail: {message}");
              // A send that did not go says why in its box; a mark or an archive, where Mail shows failures.
              if send {
                self.mail.send_error = Some(message);
              } else {
                self.mail.error = Some(message);
              }
            },
          }
          changed = true;
        },
        Event::MailSignatures { new, reply } => {
          tracing::debug!(new = new.is_some(), reply = reply.is_some(), "mail: signatures");
          self.mail.signature_new = new;
          self.mail.signature_reply = reply;
          changed = true;
        },
        Event::MailLabels(labels) => {
          tracing::debug!(labels = labels.len(), "mail: labels");
          self.mail.labels = labels;
          changed = true;
        },
        Event::EventReminder { event, minutes } => self.remind(event, minutes, window, cx),
      }
    }
    if groups_dirty {
      self.rebuild();
    }
    if groups_dirty || self.badge_dirty {
      self.count_badge();
    }
    if changed || groups_dirty {
      cx.notify();
    }
  }

  /// Counts the Dock number again: unread conversations set to notify for everything, and those set to notify for
  /// mentions only while their unread activity mentions the user, plus the unread inbox threads while Mail is on.
  /// Hidden and blocked conversations are out of the lists, so they are out of it too.
  fn count_badge(&mut self) {
    self.badge_dirty = false;
    let chats = self
      .all
      .iter()
      .filter(|c| c.g.unread)
      .filter(|c| match c.g.notify {
        Notify::All | Notify::MainConversations => true,
        Notify::Mentions => c.g.mentioned,
        Notify::Off => false,
      })
      .count();
    // Unread mail counts while Mail is on; `mail` is cleared as it goes off, so its count is 0 then too.
    let mail = if self.features.mail {
      self.mail.unread as usize
    } else {
      0
    };
    let badge = chats + mail;
    if badge != self.badge {
      self.badge = badge;
      crate::notify::set_badge(badge);
    }
  }

  fn is_open(&self, group: &GroupId) -> bool {
    matches!(&self.view, View::Conversation(open) if open == group)
  }

  /// Opens what a clicked notification belongs to: a conversation, or with a `mail/<thread id>` key Mail on that
  /// thread.
  pub fn open_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
    if let Some(thread) = key.strip_prefix(MAIL_KEY) {
      let thread = thread.to_string();
      self.open_mail(cx);
      if self.view == View::Mail {
        self.open_mail_thread(thread, cx);
      }
      return;
    }
    if key.starts_with(EVENT_KEY) {
      let event = self.calendar.reminded.iter().rev().find(|(held, _)| held == key);
      self.calendar.reveal = event.map(|(_, event)| event.clone());
      self.open_calendar(cx);
      return;
    }
    if let Some(id) = self.groups.keys().find(|id| id.key() == key).cloned() {
      self.clear_notifications(&id);
      self.open(id, window, cx);
    }
  }

  /// A desktop notification for an unread email that reached the inbox: its sender, subject, and start, unless mail
  /// notifications are off, the user is quiet (`status::quiet`), or the window is in front showing Mail.
  fn notify_mail(&self, thread: &MailThread, window: &Window, cx: &App) {
    let settings = cx.global::<Settings>();
    if !settings.mail_notify || crate::status::quiet(cx) {
      return;
    }
    if window.is_window_active() && self.view == View::Mail {
      return;
    }
    let sender = thread.senders.last().map_or("Gmail", crate::mail_logic::person);
    let key = format!("{MAIL_KEY}{}", thread.id);
    let sound = settings.sound.resolved();
    crate::notify::post(&key, &key, sender, Some(&thread.subject), &thread.snippet, sound);
  }

  /// An event's reminder came: a notification with its title, when it starts, its time, and where, unless reminders
  /// are off or the user is quiet (`status::quiet`). Its event is kept so a click shows it.
  fn remind(&mut self, event: CalendarEvent, minutes: u32, _window: &Window, cx: &App) {
    if !self.event_reminders || !self.features.calendar || crate::status::quiet(cx) {
      return;
    }
    let key = format!("{EVENT_KEY}{}/{}", event.calendar_id, event.id);
    let h24 = crate::clock::is_24_hour();
    let when = if event.all_day {
      "All day".to_string()
    } else {
      let clock = |time| crate::util::clock_text(crate::util::local(time), h24);
      format!("{} to {}", clock(event.start), clock(event.end))
    };
    let place = if event.location.trim().is_empty() {
      event.meet_url.as_ref().map(|_| "Google Meet".to_string())
    } else {
      Some(event.location.trim().to_string())
    };
    let body = place.map_or_else(|| when.clone(), |place| format!("{when}, {place}"));
    let lead = crate::calendar_layout::starts_in(minutes);
    let sound = cx.global::<Settings>().sound.resolved();
    crate::notify::post(&key, &key, &event.title, Some(&lead), &body, sound);
    self.calendar.reminded.retain(|(held, _)| *held != key);
    self.calendar.reminded.push((key, event));
    if self.calendar.reminded.len() > REMINDED_LIMIT {
      self.calendar.reminded.remove(0);
    }
  }

  /// Has the core watch the calendars for reminders while `Settings::event_reminders` and the Calendar feature are
  /// both on, from the first time the connection is up, and stop when either goes off.
  fn sync_event_watch(&mut self) {
    let wanted = self.event_reminders && self.features.calendar;
    if wanted && !self.watching_events && self.connection == Connection::Online {
      self.core.send(Command::WatchEvents(true));
      self.watching_events = true;
    } else if !wanted && self.watching_events {
      self.core.send(Command::WatchEvents(false));
      self.watching_events = false;
    }
  }

  /// The event whose reminder was clicked, once: the Calendar view shows its day and its card.
  pub const fn take_reveal(&mut self) -> Option<CalendarEvent> {
    self.calendar.reveal.take()
  }

  fn clear_notifications(&mut self, group: &GroupId) {
    if let Some(ids) = self.notified.remove(group) {
      crate::notify::clear(&ids);
    }
  }

  /// A desktop notification for someone else's new message, when the conversation's Google Chat setting asks
  /// for one and the user is not already looking at it.
  fn notify_incoming(&mut self, message: &Message, window: &Window, cx: &App) {
    let Some(conv) = self.groups.get(&message.group) else {
      return;
    };
    let mentioned = message.mentions(&self.me_id());
    let wanted = match conv.g.notify {
      Notify::All => true,
      Notify::MainConversations => mentioned || !message.is_thread_reply(),
      Notify::Mentions => mentioned,
      Notify::Off => false,
    };
    if !wanted || (window.is_window_active() && self.is_open(&message.group)) {
      return;
    }
    if crate::status::quiet(cx) {
      return;
    }
    // Only what just happened: a reconnect can replay older messages.
    let now = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .map_or(0, |d| d.as_micros() as i64);
    if now - message.time > 120_000_000 {
      return;
    }
    let direct = matches!(conv.g.kind, GroupKind::Dm | GroupKind::App);
    let body = message.preview();
    let sound = cx.global::<crate::settings::Settings>().sound.resolved();
    let space = (!direct).then_some(conv.g.name.as_str());
    crate::notify::post(&message.id, &message.group.key(), &message.sender_name, space, &body, sound);
    self.notified.entry(message.group.clone()).or_default().push(message.id.clone());
  }

  fn upsert_message(
    &mut self,
    message: Arc<Message>,
    replaces: Option<String>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let group = message.group.clone();
    let Some(history) = self.histories.get_mut(&group) else {
      return;
    };
    // The open view hands its list back first, so `make_mut` changes the history's list in place instead of copying
    // it. `set_history` below always gives the view a list again.
    let open = matches!(&self.view, View::Conversation(open) if *open == group);
    let view = self.conv.as_ref().filter(|_| open);
    if let Some(view) = view {
      drop(view.update(cx, |view, _| view.take_messages()));
    }
    let list = Arc::make_mut(&mut history.messages);
    let existing = replaces
      .as_ref()
      .and_then(|local| list.iter().position(|m| &m.id == local))
      .or_else(|| list.iter().position(|m| m.id == message.id));
    if let Some(ix) = existing {
      list[ix] = message.clone()
    } else {
      // A thread reply sits under its topic; anything else goes by time.
      let topic_end = message
        .is_thread_reply()
        .then(|| list.iter().rposition(|m| m.topic_id == message.topic_id))
        .flatten();
      let at = match topic_end {
        Some(ix) => ix + 1,
        None => list.partition_point(|m| m.time <= message.time),
      };
      list.insert(at, message.clone());
    }
    let snapshot = history.clone();
    if let Some(view) = view {
      view.update(cx, |view, cx| {
        view.set_history(
          snapshot.messages,
          snapshot.reached_start,
          snapshot.reached_end,
          0,
          None,
          window,
          cx,
        );
      });
      let mine = self.me.as_ref().is_some_and(|me| me.id == message.sender_id);
      if !mine && !message.pending && existing.is_none() && window.is_window_active() {
        self.core.send(Command::MarkRead(group));
      }
    }
  }

  // Derived lists.

  fn rebuild(&mut self) {
    let mut every: Vec<Arc<Conv>> = self.groups.values().cloned().collect();
    every.sort_by(|a, b| b.g.sort_time.cmp(&a.g.sort_time).then_with(|| a.g.name.cmp(&b.g.name)));
    self.all = every.iter().filter(|conv| listed(&conv.g)).cloned().collect();
    self.every = every;
    self.rebuild_sidebar();
    self.rebuild_lists();
  }

  /// Home or the Messages, Rail, and IRC list, whichever the layout shows, the Rail tabs, and the sections' unread
  /// counts. The list keeps the chosen order within the parts `list_key` makes: pinned first, and in IRC one kind
  /// after another, each with its pinned, then the others, then the muted ones. Home, and search results, keep their
  /// own order.
  fn rebuild_lists(&mut self) {
    let query = self.query.trim();
    // Searching finds hidden and blocked conversations too, so they can be opened, unhidden, or unblocked again.
    let source = if query.is_empty() {
      &self.all
    } else {
      &self.every
    };
    let unread_only = self.unread_only;
    // `pane` fits the layout, so Home, in the Google Chat layout, only ever has All or a kind.
    let pane = &self.pane;
    let in_pane = |conv: &Arc<Conv>| match pane {
      Pane::All => true,
      Pane::Kind(kind) => Kind::of(conv.g.kind) == *kind,
      Pane::Section(section) => conv.section == *section,
      Pane::Shortcut(_) => false,
    };
    // Home lists conversations only where no list sits beside it, and the list only there.
    let beside = self.layout.keeps_list();
    let mut shown: Vec<Arc<Conv>> = source
      .iter()
      .filter(|conv| (!unread_only || shows_unread(&conv.g)) && in_pane(conv))
      .cloned()
      .collect();
    if query.is_empty() {
      if self.list_sort == Sort::Alphabetical {
        shown.sort_by(|a, b| name_key(a).cmp(name_key(b)));
      }
    } else {
      shown = Fuzzy::new(query)
        .rank(shown, |conv| conv.g.name.as_str())
        .into_iter()
        .map(|(_, conv)| conv)
        .collect();
    }
    let searching = !query.is_empty();
    if beside {
      // Stable, so the chosen order holds within each part. Search results keep best match first.
      if !searching {
        let irc = self.layout == Layout::Irc;
        shown.sort_by_key(|conv| list_key(&conv.g, irc));
      }
      self.home = Vec::new();
      self.set_list(shown, searching);
    } else {
      self.home = shown;
      self.set_list(Vec::new(), searching);
    }

    let mut by_section: HashMap<&Section, usize> = HashMap::new();
    let mut by_kind: HashMap<Kind, usize> = HashMap::new();
    for conv in &self.all {
      let unread = usize::from(shows_unread(&conv.g));
      *by_section.entry(&conv.section).or_default() += unread;
      *by_kind.entry(Kind::of(conv.g.kind)).or_default() += unread;
    }
    let mut tabs = vec![Tab {
      pane: Pane::All,
      unread: by_kind.values().sum(),
    }];
    for (shown, shortcut) in [(self.show_mentions, Shortcut::Mentions), (self.show_starred, Shortcut::Starred)] {
      if shown {
        tabs.push(Tab {
          pane: Pane::Shortcut(shortcut),
          unread: 0,
        });
      }
    }
    // The user's own sections, then the kinds the account has, then Meetings.
    let sections = ordered(by_section.keys().copied());
    let section_tab = |section: &Section| Tab {
      unread: by_section[section],
      pane: Pane::Section(section.clone()),
    };
    tabs.extend(
      sections
        .iter()
        .filter(|section| matches!(section, Section::Custom { .. }))
        .map(section_tab),
    );
    tabs.extend(
      Kind::ALL
        .into_iter()
        .filter_map(|kind| by_kind.get(&kind).map(|&unread| Tab { pane: Pane::Kind(kind), unread })),
    );
    tabs.extend(
      sections
        .iter()
        .filter(|section| **section == Section::Meetings)
        .map(section_tab),
    );
    self.tabs = tabs;
    self.section_unread.clear();
    self
      .section_unread
      .extend(by_section.into_iter().map(|(section, unread)| (section.clone(), unread)));
  }

  /// How many conversations of a pane show as unread: its tab's count, 0 for a pane without a tab.
  pub fn pane_unread(&self, pane: &Pane) -> usize {
    self.tabs.iter().find(|tab| tab.pane == *pane).map_or(0, |tab| tab.unread)
  }

  /// How many conversations of a sidebar section show as unread.
  pub fn section_unread(&self, section: &Section) -> usize {
    self.section_unread.get(section).copied().unwrap_or(0)
  }

  /// Splits the list by `list_window` around the open conversation.
  fn set_list(&mut self, list: Vec<Arc<Conv>>, searching: bool) {
    let open = self.view.conversation();
    let now = crate::calendar_layout::now_micros();
    self.list.set(list, searching, self.list_window, open, now);
  }

  /// Shows or hides the older conversations at the end of the list.
  pub fn toggle_older(&mut self, cx: &mut Context<Self>) {
    self.list.show_older.toggle();
    cx.notify();
  }

  /// The Google Chat layout's sidebar; the other layouts have none. Each section lists its pinned conversations A to
  /// Z, then the others in the section's order with the muted ones last (`section_order`), as the web app does. Its
  /// rows depend on the open conversation (`sidebar_rows`), so `set_view` builds it again when that changes.
  pub fn rebuild_sidebar(&mut self) {
    if self.layout != Layout::GoogleChat {
      self.sidebar = Vec::new();
      return;
    }
    let mut items = vec![SidebarItem::Header(Section::Shortcuts)];
    if !self.collapsed.contains(&Section::Shortcuts) {
      items.push(SidebarItem::Home);
      if self.show_mentions {
        items.push(SidebarItem::Shortcut(Shortcut::Mentions));
      }
      if self.show_starred {
        items.push(SidebarItem::Shortcut(Shortcut::Starred));
      }
      if self.features.calendar {
        items.push(SidebarItem::Calendar);
      }
      if self.features.meet {
        items.push(SidebarItem::Meet);
      }
      if self.features.mail {
        items.push(SidebarItem::Mail);
      }
    }
    let mut by_section: HashMap<&Section, Vec<&Arc<Conv>>> = HashMap::new();
    for conv in &self.all {
      by_section.entry(&conv.section).or_default().push(conv);
    }
    let order = ordered(by_section.keys().copied());
    for section in order {
      // `all` is newest first, so each section starts in that order.
      let Some(members) = by_section.remove(&section) else {
        continue;
      };
      items.push(SidebarItem::Header(section.clone()));
      if self.collapsed.contains(&section) {
        continue;
      }
      let (pinned, unpinned) = section_order(members, self.section_sort(&section));
      items.extend(pinned.into_iter().map(|c| SidebarItem::Conv(c.clone())));
      let show_all = self.show_all.contains(&section);
      let rows = sidebar_rows(&unpinned, show_all, self.view.conversation());
      items.extend(rows.map(|c| SidebarItem::Conv(c.clone())));
      if unpinned.len() > COLLAPSED_ROWS {
        items.push(SidebarItem::Toggle(section));
      }
    }
    self.sidebar = items;
  }

  pub fn toggle_collapsed(&mut self, section: Section, cx: &mut Context<Self>) {
    if !self.collapsed.remove(&section) {
      self.collapsed.insert(section);
    }
    self.rebuild_sidebar();
    cx.notify();
  }

  pub fn toggle_show_all(&mut self, section: Section, cx: &mut Context<Self>) {
    if !self.show_all.remove(&section) {
      self.show_all.insert(section);
    }
    self.rebuild_sidebar();
    cx.notify();
  }

  pub fn set_unread_only(&mut self, on: bool, cx: &mut Context<Self>) {
    self.unread_only = on;
    self.rebuild_lists();
    cx.notify();
  }

  pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
    if query == self.query {
      return;
    }
    self.query = query;
    // Editing the text goes back to filtering names; Enter searches messages again.
    if self.search.as_ref().is_some_and(|search| search.query != self.query.trim()) {
      self.search = None;
    }
    self.rebuild_lists();
    // Layouts with a list beside the conversation filter it in place; the others filter Home.
    if !self.query.is_empty() && self.view != View::Home && !cx.global::<Settings>().layout.keeps_list() {
      self.go_home(cx);
    }
    cx.notify();
  }

  /// Picks what the Messages and Rail list shows. Mentions and Starred load their messages into the list and leave
  /// the open conversation alone.
  pub fn set_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
    if pane == self.pane {
      return;
    }
    self.pane = pane;
    self.sync_pane(cx);
    self.rebuild_lists();
    cx.notify();
  }

  /// Loads the shortcut messages the Rail list shows when they are not the ones in `shortcut`, as after another
  /// layout used it, and forgets them when nothing on screen shows them. `set_pane` and `sync_settings` call it when
  /// the pane or the layout changes.
  pub fn sync_pane(&mut self, cx: &App) {
    let Pane::Shortcut(shortcut) = self.pane else {
      self.drop_shortcut(cx);
      return;
    };
    if cx.global::<Settings>().layout == Layout::Rail
      && self.shortcut.as_ref().is_none_or(|list| list.shortcut != shortcut)
    {
      self.load_shortcut(shortcut);
    }
  }

  /// Forgets the shortcut messages unless something on screen shows them: the Mentions or Starred view, or the
  /// Rail list.
  fn drop_shortcut(&mut self, cx: &App) {
    let in_rail = matches!(self.pane, Pane::Shortcut(_)) && cx.global::<Settings>().layout == Layout::Rail;
    if !in_rail && !matches!(self.view, View::Shortcut(_)) {
      self.shortcut = None;
    }
  }

  /// Conversations in the order the layout lists them.
  fn order(&self, layout: Layout) -> Vec<&Arc<Conv>> {
    match layout {
      Layout::GoogleChat => self
        .sidebar
        .iter()
        .filter_map(|item| match item {
          SidebarItem::Conv(conv) => Some(conv),
          _ => None,
        })
        .collect(),
      Layout::Messages | Layout::Rail | Layout::Irc => self.list.visible().collect(),
    }
  }

  /// The sections the user made in Google Chat, A to Z: those its conversations are placed in.
  pub fn custom_sections(&self) -> Vec<Section> {
    let present: HashSet<&Section> = self
      .every
      .iter()
      .map(|conv| &conv.section)
      .filter(|section| matches!(section, Section::Custom { .. }))
      .collect();
    ordered(present.into_iter())
  }

  /// The order of a Google Chat sidebar section: the user's pick, or the section's default.
  pub fn section_sort(&self, section: &Section) -> Sort {
    self
      .section_sort
      .get(&section.key())
      .copied()
      .unwrap_or_else(|| section.default_sort())
  }

  /// Takes the layout, the sort orders, the list's window, the Mentions and Starred buttons, and the features from the
  /// settings, rebuilding the lists when they changed. Call at start and whenever the settings change. A layout
  /// change puts the pane back to All when the new layout cannot pick it (`Pane::fits`) and syncs the Rail's shortcut
  /// messages (`sync_pane`). Mail turning on starts reading Gmail, and turning off stops it (`stop_mail`); a feature
  /// going off with its view on screen goes Home.
  pub fn sync_settings(&mut self, cx: &mut Context<Self>) {
    let settings = cx.global::<Settings>();
    if settings.list_sort == self.list_sort
      && settings.layout == self.layout
      && settings.list_window == self.list_window
      && settings.section_sort == self.section_sort
      && settings.show_mentions == self.show_mentions
      && settings.show_starred == self.show_starred
      && settings.features() == self.features
      && settings.event_reminders == self.event_reminders
    {
      return;
    }
    let layout_changed = settings.layout != self.layout;
    self.list_sort = settings.list_sort;
    self.layout = settings.layout;
    self.list_window = settings.list_window;
    self.section_sort = settings.section_sort.clone();
    self.show_mentions = settings.show_mentions;
    self.show_starred = settings.show_starred;
    let mail_was = self.features.mail;
    self.features = settings.features();
    self.event_reminders = settings.event_reminders;
    if layout_changed {
      if !self.pane.fits(self.layout) {
        self.pane = Pane::All;
      }
      self.sync_pane(cx);
    }
    if self.features.mail {
      self.start_mail();
    } else if mail_was {
      self.stop_mail(cx);
    }
    self.sync_event_watch();
    // The view on screen belongs to a feature just switched off: the window goes Home.
    if !shows(&self.view, self.features) {
      self.go_home(cx);
    }
    self.rebuild_sidebar();
    self.rebuild_lists();
    cx.notify();
  }

  /// Opens the next (or previous) conversation in the list the layout shows, after the open one. With none open,
  /// the first (or last).
  pub fn step_conversation(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
    let order = self.order(cx.global::<Settings>().layout);
    let Some(last) = order.len().checked_sub(1) else {
      return;
    };
    let current = match &self.view {
      View::Conversation(id) => order.iter().position(|conv| &conv.g.id == id),
      View::Home | View::Shortcut(_) | View::Calendar | View::Meet | View::Mail => None,
    };
    let next = match (current, forward) {
      (Some(ix), true) => (ix + 1).min(last),
      (Some(ix), false) => ix.saturating_sub(1),
      (None, true) => 0,
      (None, false) => last,
    };
    let id = order[next].g.id.clone();
    self.open(id, window, cx);
  }

  /// Opens the newest conversation showing as unread (`shows_unread`) other than the open one.
  pub fn open_next_unread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let open = self.view.conversation();
    let Some(id) = self
      .all
      .iter()
      .find(|conv| shows_unread(&conv.g) && Some(&conv.g.id) != open)
      .map(|conv| conv.g.id.clone())
    else {
      return;
    };
    self.open(id, window, cx);
  }

  // Navigation.

  /// The existing DM or group DM whose other members are exactly `ids`.
  pub fn existing_chat(&self, ids: &[String]) -> Option<GroupId> {
    let want: HashSet<&String> = ids.iter().collect();
    self.groups.values().find_map(|conv| {
      let members: HashSet<&String> = conv.g.member_ids.iter().collect();
      let kind_ok = if ids.len() == 1 {
        conv.g.kind == GroupKind::Dm
      } else {
        conv.g.kind == GroupKind::GroupDm
      };
      (kind_ok && members == want).then(|| conv.g.id.clone())
    })
  }

  /// Opens the one-to-one chat with a person or app, as clicking their name or a mention of them asks. Without one
  /// yet, the core creates it and answers `Event::OpenChat`.
  pub fn open_person(&mut self, user_id: &str, window: &mut Window, cx: &mut Context<Self>) {
    if user_id.is_empty() || user_id == self.me_id() {
      return;
    }
    let existing = self.groups.values().find_map(|conv| {
      let one = matches!(conv.g.kind, GroupKind::Dm | GroupKind::App) && conv.g.member_ids == [user_id];
      one.then(|| conv.g.id.clone())
    });
    match existing {
      Some(id) => self.open(id, window, cx),
      None => self.core.send(Command::CreateChat { user_ids: vec![user_id.to_string()] }),
    }
  }

  pub fn open(&mut self, id: GroupId, window: &mut Window, cx: &mut Context<Self>) {
    let Some(group) = self.groups.get(&id).map(|c| c.g.clone()) else {
      return;
    };
    if self.is_open(&id) {
      if let Some(view) = &self.conv {
        view.update(cx, |view, cx| view.focus_composer(window, cx));
      }
      return;
    }
    if !self.query.is_empty() {
      self.query.clear();
      self.search = None;
    }
    self.recent.retain(|r| r != &id);
    self.recent.insert(0, id.clone());
    self.recent.truncate(50);
    // A window left by a jump is not what opening shows: the core answers with the latest messages.
    let history = self.histories.get(&id).filter(|history| history.reached_end).cloned();
    let store = cx.weak_entity();
    let (core, images, me) = (self.core.clone(), self.images.clone(), self.me.clone());
    self.conv = Some(cx.new(|cx| ConversationView::new(store, core, images, group, me, history, window, cx)));
    self.set_view(View::Conversation(id.clone()));
    // The open conversation is never held back as older.
    self.rebuild_lists();
    self.drop_shortcut(cx);
    self.core.send(Command::Open(id.clone()));
    self.clear_notifications(&id);
    self.core.send(Command::MarkRead(id));
    cx.notify();
  }

  /// Switches the view, recording the one it leaves as a step Back can return to. A change of the open conversation
  /// builds the sidebar again, whose rows depend on it.
  fn set_view(&mut self, view: View) {
    let reopened = view.conversation() != self.view.conversation();
    if view != self.view && !self.moving {
      self.back.push(std::mem::replace(&mut self.view, View::Home));
      if self.back.len() > HISTORY_LIMIT {
        self.back.remove(0);
      }
      self.forward.clear();
    }
    self.view = view;
    // Who has read a conversation belongs to it alone.
    if self.receipts.as_ref().is_some_and(|held| !self.is_open(&held.group)) {
      self.receipts = None;
    }
    if reopened {
      self.rebuild_sidebar();
    }
  }

  /// Returns to the view before this one. With nothing recorded, a conversation or a shortcut goes home.
  pub fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let target = match self.next_step(true) {
      Some(view) => view,
      None if self.view != View::Home => View::Home,
      None => return,
    };
    self.forward.push(self.view.clone());
    self.show(target, window, cx);
  }

  /// Reopens the view the last Back left.
  pub fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(target) = self.next_step(false) else {
      return;
    };
    self.back.push(self.view.clone());
    self.show(target, window, cx);
  }

  /// The newest view on one stack that can still be shown and is not the one on screen. Conversations that are
  /// gone (left, or deleted) or out of the lists (hidden, or blocked) are skipped.
  fn next_step(&mut self, back: bool) -> Option<View> {
    let stack = if back {
      &mut self.back
    } else {
      &mut self.forward
    };
    while let Some(view) = stack.pop() {
      let shown = match &view {
        View::Conversation(id) => self.groups.get(id).is_some_and(|conv| listed(&conv.g)),
        View::Home | View::Shortcut(_) => true,
        // A feature that is off is never landed on.
        View::Calendar | View::Meet | View::Mail => shows(&view, self.features),
      };
      if shown && view != self.view {
        return Some(view);
      }
    }
    None
  }

  fn show(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
    self.moving = true;
    match view {
      View::Home => self.go_home(cx),
      View::Shortcut(shortcut) => self.open_shortcut(shortcut, cx),
      View::Calendar => self.open_calendar(cx),
      View::Meet => self.open_meet(cx),
      View::Mail => self.open_mail(cx),
      View::Conversation(id) => self.open(id, window, cx),
    }
    self.moving = false;
  }

  pub fn go_home(&mut self, cx: &mut Context<Self>) {
    if self.view != View::Home {
      self.core.send(Command::Close);
    }
    self.set_view(View::Home);
    // Dropping the view tells the core we stopped typing.
    self.conv = None;
    self.drop_shortcut(cx);
    cx.notify();
  }

  /// Shows Mentions or Starred, loading its newest messages again.
  pub fn open_shortcut(&mut self, shortcut: Shortcut, cx: &mut Context<Self>) {
    if matches!(self.view, View::Conversation(_)) {
      self.core.send(Command::Close);
    }
    self.set_view(View::Shortcut(shortcut));
    self.conv = None;
    self.load_shortcut(shortcut);
    cx.notify();
  }

  /// Shows Google Calendar. The view asks for the time it shows.
  pub fn open_calendar(&mut self, cx: &mut Context<Self>) {
    if !self.features.calendar {
      return;
    }
    if matches!(self.view, View::Conversation(_)) {
      self.core.send(Command::Close);
    }
    self.set_view(View::Calendar);
    self.conv = None;
    self.drop_shortcut(cx);
    cx.notify();
  }

  /// Shows Meet. The view asks for the day it shows.
  pub fn open_meet(&mut self, cx: &mut Context<Self>) {
    if !self.features.meet {
      return;
    }
    if matches!(self.view, View::Conversation(_)) {
      self.core.send(Command::Close);
    }
    self.set_view(View::Meet);
    self.conv = None;
    self.drop_shortcut(cx);
    self.meet.error = None;
    cx.notify();
  }

  /// Shows Gmail. The inbox is asked for when it is not held yet and not on its way, as after the last read failed.
  /// Nothing happens while the Mail switch is off: Gmail is not read then.
  pub fn open_mail(&mut self, cx: &mut Context<Self>) {
    if !self.features.mail {
      return;
    }
    if matches!(self.view, View::Conversation(_)) {
      self.core.send(Command::Close);
    }
    self.set_view(View::Mail);
    self.conv = None;
    self.drop_shortcut(cx);
    // As after the last read of it failed, too: opening Mail tries again.
    let inbox = self.mail.lists.get(&Mailbox::Inbox);
    if inbox.is_none_or(|inbox| !inbox.loaded && !inbox.loading) {
      self.request_mail(Mailbox::Inbox, false);
    }
    cx.notify();
  }

  /// Asks for a mailbox's newest threads, or with `more` for the next older page after those held. Nothing goes while
  /// a page of that mailbox is on its way, nor for `more` once Gmail said there is nothing older. The threads held stay
  /// until the answer comes.
  pub fn load_mail(&mut self, mailbox: Mailbox, more: bool, cx: &mut Context<Self>) {
    if self.request_mail(mailbox, more) {
      cx.notify();
    }
  }

  /// Sends `LoadMail` unless it would ask again for what is on its way or for a page that is not there. Answers whether
  /// it went.
  fn request_mail(&mut self, mailbox: Mailbox, more: bool) -> bool {
    let asked = self.mail.lists.ask(&mailbox, more);
    tracing::debug!(mailbox = mailbox_kind(&mailbox), more, asked, "mail: load");
    if !asked {
      return false;
    }
    // The core watches the inbox from its first load on.
    if mailbox == Mailbox::Inbox {
      self.mail.watching = true;
    }
    self.core.send(Command::LoadMail { mailbox, more });
    true
  }

  /// Reads Gmail from now on: asks for the inbox once, which also has the core watch it, while the Mail switch is on
  /// and the connection is up (the session is good). Never while the switch is off.
  fn start_mail(&mut self) {
    if self.features.mail && !self.mail.watching && self.connection == Connection::Online {
      self.request_mail(Mailbox::Inbox, false);
    }
  }

  /// The Mail switch went off: the core stops watching the inbox and forgets its mail, the mail held here goes, the
  /// Dock number drops its count, and the Mail view, if it is on screen, goes Home.
  fn stop_mail(&mut self, cx: &mut Context<Self>) {
    self.core.send(Command::StopMail);
    self.mail = MailData::default();
    self.count_badge();
    if self.view == View::Mail {
      self.go_home(cx);
    }
  }

  /// Opens a thread, by Gmail's thread id, and asks for its emails. Its emails held stay while they are asked for
  /// again; another thread's go at once. Its emails reaching the screen, with the window in front, mark it read
  /// (`Event::MailMessages`).
  pub fn open_mail_thread(&mut self, id: String, cx: &mut Context<Self>) {
    if !self.features.mail {
      return;
    }
    if self.mail.open.as_ref() == Some(&id) {
      if self.mail.opening {
        return;
      }
    } else {
      self.mail.open = Some(id.clone());
      self.mail.messages = None;
    }
    self.mail.opening = true;
    self.core.send(Command::OpenMail(id));
    cx.notify();
  }

  /// Marks a thread read or unread in Gmail. The core changes the lists at once and answers `MailWritten`.
  pub fn mark_mail(&mut self, thread: String, read: bool, cx: &mut Context<Self>) {
    if self.features.mail {
      self.core.send(Command::MarkMail { thread, read });
      self.mail.writes.push_back(MailWrite::Change);
      cx.notify();
    }
  }

  /// Takes a thread out of the inbox, or puts it back. The core changes the lists at once and answers `MailWritten`.
  pub fn archive_mail(&mut self, thread: String, archive: bool, cx: &mut Context<Self>) {
    if self.features.mail {
      self.core.send(Command::ArchiveMail { thread, archive });
      self.mail.writes.push_back(MailWrite::Change);
      cx.notify();
    }
  }

  /// Sends a reply, unless a send is on its way. `MailWritten` answers; the thread comes again with it once it went.
  pub fn reply_mail(&mut self, reply: MailReply, cx: &mut Context<Self>) {
    if !self.features.mail || self.mail.sending {
      return;
    }
    self.mail.sending = true;
    self.mail.send_error = None;
    self.core.send(Command::ReplyMail(Box::new(reply)));
    self.mail.writes.push_back(MailWrite::Reply);
    cx.notify();
  }

  /// Sends a new email, unless a send is on its way. `MailWritten` answers; the lists it shows in follow.
  pub fn compose_mail(&mut self, compose: MailCompose, cx: &mut Context<Self>) {
    if !self.features.mail || self.mail.sending {
      return;
    }
    self.mail.sending = true;
    self.mail.send_error = None;
    self.core.send(Command::ComposeMail(Box::new(compose)));
    self.mail.writes.push_back(MailWrite::Compose);
    cx.notify();
  }

  /// Which send went since the view last asked, once.
  pub const fn take_sent(&mut self) -> Option<MailSent> {
    self.mail.sent.take()
  }

  /// Closes the thread open: the Mail view shows its list again.
  pub fn close_mail_thread(&mut self, cx: &mut Context<Self>) {
    if self.mail.open.take().is_some() {
      self.mail.messages = None;
      self.mail.opening = false;
      cx.notify();
    }
  }

  /// Asks for the events of `start..end`, and for the calendars when none are held. The events on screen stay until
  /// the answer comes.
  pub fn load_calendar(&mut self, start: Micros, end: Micros, cx: &mut Context<Self>) {
    if self.calendar.calendars.is_empty() {
      self.core.send(Command::LoadCalendars);
    }
    if self.calendar.range != Some((start, end)) {
      self.calendar.range = Some((start, end));
      self.calendar.loaded = false;
    }
    self.core.send(Command::LoadEvents { start, end });
    cx.notify();
  }

  /// Sends a calendar write (`SaveEvent`, `DeleteEvent`, `RespondEvent`). The core answers `CalendarWritten`, and
  /// then the events again.
  pub fn write_calendar(&mut self, command: Command, cx: &mut Context<Self>) {
    self.calendar.writing = true;
    self.calendar.written = None;
    self.core.send(command);
    cx.notify();
  }

  /// How the last calendar write went, once.
  pub const fn take_calendar_write(&mut self) -> Option<Result<(), String>> {
    self.calendar.written.take()
  }

  /// Asks for the events of `start..end` for the Meet view, and for the calendars when none are held: the editor that
  /// "Schedule in Google Calendar" opens needs them. The events on screen stay until the answer comes.
  pub fn load_meet(&mut self, start: Micros, end: Micros, cx: &mut Context<Self>) {
    if self.calendar.calendars.is_empty() {
      self.core.send(Command::LoadCalendars);
    }
    if self.meet.range != Some((start, end)) {
      self.meet.range = Some((start, end));
      self.meet.loaded = false;
    }
    self.core.send(Command::LoadEvents { start, end });
    cx.notify();
  }

  /// Asks the core for a meeting of its own. `MeetingCreated` answers, and `intent` says what becomes of the link.
  pub fn create_meeting(&mut self, intent: Creating, cx: &mut Context<Self>) {
    if self.meet.creating.is_some() {
      return;
    }
    self.meet.creating = Some(intent);
    self.meet.error = None;
    self.core.send(Command::CreateMeeting);
    cx.notify();
  }

  /// The link of the meeting made for later, once.
  pub const fn take_meeting(&mut self) -> Option<String> {
    self.meet.made.take()
  }

  /// Starts a call with a person: the core posts a meeting in their chat, and `MeetStarted` opens it. Without a chat
  /// yet, the core makes one first and `OpenChat` carries the call on from there, without opening that chat.
  pub fn call_person(&mut self, person: &User, cx: &mut Context<Self>) {
    if self.meet.calling.is_some() {
      return;
    }
    let chat = self.existing_chat(std::slice::from_ref(&person.id));
    self.meet.error = None;
    self.meet.calling = Some(Calling::new(person.name.clone(), person.id.clone(), chat.clone()));
    match chat {
      Some(id) => self.core.send(Command::StartMeet(id)),
      None => self.core.send(Command::CreateChat { user_ids: vec![person.id.clone()] }),
    }
    cx.notify();
  }

  /// Whether `chat` is the one made for the call being started: the meeting is posted in it, and it is not opened.
  fn is_call_chat(&mut self, chat: &GroupId) -> bool {
    let (Some(call), Some(conv)) = (&mut self.meet.calling, self.groups.get(chat)) else {
      return false;
    };
    call.take_chat(chat, &conv.g.member_ids)
  }

  /// A meeting card in the chat of the call being started says the meeting is posted: nothing is waited for. The core
  /// answers `MeetStarted` after it, but an answer that never comes must not leave the call waiting. Answers whether
  /// it was that card.
  fn call_posted(&mut self, message: &Message) -> bool {
    let card = message
      .attachments
      .iter()
      .any(|attachment| attachment.kind == AttachmentKind::Meet);
    let posted = self
      .meet
      .calling
      .as_ref()
      .is_some_and(|call| call.is_posted(&message.group, card));
    if posted {
      self.meet.calling = None;
    }
    posted
  }

  /// Asks for the newest messages of a shortcut, replacing the ones held.
  fn load_shortcut(&mut self, shortcut: Shortcut) {
    self.shortcut = Some(ShortcutList {
      shortcut,
      hits: None,
      cursor: None,
      loading_more: false,
    });
    self.core.send(Command::LoadShortcut { shortcut, cursor: None });
  }

  /// Asks for the next, older page of the shortcut on screen, unless one is on its way or none is left.
  pub fn load_more_shortcut(&mut self) {
    let Some(list) = &mut self.shortcut else {
      return;
    };
    if list.loading_more {
      return;
    }
    let Some(cursor) = list.cursor.clone() else {
      return;
    };
    list.loading_more = true;
    self.core.send(Command::LoadShortcut {
      shortcut: list.shortcut,
      cursor: Some(cursor),
    });
  }

  /// Searches messages for the text in the search field.
  pub fn run_search(&mut self, cx: &mut Context<Self>) {
    let query = self.query.trim().to_string();
    if query.is_empty() {
      return;
    }
    self.core.send(Command::Search(query.clone()));
    self.search = Some(Search { query, hits: None });
    if self.view != View::Home {
      self.go_home(cx);
    }
    cx.notify();
  }

  /// Opens the conversation of a search hit at that message.
  pub fn open_hit(&mut self, hit: &SearchHit, window: &mut Window, cx: &mut Context<Self>) {
    let Some(group) = self.groups.get(&hit.group).map(|c| c.g.clone()) else {
      return;
    };
    if !self.is_open(&hit.group) {
      let store = cx.weak_entity();
      let (core, images, me) = (self.core.clone(), self.images.clone(), self.me.clone());
      self.conv = Some(cx.new(|cx| ConversationView::new(store, core, images, group, me, None, window, cx)));
      self.set_view(View::Conversation(hit.group.clone()));
      self.drop_shortcut(cx);
    }
    self.core.send(Command::Jump {
      group: hit.group.clone(),
      message_id: hit.message_id.clone(),
      time: hit.time,
    });
    cx.notify();
  }
}

/// A conversation's name for A to Z order: "Group: Ana" sorts under A, as in the web app.
fn name_key(conv: &Conv) -> &str {
  conv.lower.trim_start_matches("group: ")
}

/// Where a conversation stands within its sidebar section or IRC kind: pinned first, then the others, then the muted
/// ones, as the web app sinks a muted conversation to the bottom of its section. A pinned muted one stays pinned.
const fn standing(group: &Group) -> u8 {
  if group.pinned {
    0
  } else if group.muted {
    2
  } else {
    1
  }
}

/// The sort key of the Messages, Rail, and IRC list, for a stable sort over the chosen order: pinned first; in IRC
/// (`irc`) one kind after another (`Kind::buffer_rank`), each with its pinned, then the others, then the muted ones.
const fn list_key(group: &Group, irc: bool) -> (u8, u8) {
  if irc {
    (Kind::of(group.kind).buffer_rank(), standing(group))
  } else {
    (0, if group.pinned { 0 } else { 1 })
  }
}

/// A sidebar section's conversations, `members` newest first, in sidebar order: the pinned ones A to Z, and the
/// others in `sort` with the muted ones after the rest, each part keeping that order.
fn section_order(members: Vec<&Arc<Conv>>, sort: Sort) -> (Vec<&Arc<Conv>>, Vec<&Arc<Conv>>) {
  let (mut pinned, mut unpinned): (Vec<&Arc<Conv>>, Vec<&Arc<Conv>>) = members.into_iter().partition(|c| c.g.pinned);
  // "Group: Ana" sorts under A, as in the web app.
  pinned.sort_by(|a, b| name_key(a).cmp(name_key(b)));
  if sort == Sort::Alphabetical {
    unpinned.sort_by(|a, b| name_key(a).cmp(name_key(b)));
  }
  unpinned.sort_by_key(|c| standing(&c.g));
  (pinned, unpinned)
}

/// The `Conv` of a group. `me` is the signed-in user's id, empty until the core says it: their own last message is
/// led by "You".
fn conv(group: Group, me: &str) -> Conv {
  let lower = group.name.to_lowercase();
  let section = Section::of(&group);
  let name = SharedString::from(group.name.clone());
  let preview = group
    .snippet
    .as_ref()
    .map_or_else(SharedString::default, |snippet| preview_line(snippet, me));
  Conv { g: group, lower, section, name, preview }
}

/// A last message on one line, at most 240 characters of it, led by "You" or its sender's name.
fn preview_line(snippet: &Snippet, me: &str) -> SharedString {
  let sender = if !me.is_empty() && snippet.sender_id == me {
    "You"
  } else {
    snippet.sender_name.as_str()
  };
  let text: String = snippet
    .text
    .chars()
    .take(240)
    .map(|c| if c == '\n' { ' ' } else { c })
    .collect();
  if sender.is_empty() {
    text.into()
  } else {
    format!("{sender}: {text}").into()
  }
}

/// Whether a conversation belongs in the browsing lists: neither hidden nor blocked. Searches find it either way.
pub const fn listed(group: &Group) -> bool {
  !group.hidden && !group.blocked
}

/// Whether the lists show a conversation as unread (bold, marked, counted, kept by the Unread filter): it is unread,
/// and a muted one only once it mentions the user, as the Google Chat web app does.
pub const fn shows_unread(group: &Group) -> bool {
  group.unread && (!group.muted || group.mentioned)
}

/// Whether the unread mark is the mention one.
pub const fn shows_mention(group: &Group) -> bool {
  group.unread && group.mentioned
}

/// Sections in sidebar order: the user's own A to Z, then the fixed ones. Only those in `present`.
fn ordered<'a>(present: impl Iterator<Item = &'a Section>) -> Vec<Section> {
  let present: Vec<&Section> = present.collect();
  let mut order: Vec<Section> = present
    .iter()
    .filter(|section| matches!(section, Section::Custom { .. }))
    .map(|section| (*section).clone())
    .collect();
  order.sort_by_key(|section| section.title().to_lowercase());
  order.extend(
    [Section::Dms, Section::Apps, Section::Spaces, Section::Meetings]
      .into_iter()
      .filter(|section| present.contains(&section)),
  );
  order
}

/// Whether `view` can be on screen while `features` are on: a feature's view only while it is on.
pub const fn shows(view: &View, features: Features) -> bool {
  match view {
    View::Calendar => features.calendar,
    View::Meet => features.meet,
    View::Mail => features.mail,
    View::Home | View::Shortcut(_) | View::Conversation(_) => true,
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;

  use super::{
    Calling, Conv, DAY_MICROS, Kind, ListRow, Listing, Pane, Section, View, conv, is_older, list_key, section_order,
    shows, shows_mention, shows_unread, sidebar_rows,
  };
  use crate::settings::{Features, Layout, ListWindow, Sort};
  use gchat_core::{Group, GroupId, GroupKind, Micros, Notify, Placement};

  #[test]
  fn a_feature_that_is_off_has_no_view_on_screen() {
    let all = Features::default();
    let none = Features {
      calendar: false,
      meet: false,
      mail: false,
    };
    for view in [View::Calendar, View::Meet, View::Mail] {
      assert!(shows(&view, all));
      assert!(!shows(&view, none));
    }
    assert!(shows(&View::Home, none));
    assert!(shows(&View::Shortcut(gchat_core::Shortcut::Mentions), none));
    assert!(shows(&View::Conversation(GroupId::Dm("jpark".into())), none));
    let meet_only = Features { calendar: false, meet: true, mail: false };
    assert!(shows(&View::Meet, meet_only) && !shows(&View::Calendar, meet_only));
  }

  fn group(kind: GroupKind, placement: Placement) -> Group {
    Group {
      id: GroupId::Dm("1".into()),
      kind,
      name: "Ada".into(),
      avatar_url: None,
      emoji: None,
      member_ids: Vec::new(),
      member_avatars: Vec::new(),
      sort_time: 0,
      last_read_time: 0,
      unread: false,
      mentioned: false,
      pinned: false,
      muted: false,
      notify: Notify::All,
      snippet: None,
      placement,
      hidden: false,
      blocked: false,
      can_delete: false,
    }
  }

  #[test]
  fn section_follows_placement_then_kind() {
    assert_eq!(Section::of(&group(GroupKind::Space, Placement::Meeting)), Section::Meetings);
    assert_eq!(
      Section::of(&group(
        GroupKind::Dm,
        Placement::Custom { id: "s".into(), name: "Shortcut".into() }
      )),
      Section::Custom { id: "s".into(), name: "Shortcut".into() }
    );
    assert_eq!(Section::of(&group(GroupKind::Dm, Placement::ByKind)), Section::Dms);
    assert_eq!(Section::of(&group(GroupKind::GroupDm, Placement::ByKind)), Section::Dms);
    assert_eq!(Section::of(&group(GroupKind::App, Placement::ByKind)), Section::Apps);
    assert_eq!(Section::of(&group(GroupKind::Space, Placement::ByKind)), Section::Spaces);
  }

  #[test]
  fn people_sections_sort_alphabetically_by_default() {
    assert_eq!(Section::Dms.default_sort(), Sort::Alphabetical);
    assert_eq!(
      Section::Custom { id: "s".into(), name: "Shortcut".into() }.default_sort(),
      Sort::Alphabetical
    );
    assert_eq!(Section::Spaces.default_sort(), Sort::Recent);
    assert_eq!(Section::Meetings.default_sort(), Sort::Recent);
    assert_eq!(Section::Dms.title(), "Direct messages");
  }

  fn dm(id: &str) -> GroupId {
    GroupId::Dm(id.into())
  }

  fn members(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_string()).collect()
  }

  #[test]
  fn a_call_in_a_chat_is_posted_when_its_meeting_card_shows_there() {
    let call = Calling::new("Jordan Park".into(), "jpark".into(), Some(dm("jpark-chat")));
    assert!(call.is_posted(&dm("jpark-chat"), true));
    assert!(!call.is_posted(&dm("jpark-chat"), false));
    assert!(!call.is_posted(&dm("rbrooks-chat"), true));
  }

  #[test]
  fn a_call_without_a_chat_waits_for_the_chat_made_with_that_person_alone() {
    let mut call = Calling::new("Jordan Park".into(), "jpark".into(), None);
    // Nothing is posted until there is a chat to post it in.
    assert!(!call.is_posted(&dm("jpark-chat"), true));
    // The chat of someone else, and a chat that has them and others, are not the one.
    assert!(!call.take_chat(&dm("rbrooks-chat"), &members(&["rbrooks"])));
    assert!(!call.take_chat(&dm("group-chat"), &members(&["jpark", "rbrooks"])));
    assert!(call.take_chat(&dm("jpark-chat"), &members(&["jpark"])));
    assert!(call.is_posted(&dm("jpark-chat"), true));
    // The call has its chat: it takes no other.
    assert!(!call.take_chat(&dm("jpark-other"), &members(&["jpark"])));
    assert!(!call.is_posted(&dm("jpark-other"), true));
  }

  /// Far enough past the epoch for every age the tests give.
  const NOW: Micros = 365 * DAY_MICROS;

  fn chat(id: &str, kind: GroupKind, days_ago: Micros) -> Group {
    let mut g = group(kind, Placement::ByKind);
    g.id = GroupId::Space(id.into());
    g.name = id.into();
    g.sort_time = NOW - days_ago * DAY_MICROS;
    g
  }

  #[test]
  fn only_quiet_spaces_and_group_chats_are_older() {
    let old = |kind| chat("design-review", kind, 8);
    assert!(is_older(&old(GroupKind::Space), false, ListWindow::Week, NOW));
    assert!(is_older(&old(GroupKind::GroupDm), false, ListWindow::Week, NOW));
    // People and apps stay, however quiet.
    assert!(!is_older(&old(GroupKind::Dm), false, ListWindow::Week, NOW));
    assert!(!is_older(&old(GroupKind::App), false, ListWindow::Week, NOW));
    // A longer window, or All, keeps it.
    assert!(!is_older(&old(GroupKind::Space), false, ListWindow::TwoWeeks, NOW));
    assert!(!is_older(&old(GroupKind::Space), false, ListWindow::All, NOW));
    // Pinned, unread, and open ones stay.
    let mut pinned = old(GroupKind::Space);
    pinned.pinned = true;
    assert!(!is_older(&pinned, false, ListWindow::Week, NOW));
    let mut unread = old(GroupKind::Space);
    unread.unread = true;
    assert!(!is_older(&unread, false, ListWindow::Week, NOW));
    assert!(!is_older(&old(GroupKind::Space), true, ListWindow::Week, NOW));
  }

  #[test]
  fn older_means_more_than_the_window_ago() {
    let mut space = chat("design-review", GroupKind::Space, 7);
    assert!(!is_older(&space, false, ListWindow::Week, NOW));
    space.sort_time -= 1;
    assert!(is_older(&space, false, ListWindow::Week, NOW));
  }

  fn list() -> Vec<Arc<Conv>> {
    [
      chat("launch", GroupKind::Space, 1),
      chat("garden-club", GroupKind::GroupDm, 40),
      chat("riley-brooks", GroupKind::Dm, 60),
      chat("archive", GroupKind::Space, 90),
    ]
    .into_iter()
    .map(|g| Arc::new(conv(g, "")))
    .collect()
  }

  fn names<'a>(convs: impl Iterator<Item = &'a Arc<Conv>>) -> Vec<&'a str> {
    convs.map(|conv| conv.g.name.as_str()).collect()
  }

  #[test]
  fn the_list_holds_older_ones_behind_a_row_until_it_is_clicked() {
    let mut listing = Listing::default();
    listing.set(list(), false, ListWindow::Week, None, NOW);
    assert_eq!(names(listing.visible()), ["launch", "riley-brooks"]);
    assert_eq!(listing.rows(), 3);
    assert!(matches!(listing.row(2), Some(ListRow::Older)));
    assert!(listing.row(3).is_none());
    assert_eq!(listing.position(&GroupId::Space("archive".into())), None);

    listing.show_older = true;
    assert_eq!(names(listing.visible()), ["launch", "riley-brooks", "garden-club", "archive"]);
    assert_eq!(listing.rows(), 5);
    assert!(matches!(listing.row(3), Some(ListRow::Conv(conv)) if conv.g.name == "garden-club"));
    assert_eq!(listing.position(&GroupId::Space("archive".into())), Some(4));

    // Clicked stays clicked when the list is built again.
    listing.set(list(), false, ListWindow::Week, None, NOW);
    assert_eq!(listing.rows(), 5);
  }

  #[test]
  fn the_open_one_and_a_search_hold_nothing_back() {
    let mut listing = Listing::default();
    listing.set(list(), false, ListWindow::Week, Some(&GroupId::Space("archive".into())), NOW);
    assert_eq!(names(listing.visible()), ["launch", "riley-brooks", "archive"]);
    listing.set(list(), true, ListWindow::Week, None, NOW);
    assert_eq!(names(listing.visible()), ["launch", "garden-club", "riley-brooks", "archive"]);
    assert_eq!(listing.rows(), 4);
    assert!(listing.older.is_empty());
  }

  #[test]
  fn sections_and_shortcuts_fit_only_the_rail() {
    for layout in Layout::ALL {
      assert!(Pane::All.fits(layout));
      assert!(Pane::Kind(Kind::Spaces).fits(layout));
      let rail = layout == Layout::Rail;
      assert_eq!(Pane::Section(Section::Dms).fits(layout), rail);
      assert_eq!(Pane::Shortcut(gchat_core::Shortcut::Mentions).fits(layout), rail);
    }
  }

  #[test]
  fn a_muted_conversation_shows_as_unread_only_once_it_mentions_the_user() {
    let mut g = group(GroupKind::Space, Placement::ByKind);
    assert!(!shows_unread(&g) && !shows_mention(&g));
    g.unread = true;
    assert!(shows_unread(&g) && !shows_mention(&g));
    g.muted = true;
    assert!(!shows_unread(&g) && !shows_mention(&g));
    g.mentioned = true;
    assert!(shows_unread(&g) && shows_mention(&g));
    g.muted = false;
    assert!(shows_unread(&g) && shows_mention(&g));
    g.unread = false;
    assert!(!shows_unread(&g) && !shows_mention(&g));
  }

  #[test]
  fn a_muted_unread_space_is_older_unless_it_mentions_the_user() {
    let mut space = chat("design-review", GroupKind::Space, 8);
    space.unread = true;
    space.muted = true;
    assert!(is_older(&space, false, ListWindow::Week, NOW));
    space.mentioned = true;
    assert!(!is_older(&space, false, ListWindow::Week, NOW));
  }

  #[test]
  fn a_section_past_its_first_rows_keeps_its_unread_and_open_conversations() {
    // Twelve spaces: the tenth unread, the twelfth unread but muted.
    let convs: Vec<Arc<Conv>> = (0..12)
      .map(|ix| {
        let mut g = chat(&format!("space-{ix}"), GroupKind::Space, 1);
        g.unread = ix == 9 || ix == 11;
        g.muted = ix == 11;
        Arc::new(conv(g, ""))
      })
      .collect();
    let unpinned: Vec<&Arc<Conv>> = convs.iter().collect();
    let spaces = |ixs: &[usize]| -> Vec<String> { ixs.iter().map(|ix| format!("space-{ix}")).collect() };

    assert_eq!(
      names(sidebar_rows(&unpinned, false, None)),
      spaces(&[0, 1, 2, 3, 4, 5, 6, 7, 9])
    );
    let open = GroupId::Space("space-10".into());
    assert_eq!(
      names(sidebar_rows(&unpinned, false, Some(&open))),
      spaces(&[0, 1, 2, 3, 4, 5, 6, 7, 9, 10])
    );
    assert_eq!(
      names(sidebar_rows(&unpinned, true, None)),
      spaces(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11])
    );
  }

  /// Conversations of `kind`, newest first in the order given; `pinned` and `muted` name which are.
  fn convs(kind: GroupKind, ids: &[&str], pinned: &[&str], muted: &[&str]) -> Vec<Arc<Conv>> {
    ids
      .iter()
      .enumerate()
      .map(|(ix, id)| {
        let mut g = chat(id, kind, Micros::try_from(ix).unwrap_or_default());
        g.pinned = pinned.contains(id);
        g.muted = muted.contains(id);
        Arc::new(conv(g, ""))
      })
      .collect()
  }

  #[test]
  fn a_section_lists_its_muted_conversations_after_the_others_in_its_order() {
    let convs = convs(
      GroupKind::Space,
      &["launch", "design", "budget", "archive", "ops"],
      &["ops"],
      &["design", "archive", "ops"],
    );
    let members = || convs.iter().collect::<Vec<&Arc<Conv>>>();

    let (pinned, unpinned) = section_order(members(), Sort::Recent);
    // A pinned muted one stays with the pinned.
    assert_eq!(names(pinned.into_iter()), ["ops"]);
    assert_eq!(names(unpinned.into_iter()), ["launch", "budget", "design", "archive"]);

    let (_, unpinned) = section_order(members(), Sort::Alphabetical);
    assert_eq!(names(unpinned.into_iter()), ["budget", "launch", "archive", "design"]);
  }

  #[test]
  fn the_irc_list_sinks_each_kinds_muted_conversations_within_that_kind() {
    let mut shown = convs(GroupKind::Dm, &["riley", "jordan", "sam"], &[], &["riley"]);
    shown.extend(convs(
      GroupKind::Space,
      &["launch", "design", "budget", "ops"],
      &["ops"],
      &["launch", "ops"],
    ));
    let mut irc = shown.clone();
    irc.sort_by_key(|conv| list_key(&conv.g, true));
    assert_eq!(
      names(irc.iter()),
      ["ops", "design", "budget", "launch", "jordan", "sam", "riley"]
    );
    // The lists by recency keep muted ones in place, pinned first.
    shown.sort_by_key(|conv| list_key(&conv.g, false));
    assert_eq!(
      names(shown.iter()),
      ["ops", "riley", "jordan", "sam", "launch", "design", "budget"]
    );
  }
}
