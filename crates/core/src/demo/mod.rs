//! Demo mode (`WORKING_DEMO=1`): a made-up workspace served from memory, so the app runs without a Google
//! account. It never opens a network connection or the disk cache. Commands answer the way the real engine
//! answers them, and the people in it reply to messages, type, and post now and then.

mod data;
mod image;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::Instant;

use crate::engine::{FetchReply, Fetcher};
use crate::model::{
  Attachment, AttachmentKind, CardAction, Chip, ChipKind, Command, Connection, DndFor, DndSchedule, DndSchedules,
  Event, Group, GroupId, GroupKind, McpConfig, McpStatus, Message, Micros, MyStatus, PersonStatus, Quote, Reaction,
  SearchHit, Shortcut, Snippet, User,
};
use data::{APPROVE, BUILD_BOT, CANCEL, ME, World};

/// Topics shown when a conversation opens, and added per older page, as the engine pages.
const LATEST_PAGE: usize = 40;
const OLDER_PAGE: usize = 30;
/// Most search hits answered at once.
const SEARCH_LIMIT: usize = 50;
/// First unprompted message after start, then the gap between them.
const FIRST_AMBIENT: Duration = Duration::from_secs(12);
const AMBIENT_EVERY: Duration = Duration::from_secs(75);

/// Starts the demo core thread and returns a handle for image fetches.
pub fn spawn(commands: flume::Receiver<Command>, events: flume::Sender<Event>) -> Arc<Fetcher> {
  let (requests, fetches) = flume::unbounded();
  std::thread::Builder::new()
    .name("gchat-demo".into())
    .spawn(move || {
      let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap_or_else(|error| {
          tracing::error!("the demo runtime did not start: {error}");
          std::process::exit(1);
        });
      runtime.block_on(run(commands, events, fetches));
    })
    .unwrap_or_else(|error| {
      tracing::error!("the demo thread did not start: {error}");
      std::process::exit(1);
    });
  Arc::new(Fetcher::new(requests))
}

async fn run(
  commands: flume::Receiver<Command>,
  events: flume::Sender<Event>,
  fetches: flume::Receiver<(String, FetchReply)>,
) {
  let mut demo = Demo::new(events, now_micros());
  demo.greet();
  demo.schedule(FIRST_AMBIENT, Action::Ambient);
  loop {
    let next = demo.timeline.iter().map(|(at, _)| *at).min();
    let far = Instant::now() + Duration::from_secs(3600);
    tokio::select! {
      command = commands.recv_async() => match command {
        Ok(command) => demo.handle(command),
        Err(_) => return,
      },
      fetch = fetches.recv_async() => match fetch {
        Ok((url, reply)) => {
          let _ = reply.send(fetch_bytes(&url).map(Arc::new));
        },
        Err(_) => return,
      },
      () = tokio::time::sleep_until(next.unwrap_or(far)), if next.is_some() => demo.run_due(),
    }
  }
}

fn now_micros() -> Micros {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map_or(0, |elapsed| i64::try_from(elapsed.as_micros()).unwrap_or(i64::MAX))
}

/// Bytes for a URL the demo hands out: drawn images, its own files, and files the user attached.
fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
  if let Some(spec) = url.strip_prefix("demo:image/") {
    let mut parts = spec.split('/').map(|part| part.parse::<u32>().ok());
    if let (Some(Some(hue)), Some(Some(width)), Some(Some(height))) = (parts.next(), parts.next(), parts.next()) {
      return Ok(image::gradient(hue, width.min(2400), height.min(2400)));
    }
  }
  if let Some(name) = url.strip_prefix("demo:file/") {
    return Ok(data::file_body(name).into_bytes());
  }
  if let Some(path) = url.strip_prefix("demo:upload/").or_else(|| url.strip_prefix("file://")) {
    return std::fs::read(path).map_err(|error| error.to_string());
  }
  Err("The demo has no network.".into())
}

