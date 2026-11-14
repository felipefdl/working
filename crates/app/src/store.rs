//! The one entity holding everything the core tells us, plus the precomputed lists the views render.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gchat_core::fuzzy::Fuzzy;
use gchat_core::{
  AttachmentKind, Calendar, CalendarEvent, Command, Connection, Core, Event, Group, GroupId, GroupKind, Message,
  Micros, Notify, Placement, ReadReceipt, SearchHit, Shortcut, SpanKind, User,
};
use gpui_kit::*;

use crate::conversation::ConversationView;
use crate::images::Images;
use crate::settings::{Layout, Settings, Sort};

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

  fn of(group: &Group) -> Self {
    match (&group.placement, group.kind) {
      (Placement::Custom { id, name }, _) => Self::Custom { id: id.clone(), name: name.clone() },
      (Placement::Meeting, _) => Self::Meetings,
      (Placement::ByKind, GroupKind::Dm | GroupKind::GroupDm) => Self::Dms,
      (Placement::ByKind, GroupKind::App) => Self::Apps,
      (Placement::ByKind, GroupKind::Space) => Self::Spaces,
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

/// A group with its lowercase name precomputed for filtering.
pub struct Conv {
  pub g: Group,
  pub lower: String,
}

#[derive(Clone)]
pub enum SidebarItem {
  Home,
  Shortcut(Shortcut),
  /// Google Calendar, under the shortcuts.
  Calendar,
  /// Google Meet, under Calendar.
  Meet,
  Header(Section),
  Conv(Arc<Conv>),
  Toggle(Section),
}

/// What the Messages chips and the Rail select: the conversations listed beside the open one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pane {
  All,
  Unread,
  /// People, group chats, spaces, or apps, wherever the sidebar places them.
  Kind(Kind),
  Section(Section),
  /// Mentions or Starred: the list shows the shortcut's messages (`Store::shortcut`) instead of conversations.
  Shortcut(Shortcut),
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
}

/// One Rail entry: a pane and how many of its conversations are unread.
#[derive(Clone)]
pub struct Tab {
  pub pane: Pane,
  pub unread: usize,
}

/// Steps Back can go, at most.
const HISTORY_LIMIT: usize = 50;

#[derive(Clone, PartialEq, Eq)]
pub enum View {
  Home,
  /// Mentions or Starred: a message list across conversations.
  Shortcut(Shortcut),
  /// Google Calendar.
  Calendar,
  /// Google Meet.
  Meet,
  Conversation(GroupId),
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

const COLLAPSED_ROWS: usize = 8;

pub struct Store {
  pub core: Core,
  pub images: Images,
  pub me: Option<User>,
  pub connection: Connection,
  pub signed_out: Option<String>,
  groups: HashMap<GroupId, Arc<Conv>>,
  /// Every conversation by `sort_time` descending.
  pub all: Vec<Arc<Conv>>,
  /// What Home shows: `all` narrowed by the Unread toggle and, best match first, by the search text.
  pub home: Vec<Arc<Conv>>,
  /// What the Messages and Rail list shows: `all` narrowed by `pane`, the Unread toggle, and the search text,
  /// pinned first.
  pub list: Vec<Arc<Conv>>,
  pub pane: Pane,
  /// Every Rail entry: All, Mentions, Starred, the user's own sections, each kind the account has, then Meetings.
  pub tabs: Vec<Tab>,
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
  pub unread_only: bool,
  /// Copies of `Settings::list_sort`, `Settings::section_sort`, and the `show_*` switches of the Mentions, Starred,
  /// Calendar, and Meet buttons, taken by `sync_settings`.
  pub list_sort: Sort,
  section_sort: std::collections::BTreeMap<String, Sort>,
  show_mentions: bool,
  show_starred: bool,
  show_calendar: bool,
  show_meet: bool,
  collapsed: HashSet<Section>,
  show_all: HashSet<Section>,
  histories: HashMap<GroupId, History>,
  /// Who has read the open conversation. Each event for that conversation replaces it, and it is dropped as soon as
  /// another view takes the conversation's place: the core sends it again whenever a conversation opens.
  receipts: Option<OpenReceipts>,
  /// Recently opened conversations, newest first.
  pub recent: Vec<GroupId>,
  /// Unread conversations shown on the Dock icon.
  badge: usize,
  /// Notifications still showing, by conversation: taken down when the conversation is read.
  notified: HashMap<GroupId, Vec<String>>,
  /// People the user can start a chat with, name order.
  pub people: Vec<User>,
  pub calendar: CalendarData,
  pub meet: MeetData,
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
      all: Vec::new(),
      home: Vec::new(),
      list: Vec::new(),
      pane: Pane::All,
      tabs: Vec::new(),
      sidebar: vec![
        SidebarItem::Header(Section::Shortcuts),
        SidebarItem::Home,
        SidebarItem::Shortcut(Shortcut::Mentions),
        SidebarItem::Shortcut(Shortcut::Starred),
        SidebarItem::Calendar,
        SidebarItem::Meet,
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
      section_sort: std::collections::BTreeMap::new(),
      show_mentions: true,
      show_starred: true,
      show_calendar: true,
      show_meet: true,
      // The web app keeps Meetings folded at the bottom.
      collapsed: HashSet::from([Section::Meetings]),
      show_all: HashSet::new(),
      histories: HashMap::new(),
      receipts: None,
      recent: Vec::new(),
      badge: 0,
      notified: HashMap::new(),
      people: Vec::new(),
      calendar: CalendarData::default(),
      meet: MeetData::default(),
    }
  }

  pub fn me_id(&self) -> String {
    self.me.as_ref().map(|u| u.id.clone()).unwrap_or_default()
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

  /// Applies a drained batch of core events, then notifies once.
  pub fn apply(&mut self, events: Vec<Event>, window: &mut Window, cx: &mut Context<Self>) {
    let mut groups_dirty = false;
    for event in events {
      match event {
        Event::Me(user) => {
          self.me = Some(user.clone());
          if let Some(conv) = &self.conv {
            conv.update(cx, |conv, cx| conv.set_me(user, cx));
          }
        },
        Event::Connection(connection) => {
          self.signed_out = None;
          // The schedules can change in Google Chat while this was away.
          if connection == Connection::Online && self.connection != Connection::Online {
            self.core.send(Command::LoadDnd);
          }
          self.connection = connection;
        },
        Event::World(list) => {
          self.groups = list.iter().map(|g| (g.id.clone(), Arc::new(conv(g.clone())))).collect();
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
          self.groups.insert(group.id.clone(), Arc::new(conv(group)));
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
          }
        },
        Event::MessageUpsert { message, replaces, incoming } => {
          self.call_posted(&message);
          if incoming {
            self.notify_incoming(&message, window, cx);
          }
          self.upsert_message(message, replaces, window, cx);
        },
        Event::MessageDeleted { group, id } => {
          if let Some(history) = self.histories.get_mut(&group) {
            let list = Arc::make_mut(&mut history.messages);
            list.retain(|m| m.id != id);
            let snapshot = history.clone();
            if self.is_open(&group)
              && let Some(view) = &self.conv
            {
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
        },
        // The call button posts the link. Auto-open then joins it (built-in window or browser).
        Event::MeetStarted(url) => {
          self.meet.calling = None;
          if cx.global::<crate::settings::Settings>().meet_auto_open {
            crate::call::open(&url, cx);
          }
        },
        Event::Mcp(status) => cx.set_global(crate::settings::McpState(status)),
        Event::Dnd(schedules) => cx.set_global(crate::status::DndState(Some(schedules))),
        Event::Mine(status) => {
          cx.set_global(crate::status::Mine(status));
          cx.notify();
        },
        Event::Statuses(statuses) => {
          let statuses = crate::ui::People(statuses.into_iter().collect());
          if cx.try_global::<crate::ui::People>() != Some(&statuses) {
            cx.set_global(statuses);
            cx.refresh_windows();
          }
        },
        Event::People(people) => self.people = people,
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
        },
        Event::Calendars(calendars) => self.calendar.calendars = calendars,
        Event::CalendarEvents { start, end, events } => {
          // Each view keeps the range it asked for last. An answer for a range neither is on any more is dropped.
          let range = Some((start, end));
          if self.meet.range == range {
            self.meet.events = events.clone();
            self.meet.loaded = true;
            self.calendar.error = None;
          }
          if self.calendar.range == range {
            self.calendar.events = events;
            self.calendar.loaded = true;
            self.calendar.error = None;
          }
        },
        Event::CalendarWritten(result) => {
          self.calendar.writing = false;
          self.calendar.written = Some(result);
        },
        Event::CalendarFailed(message) => {
          tracing::warn!("calendar: {message}");
          self.calendar.error = Some(message);
        },
        Event::MeetingCreated(result) => match (result, self.meet.creating.take()) {
          (Ok(url), Some(Creating::Instant)) => crate::call::open(&url, cx),
          (Ok(url), Some(Creating::ForLater)) => self.meet.made = Some(url),
          (Err(message), _) => self.meet.error = Some(message),
          // An answer nobody asked for.
          (Ok(_), None) => {},
        },
        Event::Error(message) => {
          tracing::warn!("core error: {message}");
          // The call that was being started failed: the Meet view says why.
          if self.meet.calling.take().is_some() {
            self.meet.error = Some(message);
          }
        },
      }
    }
    if groups_dirty {
      self.rebuild();
    }
    // The Dock number counts what asked for attention: unread conversations set to notify for everything,
    // and the others only while a notification Working posted for them (a mention) is still unread.
    let badge = self
      .groups
      .values()
      .filter(|c| c.g.unread)
      .filter(|c| match c.g.notify {
        Notify::All | Notify::MainConversations => true,
        Notify::Mentions => self.notified.contains_key(&c.g.id),
        Notify::Off => false,
      })
      .count();
    if badge != self.badge {
      self.badge = badge;
      crate::notify::set_badge(badge);
    }
    cx.notify();
  }

  fn is_open(&self, group: &GroupId) -> bool {
    matches!(&self.view, View::Conversation(open) if open == group)
  }

  /// Opens the conversation a clicked notification belongs to.
  pub fn open_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
    if let Some(id) = self.groups.keys().find(|id| id.key() == key).cloned() {
      self.clear_notifications(&id);
      self.open(id, window, cx);
    }
  }

  fn clear_notifications(&mut self, group: &GroupId) {
    if let Some(ids) = self.notified.remove(group) {
      crate::notify::clear(&ids);
    }
  }

  /// A macOS notification for someone else's new message, when the conversation's Google Chat setting asks
  /// for one and the user is not already looking at it.
  fn notify_incoming(&mut self, message: &Message, window: &Window, cx: &App) {
    let Some(conv) = self.groups.get(&message.group) else {
      return;
    };
    let me = self.me_id();
    let mentioned = message.spans.iter().any(|span| {
      matches!(&span.kind, SpanKind::MentionAll) || matches!(&span.kind, SpanKind::Mention(id) if *id == me)
    });
    let wanted = match conv.g.notify {
      Notify::All => true,
      Notify::MainConversations => mentioned || !message.is_thread_reply(),
      Notify::Mentions => mentioned,
      Notify::Off => false,
    };
    if !wanted || (window.is_window_active() && self.is_open(&message.group)) {
      return;
    }
    // Inside a do-not-disturb schedule, or while the status button has do not disturb on, Google Chat
    // stays quiet, and so does this.
    let scheduled = cx
      .global::<crate::status::DndState>()
      .0
      .as_ref()
      .is_some_and(crate::status::muted_now);
    let manual = cx
      .try_global::<crate::status::Mine>()
      .is_some_and(|mine| mine.0.dnd_on(crate::status_menu::now_usec()));
    if scheduled || manual {
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
    let body = if message.text.trim().is_empty() {
      "Sent an attachment"
    } else {
      message.text.as_str()
    };
    let sound = cx.global::<crate::settings::Settings>().sound;
    let space = (!direct).then_some(conv.g.name.as_str());
    crate::notify::post(&message.id, &message.group.key(), &message.sender_name, space, body, sound);
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
    if self.is_open(&group)
      && let Some(view) = &self.conv
    {
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
    let mut all: Vec<Arc<Conv>> = self.groups.values().cloned().collect();
    all.sort_by(|a, b| b.g.sort_time.cmp(&a.g.sort_time).then_with(|| a.g.name.cmp(&b.g.name)));
    self.all = all;
    self.rebuild_sidebar();
    self.rebuild_lists();
  }

  /// Home, the Messages and Rail list, and the Rail tabs.
  fn rebuild_lists(&mut self) {
    let query = self.query.trim();
    let rank = |shown: Vec<Arc<Conv>>| -> Vec<Arc<Conv>> {
      if query.is_empty() {
        shown
      } else {
        Fuzzy::new(query)
          .rank(shown, |conv| conv.g.name.as_str())
          .into_iter()
          .map(|(_, conv)| conv)
          .collect()
      }
    };
    let unread_only = self.unread_only;
    let alphabetical = query.is_empty() && self.list_sort == Sort::Alphabetical;
    let by_name = |list: &mut Vec<Arc<Conv>>| {
      if alphabetical {
        list.sort_by(|a, b| name_key(a).cmp(name_key(b)));
      }
    };
    let pane = &self.pane;
    let in_pane = |conv: &Arc<Conv>| match pane {
      Pane::All => true,
      Pane::Unread => conv.g.unread,
      Pane::Kind(kind) => Kind::of(conv.g.kind) == *kind,
      Pane::Section(section) => Section::of(&conv.g) == *section,
      Pane::Shortcut(_) => false,
    };
    // Home takes the Unread and kind filters; sections and shortcuts belong to the Rail.
    let in_home = |conv: &Arc<Conv>| match pane {
      Pane::Unread | Pane::Kind(_) => in_pane(conv),
      Pane::All | Pane::Section(_) | Pane::Shortcut(_) => true,
    };
    self.home = rank(
      self
        .all
        .iter()
        .filter(|conv| (!unread_only || conv.g.unread) && in_home(conv))
        .cloned()
        .collect(),
    );
    by_name(&mut self.home);

    let mut list = rank(
      self
        .all
        .iter()
        .filter(|conv| (!unread_only || conv.g.unread) && in_pane(conv))
        .cloned()
        .collect(),
    );
    by_name(&mut list);
    // Stable: pinned first, in the chosen order within each. Search results keep best match first.
    if query.is_empty() {
      list.sort_by_key(|conv| !conv.g.pinned);
    }
    self.list = list;

    let mut by_section: HashMap<Section, usize> = HashMap::new();
    let mut by_kind: HashMap<Kind, usize> = HashMap::new();
    for conv in &self.all {
      let unread = usize::from(conv.g.unread);
      *by_section.entry(Section::of(&conv.g)).or_default() += unread;
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
    let sections = ordered(by_section.keys());
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
  }

  pub fn rebuild_sidebar(&mut self) {
    let mut items = vec![SidebarItem::Header(Section::Shortcuts)];
    if !self.collapsed.contains(&Section::Shortcuts) {
      items.push(SidebarItem::Home);
      if self.show_mentions {
        items.push(SidebarItem::Shortcut(Shortcut::Mentions));
      }
      if self.show_starred {
        items.push(SidebarItem::Shortcut(Shortcut::Starred));
      }
      if self.show_calendar {
        items.push(SidebarItem::Calendar);
      }
      if self.show_meet {
        items.push(SidebarItem::Meet);
      }
    }
    let mut by_section: HashMap<Section, Vec<&Arc<Conv>>> = HashMap::new();
    for conv in &self.all {
      by_section.entry(Section::of(&conv.g)).or_default().push(conv);
    }
    let order = ordered(by_section.keys());
    for section in order {
      // `all` is newest first, so each section starts in that order.
      let Some(members) = by_section.remove(&section) else {
        continue;
      };
      items.push(SidebarItem::Header(section.clone()));
      if self.collapsed.contains(&section) {
        continue;
      }
      let (mut pinned, mut unpinned): (Vec<&Arc<Conv>>, Vec<&Arc<Conv>>) =
        members.into_iter().partition(|c| c.g.pinned);
      // "Group: Ana" sorts under A, as in the web app.
      pinned.sort_by(|a, b| name_key(a).cmp(name_key(b)));
      if self.section_sort(&section) == Sort::Alphabetical {
        unpinned.sort_by(|a, b| name_key(a).cmp(name_key(b)));
      }
      items.extend(pinned.into_iter().map(|c| SidebarItem::Conv(c.clone())));
      let shown = if self.show_all.contains(&section) {
        unpinned.len()
      } else {
        unpinned.len().min(COLLAPSED_ROWS)
      };
      items.extend(unpinned.iter().take(shown).map(|c| SidebarItem::Conv((*c).clone())));
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
  /// layout used it. Call when the pane or the layout changes.
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
      Layout::Messages | Layout::Rail => self.list.iter().collect(),
    }
  }

  /// The order of a Google Chat sidebar section: the user's pick, or the section's default.
  pub fn section_sort(&self, section: &Section) -> Sort {
    self
      .section_sort
      .get(&section.key())
      .copied()
      .unwrap_or_else(|| section.default_sort())
  }

  /// Takes the sort orders and the Mentions, Starred, Calendar, and Meet buttons from the settings, rebuilding the
  /// lists when they changed. Call at start and whenever the settings change.
  pub fn sync_settings(&mut self, cx: &mut Context<Self>) {
    let settings = cx.global::<Settings>();
    if settings.list_sort == self.list_sort
      && settings.section_sort == self.section_sort
      && settings.show_mentions == self.show_mentions
      && settings.show_starred == self.show_starred
      && settings.show_calendar == self.show_calendar
      && settings.show_meet == self.show_meet
    {
      return;
    }
    self.list_sort = settings.list_sort;
    self.section_sort = settings.section_sort.clone();
    self.show_mentions = settings.show_mentions;
    self.show_starred = settings.show_starred;
    self.show_calendar = settings.show_calendar;
    self.show_meet = settings.show_meet;
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
      View::Home | View::Shortcut(_) | View::Calendar | View::Meet => None,
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

  /// Opens the newest unread conversation other than the open one.
  pub fn open_next_unread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let open = match &self.view {
      View::Conversation(id) => Some(id),
      View::Home | View::Shortcut(_) | View::Calendar | View::Meet => None,
    };
    let Some(id) = self
      .all
      .iter()
      .find(|conv| conv.g.unread && Some(&conv.g.id) != open)
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
      self.rebuild_lists();
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
    self.drop_shortcut(cx);
    self.core.send(Command::Open(id.clone()));
    self.clear_notifications(&id);
    self.core.send(Command::MarkRead(id));
    cx.notify();
  }

  /// Switches the view, recording the one it leaves as a step Back can return to.
  fn set_view(&mut self, view: View) {
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
  /// gone (left, or deleted) are skipped.
  fn next_step(&mut self, back: bool) -> Option<View> {
    let stack = if back {
      &mut self.back
    } else {
      &mut self.forward
    };
    while let Some(view) = stack.pop() {
      let shown = match &view {
        View::Conversation(id) => self.groups.contains_key(id),
        View::Home | View::Shortcut(_) | View::Calendar | View::Meet => true,
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
    if matches!(self.view, View::Conversation(_)) {
      self.core.send(Command::Close);
    }
    self.set_view(View::Meet);
    self.conv = None;
    self.drop_shortcut(cx);
    self.meet.error = None;
    cx.notify();
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
  /// answers `MeetStarted` after it, but an answer that never comes must not leave the call waiting.
  fn call_posted(&mut self, message: &Message) {
    let card = message
      .attachments
      .iter()
      .any(|attachment| attachment.kind == AttachmentKind::Meet);
    if self
      .meet
      .calling
      .as_ref()
      .is_some_and(|call| call.is_posted(&message.group, card))
    {
      self.meet.calling = None;
    }
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

fn conv(group: Group) -> Conv {
  let lower = group.name.to_lowercase();
  Conv { g: group, lower }
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

#[cfg(test)]
mod tests {
  use super::{Calling, Section};
  use crate::settings::Sort;
  use gchat_core::{Group, GroupId, GroupKind, Notify, Placement};

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
      pinned: false,
      muted: false,
      notify: Notify::All,
      snippet: None,
      placement,
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
}
