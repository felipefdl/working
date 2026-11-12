//! The one entity holding everything the core tells us, plus the precomputed lists the views render.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gchat_core::fuzzy::Fuzzy;
use gchat_core::{
  Command, Connection, Core, Event, Group, GroupId, GroupKind, Message, Notify, Placement, SearchHit, Shortcut,
  SpanKind, User,
};
use gpui_kit::*;

use crate::conversation::ConversationView;
use crate::images::Images;

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

  /// Unpinned rows go A to Z in people sections and newest first elsewhere, as the web app lists them.
  const fn alphabetical(&self) -> bool {
    matches!(self, Self::Custom { .. } | Self::Dms)
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
  Header(Section),
  Conv(Arc<Conv>),
  Toggle(Section),
}

#[derive(Clone, PartialEq, Eq)]
pub enum View {
  Home,
  /// Mentions or Starred: a message list across conversations.
  Shortcut(Shortcut),
  Conversation(GroupId),
}

#[derive(Clone)]
pub struct History {
  pub messages: Arc<Vec<Arc<Message>>>,
  pub reached_start: bool,
  /// False while the list is a window around a search hit rather than the latest messages.
  pub reached_end: bool,
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
  pub sidebar: Vec<SidebarItem>,
  pub view: View,
  pub conv: Option<Entity<ConversationView>>,
  pub query: String,
  /// The message search started by Enter in the search field, shown under the conversation matches.
  pub search: Option<Search>,
  /// What the Mentions or Starred view shows.
  pub shortcut: Option<ShortcutList>,
  pub unread_only: bool,
  collapsed: HashSet<Section>,
  show_all: HashSet<Section>,
  histories: HashMap<GroupId, History>,
  /// Recently opened conversations, newest first.
  pub recent: Vec<GroupId>,
  /// Unread conversations shown on the Dock icon.
  badge: usize,
  /// Notifications still showing, by conversation: taken down when the conversation is read.
  notified: HashMap<GroupId, Vec<String>>,
  /// People the user can start a chat with, name order.
  pub people: Vec<User>,
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
      sidebar: vec![
        SidebarItem::Header(Section::Shortcuts),
        SidebarItem::Home,
        SidebarItem::Shortcut(Shortcut::Mentions),
        SidebarItem::Shortcut(Shortcut::Starred),
      ],
      view: View::Home,
      conv: None,
      query: String::new(),
      search: None,
      shortcut: None,
      unread_only: false,
      // The web app keeps Meetings folded at the bottom.
      collapsed: HashSet::from([Section::Meetings]),
      show_all: HashSet::new(),
      histories: HashMap::new(),
      recent: Vec::new(),
      badge: 0,
      notified: HashMap::new(),
      people: Vec::new(),
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
        Event::SignedOut(reason) => self.signed_out = Some(reason),
        // The call button posts the link. Auto-open then joins it (built-in window or browser).
        Event::MeetStarted(url) => {
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
        Event::OpenChat(id) => self.open(id, window, cx),
        Event::Error(message) => tracing::warn!("core error: {message}"),
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
    self.rebuild_home();
  }

  pub fn rebuild_home(&mut self) {
    let shown = self.all.iter().filter(|conv| !self.unread_only || conv.g.unread).cloned();
    let query = self.query.trim();
    self.home = if query.is_empty() {
      shown.collect()
    } else {
      Fuzzy::new(query)
        .rank(shown, |conv| conv.g.name.as_str())
        .into_iter()
        .map(|(_, conv)| conv)
        .collect()
    };
  }

  pub fn rebuild_sidebar(&mut self) {
    let mut items = vec![SidebarItem::Header(Section::Shortcuts)];
    if !self.collapsed.contains(&Section::Shortcuts) {
      items.push(SidebarItem::Home);
      items.push(SidebarItem::Shortcut(Shortcut::Mentions));
      items.push(SidebarItem::Shortcut(Shortcut::Starred));
    }
    let mut by_section: HashMap<Section, Vec<&Arc<Conv>>> = HashMap::new();
    for conv in &self.all {
      by_section.entry(Section::of(&conv.g)).or_default().push(conv);
    }
    let mut custom: Vec<Section> = by_section
      .keys()
      .filter(|section| matches!(section, Section::Custom { .. }))
      .cloned()
      .collect();
    custom.sort_by_key(|section| section.title().to_lowercase());
    let order = custom
      .into_iter()
      .chain([Section::Dms, Section::Apps, Section::Spaces, Section::Meetings]);
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
      let key = |c: &Arc<Conv>| c.lower.trim_start_matches("group: ").to_string();
      pinned.sort_by_key(|c| key(c));
      if section.alphabetical() {
        unpinned.sort_by_key(|c| key(c));
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
    self.rebuild_home();
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
    self.rebuild_home();
    if !self.query.is_empty() && self.view != View::Home {
      self.go_home(cx);
    }
    cx.notify();
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
      self.rebuild_home();
    }
    self.recent.retain(|r| r != &id);
    self.recent.insert(0, id.clone());
    self.recent.truncate(50);
    // A window left by a jump is not what opening shows: the core answers with the latest messages.
    let history = self.histories.get(&id).filter(|history| history.reached_end).cloned();
    let store = cx.weak_entity();
    let (core, images, me) = (self.core.clone(), self.images.clone(), self.me.clone());
    self.conv = Some(cx.new(|cx| ConversationView::new(store, core, images, group, me, history, window, cx)));
    self.view = View::Conversation(id.clone());
    self.shortcut = None;
    self.core.send(Command::Open(id.clone()));
    self.clear_notifications(&id);
    self.core.send(Command::MarkRead(id));
    cx.notify();
  }

  pub fn go_home(&mut self, cx: &mut Context<Self>) {
    if self.view != View::Home {
      self.core.send(Command::Close);
    }
    self.view = View::Home;
    // Dropping the view tells the core we stopped typing.
    self.conv = None;
    self.shortcut = None;
    cx.notify();
  }

  /// Shows Mentions or Starred, loading its newest messages again.
  pub fn open_shortcut(&mut self, shortcut: Shortcut, cx: &mut Context<Self>) {
    if matches!(self.view, View::Conversation(_)) {
      self.core.send(Command::Close);
    }
    self.view = View::Shortcut(shortcut);
    self.conv = None;
    self.shortcut = Some(ShortcutList {
      shortcut,
      hits: None,
      cursor: None,
      loading_more: false,
    });
    self.core.send(Command::LoadShortcut { shortcut, cursor: None });
    cx.notify();
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
      self.view = View::Conversation(hit.group.clone());
      self.shortcut = None;
    }
    self.core.send(Command::Jump {
      group: hit.group.clone(),
      message_id: hit.message_id.clone(),
      time: hit.time,
    });
    cx.notify();
  }
}

fn conv(group: Group) -> Conv {
  let lower = group.name.to_lowercase();
  Conv { g: group, lower }
}

#[cfg(test)]
mod tests {
  use super::Section;
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
  fn people_sections_sort_alphabetically() {
    assert!(Section::Dms.alphabetical());
    assert!(Section::Custom { id: "s".into(), name: "Shortcut".into() }.alphabetical());
    assert!(!Section::Spaces.alphabetical());
    assert!(!Section::Meetings.alphabetical());
    assert_eq!(Section::Dms.title(), "Direct messages");
  }
}