/// Something the demo does later.
enum Action {
  /// The server accepts a message the user sent: it replaces the pending copy.
  Confirm { local_id: String, message: Message },
  Typing {
    group: GroupId,
    user: String,
    typing: bool,
  },
  /// Someone else's message arrives.
  Arrive(Message),
  /// A message from someone, unprompted, then the next one scheduled.
  Ambient,
}

/// Which slice of a conversation's messages the UI holds.
#[derive(Clone, Copy)]
struct Window {
  start: usize,
  /// `None` while the window reaches the newest message.
  end: Option<usize>,
}

struct Demo {
  events: flume::Sender<Event>,
  me: User,
  users: HashMap<String, User>,
  groups: HashMap<GroupId, Group>,
  histories: HashMap<GroupId, Vec<Arc<Message>>>,
  members: HashMap<GroupId, Vec<String>>,
  windows: HashMap<GroupId, Window>,
  open: Option<GroupId>,
  timeline: Vec<(Instant, Action)>,
  mine: MyStatus,
  dnd: DndSchedules,
  /// Counts ids, canned replies, and unprompted messages.
  serial: usize,
  ambient: usize,
}

impl Demo {
  fn new(events: flume::Sender<Event>, now: Micros) -> Self {
    let World {
      me, users, groups, histories, members, ..
    } = data::world(now);
    Self {
      events,
      me,
      users: users.into_iter().map(|user| (user.id.clone(), user)).collect(),
      groups: groups.into_iter().map(|group| (group.id.clone(), group)).collect(),
      histories,
      members,
      windows: HashMap::new(),
      open: None,
      timeline: Vec::new(),
      mine: MyStatus::default(),
      dnd: DndSchedules {
        match_time_zone: true,
        time_zone: String::new(),
        schedules: vec![DndSchedule {
          days: vec![1, 2, 3, 4, 5],
          start: 18 * 60,
          end: 8 * 60,
          enabled: true,
        }],
      },
      serial: 0,
      ambient: 0,
    }
  }

  fn emit(&self, event: Event) {
    let _ = self.events.send(event);
  }

  fn user(&self, id: &str) -> User {
    self.users.get(id).cloned().unwrap_or_default()
  }

  const fn next_serial(&mut self) -> usize {
    self.serial += 1;
    self.serial
  }

  fn schedule(&mut self, after: Duration, action: Action) {
    self.timeline.push((Instant::now() + after, action));
  }

  /// Everything a fresh sign-in shows.
  fn greet(&self) {
    self.emit(Event::Me(self.me.clone()));
    self.emit(Event::World(Arc::new(self.groups.values().cloned().collect())));
    let mut people: Vec<User> = self
      .users
      .values()
      .filter(|user| !user.is_bot && user.id != ME)
      .cloned()
      .collect();
    people.sort_by_key(|user| user.name.to_lowercase());
    self.emit(Event::People(people));
    self.emit(Event::Statuses(vec![
      ("demo-jordan".into(), PersonStatus::Active),
      ("demo-riley".into(), PersonStatus::Away),
      ("demo-priya".into(), PersonStatus::DoNotDisturb),
      ("demo-sam".into(), PersonStatus::Active),
    ]));
    self.emit(Event::Mine(self.mine.clone()));
    self.emit(Event::Connection(Connection::Online));
  }

  fn run_due(&mut self) {
    let now = Instant::now();
    let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.timeline).into_iter().partition(|(at, _)| *at <= now);
    self.timeline = later;
    for (_, action) in due {
      match action {
        Action::Confirm { local_id, message } => self.upsert(&Arc::new(message), Some(&local_id)),
        Action::Typing { group, user, typing } => {
          let user_name = self.user(&user).name;
          self.emit(Event::Typing { group, user_id: user, user_name, typing });
        },
        Action::Arrive(mut message) => {
          message.time = now_micros();
          message.update_time = message.time;
          self.upsert(&Arc::new(message), None);
        },
        Action::Ambient => self.ambient(),
      }
    }
  }

  fn handle(&mut self, command: Command) {
    match command {
      Command::Open(group) => self.open(group),
      Command::Close => self.open = None,
      Command::LoadOlder(group) => self.load_older(&group),
      Command::LoadNewer(group) => self.load_newer(&group),
      Command::Jump { group, message_id, .. } => self.jump(group, &message_id),
      Command::Search(query) => self.search(query),
      Command::LoadShortcut { shortcut, cursor } => self.shortcut(shortcut, cursor.is_some()),
      Command::Send {
        group,
        topic_id,
        text,
        quote,
        files,
        chips,
      } => {
        let mut draft = self.draft(&group, topic_id.as_deref(), &text, quote.as_deref());
        draft.attachments = files.iter().map(|path| local_attachment(path)).collect();
        self.send(draft, &chips);
      },
      Command::StartMeet(group) => {
        let code = format!("dem-{:04}-mtg", self.next_serial());
        let mut draft = self.draft(&group, None, "", None);
        draft.attachments = vec![data::meet(&code)];
        self.send(draft, &[]);
      },
      Command::Edit { group, message_id, text } => self.edit(&group, &message_id, text),
      Command::Delete { group, message_id } => self.delete(&group, &message_id),
      Command::MarkRead(group) => self.update_group(&group, |group| {
        group.unread = false;
        group.last_read_time = now_micros();
      }),
      Command::ClickCard { group, message_id, card_id, action, .. } => {
        self.click_card(&group, &message_id, &card_id, &action);
      },
      Command::Star { group, message_id, starred, .. } => {
        self.change_message(&group, &message_id, |message| message.starred = starred);
      },
      Command::React { group, message_id, emoji, add, .. } => {
        self.change_message(&group, &message_id, |message| react(&mut message.reactions, &emoji, add));
      },
      Command::LoadMembers(group) => self.load_members(group),
      Command::LoadSlashCommands(group) => {
        let has_bot = self.members.get(&group).is_some_and(|ids| ids.iter().any(|id| id == BUILD_BOT));
        let commands = if has_bot {
          data::commands()
        } else {
          Vec::new()
        };
        self.emit(Event::SlashCommands { group, commands: Arc::new(commands) });
      },
      Command::CreateChat { user_ids } => self.create_chat(user_ids),
      Command::SetMcp(config) => self.emit(Event::Mcp(mcp_status(config.as_ref()))),
      Command::LoadDnd => self.emit(Event::Dnd(self.dnd.clone())),
      Command::SaveDnd(schedules) => {
        self.dnd = schedules;
        self.emit(Event::Dnd(self.dnd.clone()));
      },
      Command::SetDnd(choice) => {
        self.mine.dnd = matches!(choice, DndFor::For(_));
        self.mine.dnd_until = match choice {
          DndFor::Off => None,
          DndFor::For(span) => Some(now_micros() + span),
        };
        self.emit(Event::Mine(self.mine.clone()));
      },
      Command::SetStatus { emoji, text, until } => {
        self.mine.emoji = emoji;
        self.mine.text = text;
        self.mine.status_until = until;
        self.emit(Event::Mine(self.mine.clone()));
      },
      Command::RefreshWorld => self.emit(Event::World(Arc::new(self.groups.values().cloned().collect()))),
      Command::Connect => self.greet(),
      Command::SetTyping { .. } | Command::SetPresence(_) => {},
    }
  }

  // ---- History windows ----

  fn messages(&self, group: &GroupId) -> &[Arc<Message>] {
    self.histories.get(group).map_or(&[], Vec::as_slice)
  }

  /// Moves `index` back to the first message of its topic, so a page never starts inside a thread.
  fn topic_start(&self, group: &GroupId, mut index: usize) -> usize {
    let messages = self.messages(group);
    while index > 0 && messages.get(index).is_some_and(|message| message.is_thread_reply()) {
      index -= 1;
    }
    index
  }

  /// Moves `index` past the replies of the topic before it, so a page never ends inside a thread.
  fn topic_end(&self, group: &GroupId, mut index: usize) -> usize {
    let messages = self.messages(group);
    while messages.get(index).is_some_and(|message| message.is_thread_reply()) {
      index += 1;
    }
    index
  }

  fn emit_history(&self, group: &GroupId, prepended: usize, focus: Option<String>) {
    let messages = self.messages(group);
    let window = self.windows.get(group).copied().unwrap_or(Window { start: 0, end: None });
    let end = window.end.unwrap_or(messages.len()).min(messages.len());
    let start = window.start.min(end);
    self.emit(Event::History {
      group: group.clone(),
      messages: Arc::new(messages.get(start..end).unwrap_or_default().to_vec()),
      reached_start: start == 0,
      reached_end: end == messages.len(),
      prepended,
      focus,
    });
  }

  fn open(&mut self, group: GroupId) {
    let start = self.topic_start(&group, self.messages(&group).len().saturating_sub(LATEST_PAGE));
    self.windows.insert(group.clone(), Window { start, end: None });
    self.emit_history(&group, 0, None);
    self.open = Some(group);
  }

  fn load_older(&mut self, group: &GroupId) {
    let Some(window) = self.windows.get(group).copied() else {
      return;
    };
    let start = self.topic_start(group, window.start.saturating_sub(OLDER_PAGE));
    self.windows.insert(group.clone(), Window { start, ..window });
    self.emit_history(group, window.start - start, None);
  }

  fn load_newer(&mut self, group: &GroupId) {
    let Some(Window { start, end: Some(end) }) = self.windows.get(group).copied() else {
      return;
    };
    let end = self.topic_end(group, end + OLDER_PAGE);
    let end = (end < self.messages(group).len()).then_some(end);
    self.windows.insert(group.clone(), Window { start, end });
    self.emit_history(group, 0, None);
  }

  fn jump(&mut self, group: GroupId, message_id: &str) {
    let Some(index) = self.messages(&group).iter().position(|message| message.id == message_id) else {
      return self.open(group);
    };
    let start = self.topic_start(&group, index.saturating_sub(OLDER_PAGE / 2));
    let end = self.topic_end(&group, index + OLDER_PAGE / 2);
    let end = (end < self.messages(&group).len()).then_some(end);
    self.windows.insert(group.clone(), Window { start, end });
    self.emit_history(&group, 0, Some(message_id.into()));
    self.open = Some(group);
  }

  // ---- Messages ----

  /// Adds or replaces a message and tells the UI, as the engine does: a reply joins its topic, anything new
  /// updates the conversation's snippet and unread state.
  fn upsert(&mut self, message: &Arc<Message>, local_id: Option<&str>) {
    let group = message.group.clone();
    let open = self.open.as_ref() == Some(&group);
    let window = self.windows.get(&group).copied();
    let list = self.histories.entry(group.clone()).or_default();
    let found = list
      .iter()
      .position(|held| held.id == message.id || local_id.is_some_and(|local| held.pending && held.id == local));
    let mut replaces = None;
    let mut shown = true;
    let is_new = found.is_none();
    if let Some(slot) = found.and_then(|index| list.get_mut(index)) {
      if slot.id != message.id {
        replaces = Some(slot.id.clone());
      }
      *slot = message.clone();
    } else {
      let topic_end = (!message.topic_id.is_empty())
        .then(|| list.iter().rposition(|held| held.topic_id == message.topic_id))
        .flatten();
      let index = topic_end.map_or(list.len(), |index| index + 1);
      list.insert(index, message.clone());
      if let Some(Window { start, end: Some(end) }) = window {
        // A window into the past grows only when the message lands inside it.
        shown = index < end;
        let end = if shown { end + 1 } else { end };
        self.windows.insert(group.clone(), Window { start, end: Some(end) });
      }
    }
    if shown {
      let incoming = is_new && !message.pending && message.sender_id != ME;
      self.emit(Event::MessageUpsert {
        message: message.clone(),
        replaces,
        incoming,
      });
    }
    if is_new {
      self.update_group(&group, |group| {
        let text = data::snippet_text(message);
        group.sort_time = group.sort_time.max(message.time);
        group.snippet = Some(Snippet {
          sender_id: message.sender_id.clone(),
          sender_name: message.sender_name.clone(),
          text,
          time: message.time,
        });
        if message.sender_id == ME {
          group.unread = false;
        } else if !open {
          group.unread = true;
        }
      });
    }
  }

  fn update_group(&mut self, id: &GroupId, change: impl FnOnce(&mut Group)) {
    let Some(group) = self.groups.get_mut(id) else {
      return;
    };
    let before = group.clone();
    change(group);
    if *group != before {
      let group = group.clone();
      self.emit(Event::GroupUpsert(group));
    }
  }

  fn held(&self, group: &GroupId, message_id: &str) -> Option<Arc<Message>> {
    self.messages(group).iter().find(|message| message.id == message_id).cloned()
  }

  fn change_message(&mut self, group: &GroupId, message_id: &str, change: impl FnOnce(&mut Message)) {
    let Some(held) = self.held(group, message_id) else {
      return;
    };
    let mut message = (*held).clone();
    change(&mut message);
    if message != *held {
      self.upsert(&Arc::new(message), None);
    }
  }

  /// The user's message as it shows before the server answers. An empty `topic_id` starts a topic.
  fn draft(&self, group: &GroupId, topic_id: Option<&str>, text: &str, quote: Option<&str>) -> Message {
    let mut draft = data::message(group, "", topic_id.unwrap_or_default(), &self.me, now_micros(), text);
    draft.quote = quote.and_then(|id| self.held(group, id)).map(|quoted| Quote {
      sender_name: quoted.sender_name.clone(),
      text: quoted.text.clone(),
    });
    draft.pending = true;
    draft
  }

  /// Shows `pending` at once, confirms it a moment later, and lets someone answer.
  fn send(&mut self, mut pending: Message, chips: &[Chip]) {
    let group = pending.group.clone();
    let local_id = format!("working%{}", self.next_serial());
    pending.id.clone_from(&local_id);
    pending.spans = crate::convert::chip_spans(chips);
    self.upsert(&Arc::new(pending.clone()), None);

    let id = format!("{}-sent-{}", group.key(), self.next_serial());
    let mut sent = pending;
    if sent.topic_id.is_empty() {
      sent.topic_id.clone_from(&id);
    }
    sent.id = id;
    sent.pending = false;
    sent.attachments.iter_mut().for_each(uploaded);
    let (topic, text) = (sent.topic_id.clone(), sent.text.clone());
    self.schedule(Duration::from_millis(350), Action::Confirm { local_id, message: sent });
    self.answer(&group, &topic, &text, chips);
  }

  fn edit(&mut self, group: &GroupId, message_id: &str, text: String) {
    self.change_message(group, message_id, |message| {
      message.text = text;
      message.spans = Vec::new();
      message.edited = true;
      message.update_time = now_micros();
    });
  }

  fn delete(&mut self, group: &GroupId, message_id: &str) {
    let Some(list) = self.histories.get_mut(group) else {
      return;
    };
    let Some(index) = list.iter().position(|message| message.id == message_id) else {
      return;
    };
    list.remove(index);
    if let Some(window) = self.windows.get_mut(group)
      && let Some(end) = window.end.as_mut()
      && index < *end
    {
      *end -= 1;
    }
    self.emit(Event::MessageDeleted {
      group: group.clone(),
      id: message_id.into(),
    });
  }

  // ---- The other people ----

  /// Someone answers what the user sent: the app answers its commands, a DM's person replies, and a person
  /// mentioned in a space replies in the thread.
  fn answer(&mut self, group: &GroupId, topic: &str, text: &str, chips: &[Chip]) {
    let command = chips.iter().find_map(|chip| match &chip.kind {
      ChipKind::Command { name, .. } => Some(name.clone()),
      _ => None,
    });
    let kind = self.groups.get(group).map(|group| group.kind);
    if let Some(name) = command.or_else(|| (kind == Some(GroupKind::App)).then(|| text.trim().to_string())) {
      return self.bot_reply(group, &name);
    }
    let mentioned = chips.iter().find_map(|chip| match &chip.kind {
      ChipKind::Mention { user_id, bot: false } => Some(user_id.clone()),
      _ => None,
    });
    let members = self.members.get(group).cloned().unwrap_or_default();
    let everyone = chips.iter().any(|chip| chip.kind == ChipKind::MentionAll);
    let (who, thread) = match kind {
      Some(GroupKind::Dm | GroupKind::GroupDm) => (mentioned.or_else(|| members.first().cloned()), false),
      Some(GroupKind::Space) => (mentioned.or_else(|| everyone.then(|| members.first().cloned()).flatten()), true),
      _ => (None, false),
    };
    let Some(who) = who.filter(|id| id != ME) else {
      return;
    };
    let reply = canned(self.next_serial());
    let id = format!("{}-reply-{}", group.key(), self.next_serial());
    let topic = if thread {
      topic.to_string()
    } else {
      id.clone()
    };
    let message = data::message(group, &id, &topic, &self.user(&who), 0, reply);
    self.typed(group, &who, message, Duration::from_millis(900));
  }

  /// `message` arrives after its sender types for a moment.
  fn typed(&mut self, group: &GroupId, who: &str, message: Message, after: Duration) {
    let typing = |typing| Action::Typing {
      group: group.clone(),
      user: who.into(),
      typing,
    };
    self.schedule(after, typing(true));
    self.schedule(after + Duration::from_millis(2400), typing(false));
    self.schedule(after + Duration::from_millis(2450), Action::Arrive(message));
  }

  fn bot_reply(&mut self, group: &GroupId, command: &str) {
    let bot = self.user(BUILD_BOT);
    let id = format!("{}-bot-{}", group.key(), self.next_serial());
    let mut message = data::message(group, &id, &id, &bot, 0, "");
    match command.split_whitespace().next().unwrap_or_default() {
      "/status" => message.cards = vec![data::status_card(&format!("{id}-card"))],
      "/deploy" => message.cards = vec![data::deploy_card(&format!("{id}-card"), "1843", true, "")],
      _ => "I answer /status, /deploy, and /help.".clone_into(&mut message.text),
    }
    self.schedule(Duration::from_millis(1200), Action::Arrive(message));
  }

  fn click_card(&mut self, group: &GroupId, message_id: &str, card_id: &str, action: &CardAction) {
    let build = action
      .parameters
      .iter()
      .find_map(|(key, value)| (key == "build").then(|| value.clone()))
      .unwrap_or_default();
    let approved = action.method == APPROVE;
    if !approved && action.method != CANCEL {
      return;
    }
    let outcome = if approved {
      format!("Approved by {}. Deploying…", self.me.name)
    } else {
      format!("Canceled by {}.", self.me.name)
    };
    self.change_message(group, message_id, |message| {
      for card in message.cards.iter_mut().filter(|card| card.id == card_id) {
        *card = data::deploy_card(card_id, &build, false, &outcome);
      }
    });
    if approved {
      let bot = self.user(BUILD_BOT);
      let id = format!("{}-bot-{}", group.key(), self.next_serial());
      let live = data::message(group, &id, &id, &bot, 0, &format!("Build {build} is live in production."));
      self.schedule(Duration::from_secs(4), Action::Arrive(live));
    }
  }

  /// One unprompted message from the rotation, then the next one scheduled.
  fn ambient(&mut self) {
    const LINES: [(&str, &str, &str); 5] = [
      (
        "demo-jordan-dm",
        "demo-jordan",
        "Also, the customer asked if we can demo on Friday.",
      ),
      ("demo-launch", "demo-mei", "Staging is updated with the final copy."),
      ("demo-trio", "demo-riley", "Adapters: USB-C to HDMI, right?"),
      (
        "demo-priya-dm",
        "demo-priya",
        "Quick question about option B when you have a minute.",
      ),
      ("demo-launch", "demo-sam", "Support macros are reviewed and live ✅"),
    ];
    let (group, who, text) = LINES.get(self.ambient % LINES.len()).copied().unwrap_or(LINES[0]);
    self.ambient += 1;
    if let Some(group) = self
      .groups
      .keys()
      .find(|id| matches!(id, GroupId::Dm(id) | GroupId::Space(id) if id == group))
      .cloned()
    {
      let id = format!("{}-ambient-{}", group.key(), self.next_serial());
      let message = data::message(&group, &id, &id, &self.user(who), 0, text);
      self.typed(&group, who, message, Duration::ZERO);
    }
    self.schedule(AMBIENT_EVERY, Action::Ambient);
  }

  // ---- Lookups ----

  fn load_members(&self, group: GroupId) {
    let mut users: Vec<User> = self
      .members
      .get(&group)
      .into_iter()
      .flatten()
      .filter_map(|id| self.users.get(id).cloned())
      .collect();
    users.sort_by_key(|user| user.name.to_lowercase());
    self.emit(Event::Members { group, users: Arc::new(users) });
  }

  fn hit(message: &Message) -> SearchHit {
    SearchHit {
      group: message.group.clone(),
      message_id: message.id.clone(),
      topic_id: message.topic_id.clone(),
      sender_id: message.sender_id.clone(),
      sender_name: message.sender_name.clone(),
      sender_avatar: message.sender_avatar.clone(),
      text: message.text.clone(),
      time: message.time,
    }
  }

  /// Every held message that passes `keep`, newest first.
  fn hits(&self, keep: impl Fn(&Message) -> bool) -> Vec<SearchHit> {
    let mut hits: Vec<SearchHit> = self
      .histories
      .values()
      .flatten()
      .filter(|message| !message.pending && keep(message))
      .map(|message| Self::hit(message))
      .collect();
    hits.sort_by_key(|hit| std::cmp::Reverse(hit.time));
    hits
  }

  fn search(&self, query: String) {
    let needle = query.to_lowercase();
    let mut hits = self.hits(|message| !message.system && message.text.to_lowercase().contains(&needle));
    hits.truncate(SEARCH_LIMIT);
    self.emit(Event::SearchResults { query, hits: Arc::new(hits) });
  }

  fn shortcut(&self, shortcut: Shortcut, appended: bool) {
    let hits = if appended {
      Vec::new()
    } else {
      match shortcut {
        Shortcut::Starred => self.hits(|message| message.starred),
        Shortcut::Mentions => self.hits(|message| {
          message.sender_id != ME
            && message.spans.iter().any(|span| match &span.kind {
              crate::model::SpanKind::Mention(id) => id == ME,
              crate::model::SpanKind::MentionAll => true,
              _ => false,
            })
        }),
      }
    };
    self.emit(Event::Shortcut {
      shortcut,
      hits: Arc::new(hits),
      appended,
      cursor: None,
    });
  }

  fn create_chat(&mut self, user_ids: Vec<String>) {
    let mut ids: Vec<String> = user_ids
      .into_iter()
      .filter(|id| id != ME && self.users.contains_key(id))
      .collect();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
      return self.emit(Event::Error("Pick at least one person.".into()));
    }
    let kind = if ids.len() == 1 {
      GroupKind::Dm
    } else {
      GroupKind::GroupDm
    };
    let existing = self.groups.values().find(|group| {
      let mut members = group.member_ids.clone();
      members.sort();
      group.kind == kind && members == ids
    });
    if let Some(group) = existing {
      return self.emit(Event::OpenChat(group.id.clone()));
    }
    let names: Vec<User> = ids.iter().map(|id| self.user(id)).collect();
    let name = match names.as_slice() {
      [one] => one.name.clone(),
      many => many.iter().map(|user| user.first_name.as_str()).collect::<Vec<_>>().join(", "),
    };
    let id = GroupId::Dm(format!("demo-new-{}", self.next_serial()));
    let group = Group {
      id: id.clone(),
      kind,
      name,
      avatar_url: None,
      emoji: None,
      member_ids: ids.clone(),
      member_avatars: Vec::new(),
      sort_time: now_micros(),
      last_read_time: now_micros(),
      unread: false,
      pinned: false,
      muted: false,
      notify: crate::model::Notify::All,
      snippet: None,
      placement: crate::model::Placement::ByKind,
    };
    self.groups.insert(id.clone(), group.clone());
    self.members.insert(id.clone(), ids);
    self.histories.insert(id.clone(), Vec::new());
    self.emit(Event::GroupUpsert(group));
    self.emit(Event::OpenChat(id));
  }
}

fn mcp_status(config: Option<&McpConfig>) -> McpStatus {
  match config {
    None => McpStatus::Off,
    Some(_) => McpStatus::Failed("Agents are off in demo mode.".into()),
  }
}

/// The signed-in user's reaction toggled on a message.
fn react(reactions: &mut Vec<Reaction>, emoji: &str, add: bool) {
  match (reactions.iter().position(|reaction| reaction.emoji == emoji), add) {
    (Some(index), true) => {
      if let Some(reaction) = reactions.get_mut(index).filter(|reaction| !reaction.mine) {
        reaction.count += 1;
        reaction.mine = true;
      }
    },
    (None, true) => reactions.push(Reaction {
      emoji: emoji.into(),
      count: 1,
      mine: true,
    }),
    (Some(index), false) => {
      if let Some(reaction) = reactions.get_mut(index).filter(|reaction| reaction.mine) {
        reaction.count = reaction.count.saturating_sub(1);
        reaction.mine = false;
      }
      reactions.retain(|reaction| reaction.count > 0);
    },
    (None, false) => {},
  }
}

/// A reply someone sends back, in rotation.
fn canned(serial: usize) -> &'static str {
  const REPLIES: [&str; 6] = [
    "Sounds good, thanks!",
    "On it 👍",
    "Can we talk about this after standup?",
    "Good call. I'll update the doc.",
    "Agreed. Let's ship it.",
    "Looking now, give me five minutes.",
  ];
  REPLIES.get(serial % REPLIES.len()).copied().unwrap_or("👍")
}

/// How a local file shows until it is "uploaded".
fn local_attachment(path: &Path) -> Attachment {
  Attachment {
    kind: AttachmentKind::File,
    name: path
      .file_name()
      .map(|name| name.to_string_lossy().into_owned())
      .unwrap_or_default(),
    mime: mime_guess::from_path(path).first_or_octet_stream().essence_str().to_string(),
    url: format!("file://{}", path.display()),
    thumb_url: None,
    width: None,
    height: None,
  }
}

/// An attached local file after upload: it opens from the disk through `fetch_bytes`, and an image shows inline.
fn uploaded(attachment: &mut Attachment) {
  if attachment.mime.starts_with("image/") {
    attachment.kind = AttachmentKind::Image;
  }
  if let Some(path) = attachment.url.strip_prefix("file://") {
    attachment.url = format!("demo:upload/{path}");
  }
  if attachment.kind == AttachmentKind::Image {
    attachment.thumb_url = Some(attachment.url.clone());
  }
}

#[cfg(test)]
mod tests;
