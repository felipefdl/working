//! The core's brain: owns the state, answers commands, applies stream events, and keeps the caches warm.
#![allow(
  clippy::too_many_lines,
  clippy::too_many_arguments,
  clippy::significant_drop_tightening,
  clippy::as_conversions,
  clippy::indexing_slicing,
  clippy::string_slice,
  clippy::cast_possible_truncation,
  clippy::cast_sign_loss,
  clippy::struct_excessive_bools,
  clippy::needless_update,
  clippy::unused_async_trait_impl,
  clippy::unused_async,
  clippy::unnecessary_sort_by,
  clippy::assigning_clones,
  clippy::single_match_else,
  clippy::needless_pass_by_value,
  clippy::or_fun_call,
  reason = "command and stream dispatch keep state in one engine; protobuf enums are i32"
)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::{Notify as Wake, Semaphore};

use crate::cache::{Cache, HistoryCache, MailCache, WorldCache};
use crate::channel::{Channel, ChannelEvent};
use crate::convert::{self, Users};
use crate::http::{Http, ServiceSession, SignedOut, request_header};
use crate::markup;
use crate::mcp;
use crate::model::{
  Attachment, AttachmentKind, CardAction, Chip, Command, Connection, DndFor, DndSchedule, DndSchedules, EditScope,
  Event, EventDraft, Group, GroupId, GroupKind, MailAddress, MailCompose, MailDraft, MailDraftSent, MailForward,
  MailLabel, MailMessage, MailReply, MailSignature, MailThread, Mailbox, McpConfig, McpStatus, Message, Micros,
  MyStatus, Notify, PersonStatus, Placement, Presence, Quote, Reaction, ReadReceipt, Rsvp, SearchHit, Shortcut,
  Snippet, User,
};
use crate::proto;

/// Topics per history page, as the web client asks.
const LATEST_PAGE: i32 = 40;
const OLDER_PAGE: i32 = 30;
/// The most recent conversations whose history is fetched right after start, so the first click on them is instant.
const PREFETCH_RECENT: usize = 16;
/// Most histories one prefetch asks for: one `list_topics` request each, three at a time. Past what memory holds
/// (`HELD_HISTORIES`), each prefetched history would push an earlier one out of memory again.
const PREFETCH_MAX: usize = HELD_HISTORIES;
const XSRF_MAX_AGE_SECS: u64 = 6 * 3600;
/// Conversations held in memory besides the open one; one used longer ago leaves memory, and the disk cache brings
/// it back when it is opened again.
const HELD_HISTORIES: usize = 24;
/// Topics of a conversation the disk cache keeps: the first screen, as `LATEST_PAGE` asks for it.
const SAVED_TOPICS: usize = 40;
/// Quiet time after the last change before the queued histories are written.
const SAVE_PAUSE: Duration = Duration::from_secs(2);
/// Longest a queued history waits while changes keep coming.
const SAVE_WAIT_MAX: Duration = Duration::from_secs(10);

pub(crate) type FetchReply = flume::Sender<Result<Arc<Vec<u8>>, String>>;

/// Authenticated, disk-cached downloads for the UI, served on the core's runtime.
pub struct Fetcher {
  requests: flume::Sender<(String, FetchReply)>,
}

impl Fetcher {
  /// A fetcher whose requests go to whoever holds the other end.
  pub(crate) const fn new(requests: flume::Sender<(String, FetchReply)>) -> Self {
    Self { requests }
  }

  pub async fn fetch(&self, url: String) -> Result<Arc<Vec<u8>>> {
    let (reply, answer) = flume::bounded(1);
    self.requests.send((url, reply)).map_err(|_| anyhow!("the core stopped"))?;
    answer
      .recv_async()
      .await
      .map_err(|_| anyhow!("the core stopped"))?
      .map_err(|error| anyhow!(error))
  }
}

/// Starts the core thread and returns a handle for image fetches. A message on `stop` (or every sender of it gone)
/// ends the command loop; `done` is dropped once what was queued for the disk cache is written.
pub fn spawn(
  commands: flume::Receiver<Command>,
  events: flume::Sender<Event>,
  stop: flume::Receiver<()>,
  done: flume::Sender<()>,
) -> Arc<Fetcher> {
  let (requests, fetches) = flume::unbounded();
  std::thread::Builder::new()
    .name("gchat-core".into())
    .spawn(move || {
      let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .thread_name("gchat-core-worker")
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
          tracing::error!("the core runtime did not start: {error}");
          std::process::exit(1);
        });
      runtime.block_on(run(commands, events, fetches, stop));
      drop(done);
    })
    .unwrap_or_else(|error| {
      tracing::error!("the core thread did not start: {error}");
      std::process::exit(1);
    });
  Arc::new(Fetcher::new(requests))
}

fn now_micros() -> Micros {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map_or(0, |elapsed| i64::try_from(elapsed.as_micros()).unwrap_or(i64::MAX))
}

fn file_name(path: &Path) -> String {
  path.file_name().map_or_default(|name| name.to_string_lossy().into_owned())
}

/// How a file about to be uploaded shows in its message until the server answers with the real attachment.
fn local_attachment(path: &Path) -> Attachment {
  Attachment {
    kind: AttachmentKind::File,
    name: file_name(path),
    mime: mime_guess::from_path(path).first_or_octet_stream().essence_str().to_string(),
    url: format!("file://{}", path.display()),
    thumb_url: None,
    width: None,
    height: None,
  }
}

#[derive(Default)]
struct History {
  /// Oldest first; a topic's replies follow its first message.
  messages: Vec<Arc<Message>>,
  reached_start: bool,
  /// Sort time of the oldest topic held: where the next older page starts.
  anchor: Micros,
  /// True while the stream has been online since the last fetch, so the list is known complete.
  fresh: bool,
  loading_older: bool,
  /// The list is a window into the past (after a jump to a search hit): newer messages exist on the server.
  detached: bool,
  /// Sort time of the newest topic held while detached: where the next newer page starts.
  newest: Micros,
  loading_newer: bool,
  /// Made by a message being sent before the conversation was loaded: it is not what the disk cache holds, and
  /// opening the conversation reads the cache under it.
  partial: bool,
  /// Sort time of each topic held, by topic id.
  topic_times: HashMap<String, Micros>,
  /// When it was last used, on `State::clock`.
  used: u64,
  /// Loaded from the server since the stream last went offline.
  fetched: bool,
}

impl History {
  /// Loaded from the server and not made out of date by the stream going offline since.
  const fn loaded(&self) -> bool {
    !self.partial && (self.fresh || self.fetched)
  }

  /// What the disk cache keeps: the newest `SAVED_TOPICS` topics, without messages still being sent. A window that
  /// leaves older topics out starts at its oldest topic, so the older pages load again.
  fn saved(&self) -> HistoryCache {
    let messages: Vec<&Arc<Message>> = self.messages.iter().filter(|message| !message.pending).collect();
    let mut topics = 0;
    let mut start = messages.len();
    let mut current: Option<&str> = None;
    for (index, message) in messages.iter().enumerate().rev() {
      let same = !message.topic_id.is_empty() && current == Some(message.topic_id.as_str());
      if !same {
        if topics == SAVED_TOPICS {
          break;
        }
        topics += 1;
        current = Some(message.topic_id.as_str());
      }
      start = index;
    }
    let first = messages.get(start).filter(|_| start > 0);
    HistoryCache {
      reached_start: first.is_none() && self.reached_start,
      anchor: first.map_or(self.anchor, |first| {
        self.topic_times.get(&first.topic_id).copied().unwrap_or(first.time)
      }),
      messages: messages.into_iter().skip(start).cloned().collect(),
    }
  }

  /// The topics held, oldest first, each with its sort time and its messages, without messages still being sent.
  fn topics(&self) -> Vec<(Micros, Vec<Arc<Message>>)> {
    let mut topics: Vec<(Micros, Vec<Arc<Message>>)> = Vec::new();
    for message in self.messages.iter().filter(|message| !message.pending) {
      match topics.last_mut() {
        Some((time, messages))
          if !message.topic_id.is_empty()
            && messages.first().is_some_and(|first| first.topic_id == message.topic_id) =>
        {
          *time = (*time).max(message.time);
          messages.push(message.clone());
        },
        _ => topics.push((message.time, vec![message.clone()])),
      }
    }
    for (index, (time, messages)) in topics.iter_mut().enumerate() {
      let known = messages.first().and_then(|first| self.topic_times.get(&first.topic_id));
      match known {
        Some(known) => *time = *known,
        // The oldest topic held is where the next older page starts.
        None if index == 0 && self.anchor > 0 => *time = self.anchor,
        None => {},
      }
    }
    topics.sort_by_key(|(time, _)| *time);
    topics
  }

  /// Notes the sort time of the topic a new message lands in.
  fn note_topic(&mut self, message: &Message) {
    if !message.topic_id.is_empty() {
      self
        .topic_times
        .entry(message.topic_id.clone())
        .and_modify(|time| *time = (*time).max(message.time))
        .or_insert(message.time);
    }
  }
}

/// Settles `Group::mentioned` after `group` changed: it holds only while the conversation is unread. A held history
/// that is loaded and reaches back past the read cursor decides it alone: a message from someone else, newer than
/// `last_read_time`, that mentions `me`. Short of that, `known` (what the stream told, with the read cursor where it
/// was) stays, and a held message can only add to it.
fn settle_mention(group: &mut Group, known: bool, history: Option<&History>, me: &str) {
  let read = group.last_read_time;
  group.mentioned = group.unread
    && history.map_or(known, |history| {
      let seen = history
        .messages
        .iter()
        .any(|message| message.time > read && message.sender_id != me && message.mentions(me));
      let whole = history.loaded() && !history.detached && (history.reached_start || history.anchor <= read);
      seen || (known && !whole)
    });
}

/// Whether a hit of the Mentions search marks its conversation (`group`, `None` when it is not listed) as mentioned:
/// a message from someone else, in an unread conversation, sent after the conversation's `last_read_time`.
fn seeds_mention(hit: &SearchHit, group: Option<&Group>, me: &str) -> bool {
  hit.sender_id != me && group.is_some_and(|group| group.unread && hit.time > group.last_read_time)
}

/// Which conversations a prefetch loads, in order, at most `PREFETCH_MAX`, leaving out hidden and blocked ones and
/// those `loaded` says are loaded: first the unread ones, newest first, muted ones too (only their history tells
/// whether the unread activity mentions the user, @all included, which the Mentions search does not answer); then
/// the rest of the `PREFETCH_RECENT` most recent, newest first.
fn prefetch_order<'a>(groups: impl IntoIterator<Item = &'a Group>, loaded: impl Fn(&GroupId) -> bool) -> Vec<GroupId> {
  let mut listed: Vec<&Group> = groups.into_iter().filter(|group| !group.hidden && !group.blocked).collect();
  listed.sort_unstable_by(|a, b| b.sort_time.cmp(&a.sort_time).then_with(|| a.id.cmp(&b.id)));
  let unread = listed.iter().filter(|group| group.unread);
  let recent = listed.iter().take(PREFETCH_RECENT).filter(|group| !group.unread);
  unread
    .chain(recent)
    .filter(|group| !loaded(&group.id))
    .take(PREFETCH_MAX)
    .map(|group| group.id.clone())
    .collect()
}

/// One page of messages as converted: oldest topic first, replies after their first message.
#[derive(Default)]
struct Converted {
  messages: Vec<Arc<Message>>,
  /// Sort time of the oldest topic.
  anchor: Micros,
  /// Sort time of each topic, by topic id.
  topic_times: HashMap<String, Micros>,
}

/// What was last published of one conversation's read receipts.
#[derive(PartialEq)]
struct Receipts {
  enabled: bool,
  people: Vec<ReadReceipt>,
}

impl Receipts {
  /// What publishing `set` makes of the receipts `held`, or `None` when it changes nothing. `set` is the whole answer
  /// of loading a conversation's messages; with `merge` it is an event of the stream, which may name only the people
  /// who just read, so it is added to what is held.
  fn updated(held: Option<&Self>, set: &proto::ReadReceiptSet, me: &str, users: &Users, merge: bool) -> Option<Self> {
    let incoming = convert::read_receipts(set, me, users);
    let next = Self {
      enabled: incoming.is_some(),
      people: match (incoming, held) {
        (Some(incoming), Some(held)) if merge => convert::merge_receipts(held.people.clone(), incoming),
        (incoming, _) => incoming.unwrap_or_default(),
      },
    };
    (held != Some(&next)).then_some(next)
  }
}

/// Which page of a conversation to fetch.
#[derive(Clone, Copy)]
pub(crate) enum Page {
  Latest,
  Before(Micros),
  After(Micros),
  /// The page centered on this time: the topics up to it, and as many after it.
  At(Micros),
}

#[derive(Default)]
struct State {
  me: Option<User>,
  users: Arc<Users>,
  items: HashMap<GroupId, proto::WorldItemLite>,
  groups: HashMap<GroupId, Group>,
  histories: HashMap<GroupId, History>,
  /// Ticks on every use of a history, to find the one used longest ago.
  clock: u64,
  open: Option<GroupId>,
  connection: Option<Connection>,
  world_refresh_queued: bool,
  /// Names of the user's own sidebar sections, by section id.
  sections: HashMap<String, String>,
  /// Section ids already looked up on the web app's page, found or not: each is read once a session.
  section_lookups: HashSet<String>,
  /// The users changed since `Event::People` was last sent.
  people_stale: bool,
  /// Conversations whose history prefetch is queued or running.
  prefetching: HashSet<GroupId>,
  /// Who has read each conversation, as last published to the UI.
  receipts: HashMap<GroupId, Receipts>,
}

impl State {
  /// Marks a held history as just used.
  fn touch(&mut self, group: &GroupId) {
    self.clock += 1;
    if let Some(history) = self.histories.get_mut(group) {
      history.used = self.clock;
    }
  }

  /// The history of `group`, made empty when none is held, marked as just used. Making one lets the history used
  /// longest ago go when more than `HELD_HISTORIES` besides the open one are held.
  fn hold(&mut self, group: &GroupId) -> &mut History {
    if !self.histories.contains_key(group) {
      while self.histories.len() > HELD_HISTORIES {
        let oldest = self
          .histories
          .iter()
          .filter(|(id, _)| self.open.as_ref() != Some(*id))
          .min_by_key(|(_, history)| history.used)
          .map(|(id, _)| id.clone());
        let Some(oldest) = oldest else { break };
        self.histories.remove(&oldest);
      }
    }
    self.clock += 1;
    let clock = self.clock;
    let history = self.histories.entry(group.clone()).or_default();
    history.used = clock;
    history
  }

  /// Adds or updates users; marks the people list stale when one changed.
  fn add_users(&mut self, users: impl IntoIterator<Item = User>) {
    let changed: Vec<User> = users
      .into_iter()
      .filter(|user| self.users.get(&user.id) != Some(user))
      .collect();
    if changed.is_empty() {
      return;
    }
    let held = Arc::make_mut(&mut self.users);
    for user in changed {
      held.insert(user.id.clone(), user);
    }
    self.people_stale = true;
  }

  fn me_id(&self) -> String {
    self.me.as_ref().map_or_default(|me| me.id.clone())
  }
}

pub(crate) struct Engine {
  http: Http,
  channel: Channel,
  cache: Cache,
  events: flume::Sender<Event>,
  state: Mutex<State>,
  /// `WORKING_READONLY=1`: never write to the account (send, read state, reactions, typing).
  readonly: bool,
  /// The MCP server while it runs. Held across a restart, so two changes in a row cannot overlap.
  mcp: tokio::sync::Mutex<Option<mcp::Running>>,
  /// What `Command::SetPresence` last asked for, repeated to the server by `presence_loop`.
  presence: Mutex<Option<Presence>>,
  /// Google Calendar, on the same session.
  calendar: crate::calendar::Client,
  /// The range `Command::LoadEvents` last asked for, read again after a calendar write.
  calendar_range: Mutex<Option<(Micros, Micros)>>,
  /// The event reminder watch while `Command::WatchEvents` has it on.
  event_watch: Mutex<Option<tokio::task::AbortHandle>>,
  /// Wakes the event watch to read the events again after a calendar write.
  events_written: Wake,
  /// Mail writes in the order their commands came, for `mail_writer`, which runs them one at a time.
  mail_writes: tokio::sync::mpsc::UnboundedSender<MailJob>,
  /// Gmail, read only, on the same session.
  mail: crate::mail::Client,
  /// The mail lists held and the inbox watch.
  mail_held: Mutex<MailHeld>,
  /// Conversations waiting for `history_writer`, each as it was last changed.
  unsaved: Mutex<HashMap<GroupId, HistoryCache>>,
  /// Wakes `history_writer` when a history is queued.
  history_queued: Wake,
  /// Held while histories are written, so a later snapshot of a conversation always lands last.
  writing: tokio::sync::Mutex<()>,
  /// Wakes `statuses_loop` when the conversation list is rebuilt.
  groups_changed: Wake,
  /// Wakes the polling loops when a sign-in brings the session back.
  session_restored: Wake,
  /// Room for three history prefetches at a time.
  prefetch_slots: Semaphore,
}

/// One page of a conversation as an agent reads it: topics oldest first, each with its sort time (the cursor
/// for the page before it) and its messages, the first one followed by its replies.
pub(crate) struct TopicPage {
  pub topics: Vec<(Micros, Vec<Arc<Message>>)>,
  pub reached_start: bool,
  pub reached_end: bool,
}

#[derive(Default)]
struct PendingStart {
  mcp: Option<McpConfig>,
  has_mcp: bool,
  presence: Option<Presence>,
}

/// Waits signed out until the app asks to connect. `None` once the app stops the core.
async fn wait_for_session(
  commands: &flume::Receiver<Command>,
  fetches: &flume::Receiver<(String, FetchReply)>,
  stop: &flume::Receiver<()>,
  cache: &Cache,
  events: &flume::Sender<Event>,
) -> Option<(Http, PendingStart)> {
  let mut pending = PendingStart::default();
  loop {
    tokio::select! {
      _ = stop.recv_async() => return None,
      command = commands.recv_async() => {
        let Ok(command) = command else {
          return None;
        };
        match command {
          Command::Connect => match Http::from_cookie_file(&crate::config_dir().join("cookies.json")) {
            Ok(http) => return Some((http, pending)),
            Err(error) => {
              let _ = events.send(Event::SignedOut(error.to_string()));
            },
          },
          Command::SetMcp(config) => {
            pending.has_mcp = true;
            pending.mcp = config;
          },
          Command::SetPresence(presence) => pending.presence = Some(presence),
          Command::CreateMeeting => {
            let _ = events.send(Event::MeetingCreated(Err("Sign in to start.".into())));
          },
          // Every mail command gets its answer, signed out too.
          Command::LoadMail { .. } | Command::OpenMail(_) => {
            let _ = events.send(Event::MailFailed("Sign in to start.".into()));
          },
          Command::MarkMail { .. }
          | Command::ArchiveMail { .. }
          | Command::TrashMail { .. }
          | Command::ReplyMail { .. }
          | Command::ComposeMail { .. } => {
            let _ = events.send(Event::MailWritten(Err("Sign in to start.".into())));
          },
          _ => {},
        }
      },
      fetch = fetches.recv_async() => {
        let Ok((url, reply)) = fetch else {
          return None;
        };
        let _ = reply.send(cache.image(&url).map(Arc::new).ok_or_else(|| "signed out".to_string()));
      },
    }
  }
}

async fn run(
  commands: flume::Receiver<Command>,
  events: flume::Sender<Event>,
  fetches: flume::Receiver<(String, FetchReply)>,
  stop: flume::Receiver<()>,
) {
  let cache = Cache::open();
  tokio::task::spawn_blocking({
    let cache = cache.clone();
    move || cache.trim_images()
  });
  let world = cache.world();
  if let Some(me) = &world.me {
    let _ = events.send(Event::Me(me.clone()));
  }
  if !world.groups.is_empty() {
    let _ = events.send(Event::World(Arc::new(world.groups.clone())));
  }
  if !world.users.is_empty() {
    let me = world.me.as_ref().map_or("", |me| me.id.as_str());
    let _ = events.send(Event::People(people_list(&world.users, me)));
  }

  let mut pending = PendingStart::default();
  let http = match Http::from_cookie_file(&crate::config_dir().join("cookies.json")) {
    Ok(http) => http,
    Err(error) => {
      let _ = events.send(Event::SignedOut(error.to_string()));
      match wait_for_session(&commands, &fetches, &stop, &cache, &events).await {
        Some((http, held)) => {
          pending = held;
          http
        },
        None => return,
      }
    },
  };
  let now_secs = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map_or(0, |elapsed| elapsed.as_secs());
  if let Some((token, read_at)) = &world.xsrf
    && now_secs.saturating_sub(*read_at) < XSRF_MAX_AGE_SECS
  {
    http.set_xsrf(token.clone());
  }
  let _ = events.send(Event::Connection(Connection::Connecting));

  let state = State {
    me: world.me,
    users: Arc::new(world.users.into_iter().map(|user| (user.id.clone(), user)).collect()),
    sections: world.sections,
    groups: world.groups.into_iter().map(|group| (group.id.clone(), group)).collect(),
    ..Default::default()
  };
  let (mail_writes, mail_jobs) = tokio::sync::mpsc::unbounded_channel();
  let cookie_path = crate::config_dir().join("cookies.json");
  let calendar =
    crate::calendar::Client::new(ServiceSession::new("Google Calendar", cookie_path.clone(), events.clone()));
  let mail = crate::mail::Client::new(ServiceSession::new("Gmail", cookie_path, events.clone()));
  let engine = Arc::new(Engine {
    http,
    channel: Channel::new(),
    cache,
    events,
    state: Mutex::new(state),
    readonly: std::env::var("WORKING_READONLY").is_ok_and(|value| value == "1"),
    mcp: tokio::sync::Mutex::new(None),
    presence: Mutex::new(None),
    calendar,
    calendar_range: Mutex::new(None),
    event_watch: Mutex::new(None),
    events_written: Wake::new(),
    mail_writes,
    mail,
    mail_held: Mutex::new(MailHeld::default()),
    unsaved: Mutex::new(HashMap::new()),
    history_queued: Wake::new(),
    writing: tokio::sync::Mutex::new(()),
    groups_changed: Wake::new(),
    session_restored: Wake::new(),
    prefetch_slots: Semaphore::new(3),
  });

  tokio::spawn(serve_fetches(engine.clone(), fetches));
  let (stream_tx, stream_rx) = tokio::sync::mpsc::unbounded_channel();
  tokio::spawn({
    let engine = engine.clone();
    async move { engine.channel.run(&engine.http, &stream_tx).await }
  });
  tokio::spawn(engine.clone().stream_loop(stream_rx));
  tokio::spawn(engine.clone().presence_loop());
  tokio::spawn(engine.clone().statuses_loop());
  tokio::spawn(engine.clone().session_loop());
  tokio::spawn(engine.clone().history_writer());
  tokio::spawn(engine.clone().mail_writer(mail_jobs));
  tokio::spawn({
    let engine = engine.clone();
    async move {
      // The cached list already names the unread and recent conversations: their histories load while the list
      // refreshes.
      engine.prefetch_histories();
      let result = engine.sync_world_and_mentions(true).await;
      engine.report(result);
    }
  });

  if pending.has_mcp {
    engine.handle(Command::SetMcp(pending.mcp));
  }
  if let Some(presence) = pending.presence {
    engine.handle(Command::SetPresence(presence));
  }

  loop {
    tokio::select! {
      _ = stop.recv_async() => break,
      command = commands.recv_async() => match command {
        Ok(command) => engine.handle(command),
        Err(_) => break,
      },
    }
  }
  // The app stopped the core or let go of it: the inbox watch stops, and what is still queued for the disk cache is
  // written before the runtime stops.
  engine.stop_mail_watch();
  engine.watch_events(false);
  engine.write_histories().await;
}

async fn serve_fetches(engine: Arc<Engine>, fetches: flume::Receiver<(String, FetchReply)>) {
  let network = Arc::new(Semaphore::new(8));
  while let Ok((url, reply)) = fetches.recv_async().await {
    let engine = engine.clone();
    let network = network.clone();
    tokio::spawn(async move {
      let cache = engine.cache.clone();
      let cached = {
        let url = url.clone();
        tokio::task::spawn_blocking(move || cache.image(&url)).await.ok().flatten()
      };
      let result = if let Some(bytes) = cached {
        Ok(Arc::new(bytes))
      } else {
        let _permit = network.acquire().await;
        // Gmail's attachments and the images inside emails open only with Gmail's own cookies.
        let mail = crate::mail::is_mail_url(&url);
        let downloaded = if mail {
          engine.mail.fetch(&url).await
        } else {
          engine.http.get_bytes(&url).await
        };
        match downloaded {
          Ok(bytes) => {
            let bytes = Arc::new(bytes);
            // An attachment that is not a picture stays out of the image cache.
            if !mail || crate::mail::looks_like_image(&bytes) {
              let cache = engine.cache.clone();
              let stored = bytes.clone();
              tokio::task::spawn_blocking(move || cache.save_image(&url, &stored));
            }
            Ok(bytes)
          },
          Err(error) => Err(error.to_string()),
        }
      };
      let _ = reply.send(result);
    });
  }
}

impl Engine {
  fn emit(&self, event: Event) {
    let _ = self.events.send(event);
  }

  fn report(&self, result: Result<()>) {
    let Err(error) = result else { return };
    match error.downcast_ref::<SignedOut>() {
      Some(signed_out) => self.emit(Event::SignedOut(signed_out.0.clone())),
      None => self.emit(Event::Error(format!("{error:#}"))),
    }
  }

  fn me_id(&self) -> String {
    self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).me_id()
  }

  /// The users known now. Converting with them happens outside the state lock.
  fn users(&self) -> Arc<Users> {
    Arc::clone(&self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).users)
  }

  // ---- Conversation list ----

  /// Reads the conversation list again and publishes it. With `prefetch`, the unread and recent conversations'
  /// histories start loading as soon as the list is in.
  async fn sync_world(self: &Arc<Self>, prefetch: bool) -> Result<()> {
    let known_me = self.me_id();
    let request = proto::PaginatedWorldRequest {
      request_header: Some(request_header()),
      fetch_from_user_spaces: Some(true),
      fetch_snippets_for_unnamed_rooms: Some(true),
      world_section_requests: vec![proto::WorldSectionRequest {
        page_size: Some(999),
        num_world_items_with_snippet: Some(999),
        ..Default::default()
      }],
      ..Default::default()
    };
    let world = self.http.api::<_, proto::PaginatedWorldResponse>("paginated_world", &request);
    // The signed-in user's id is needed only once the list is in: both are asked for at once.
    let (me_id, response) = if known_me.is_empty() {
      let status_request = proto::GetSelfUserStatusRequest {
        request_header: Some(request_header()),
        ..Default::default()
      };
      let status = self
        .http
        .api::<_, proto::GetSelfUserStatusResponse>("get_self_user_status", &status_request);
      let (status, response) = tokio::join!(status, world);
      let me_id = status?
        .user_status
        .and_then(|status| status.user_id)
        .and_then(|id| id.id)
        .unwrap_or_default();
      (me_id, response?)
    } else {
      (known_me, world.await?)
    };
    let items: Vec<proto::WorldItemLite> = response.world_items.into_iter().filter(convert::is_member).collect();

    let mut needed: HashSet<String> = HashSet::from([me_id.clone()]);
    for item in &items {
      needed.extend(convert::member_ids(item));
      let creator = item.message.as_ref().and_then(|message| message.creator.as_ref());
      // A snippet author that already carries a name needs no lookup.
      if let Some(creator) = creator.filter(|creator| creator.name.as_deref().unwrap_or_default().is_empty()) {
        needed.extend(creator.user_id.as_ref().and_then(|id| id.id.clone()));
      }
    }
    {
      let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      needed.retain(|id| !id.is_empty() && !state.users.contains_key(id));
      state.items = items
        .into_iter()
        .filter_map(|item| Some((convert::group_id(item.group_id.as_ref()?)?, item)))
        .collect();
    }
    // Paint with the names already known, then again once the missing people are resolved.
    self.rebuild_groups(&me_id);
    if prefetch {
      self.prefetch_histories();
    }
    if !needed.is_empty() {
      self.fetch_users(needed.into_iter().collect()).await?;
      self.rebuild_groups(&me_id);
    }
    self.save_world();
    // Section names are not in the list answer; they are read from the web app's page when one is unknown.
    let unnamed: Vec<String> = {
      let mut guard = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      let state = &mut *guard;
      let ids: HashSet<String> = state
        .groups
        .values()
        .filter_map(|group| match &group.placement {
          Placement::Custom { id, .. } if !state.sections.contains_key(id) && !state.section_lookups.contains(id) => {
            Some(id.clone())
          },
          _ => None,
        })
        .collect();
      state.section_lookups.extend(ids.iter().cloned());
      ids.into_iter().collect()
    };
    if !unnamed.is_empty() {
      tokio::spawn(self.clone().name_sections(unnamed, me_id));
    }
    Ok(())
  }

  /// Reads the names of the user's own sections from the web app's page and publishes the list with them. A failed
  /// read lets the next list refresh look `ids` up again.
  async fn name_sections(self: Arc<Self>, ids: Vec<String>, me_id: String) {
    let page = match self.http.get_bytes("https://chat.google.com/u/0/app/home").await {
      Ok(page) => page,
      Err(error) => {
        self
          .state
          .lock()
          .unwrap_or_else(std::sync::PoisonError::into_inner)
          .section_lookups
          .retain(|id| !ids.contains(id));
        return self.report(Err(error));
      },
    };
    let names = tokio::task::spawn_blocking(move || convert::section_names(&String::from_utf8_lossy(&page)))
      .await
      .unwrap_or_default();
    if !names.is_empty() {
      self
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .sections
        .extend(names);
      self.rebuild_groups(&me_id);
      self.save_world();
    }
  }

  async fn fetch_users(self: &Arc<Self>, ids: Vec<String>) -> Result<()> {
    let mut requests = tokio::task::JoinSet::new();
    for chunk in ids.chunks(100) {
      let request = proto::GetMembersRequest {
        request_header: Some(request_header()),
        member_ids: chunk
          .iter()
          .map(|id| proto::MemberId {
            user_id: Some(proto::UserId {
              id: Some(id.clone()),
              ..Default::default()
            }),
            ..Default::default()
          })
          .collect(),
        ..Default::default()
      };
      let engine = self.clone();
      requests.spawn(async move { engine.http.api::<_, proto::GetMembersResponse>("get_members", &request).await });
    }
    let mut first_error = None;
    while let Some(joined) = requests.join_next().await {
      match joined? {
        Ok(response) => {
          let users = response
            .members
            .iter()
            .filter_map(|member| member.user.as_ref())
            .filter_map(convert::user);
          self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .add_users(users);
        },
        Err(error) => first_error = first_error.or(Some(error)),
      }
    }
    first_error.map_or(Ok(()), Err)
  }

  /// Converts the raw conversation list with the users known now and publishes it. Each conversation keeps what is
  /// known of its mentions (`settle_mention`), which the list does not carry.
  fn rebuild_groups(&self, me_id: &str) {
    let mut guard = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let state = &mut *guard;
    if state.me.as_ref().is_none_or(|me| me.name.is_empty())
      && let Some(me) = state.users.get(me_id).cloned()
    {
      state.me = Some(me.clone());
      self.emit(Event::Me(me));
    }
    let groups: HashMap<GroupId, Group> = state
      .items
      .values()
      .filter_map(|item| convert::group(item, me_id, &state.users, &state.sections))
      .map(|mut group| {
        // The list carries no mention: the held history decides it, else the earlier flag while the cursor stays.
        let known = state
          .groups
          .get(&group.id)
          .is_some_and(|held| held.mentioned && held.last_read_time == group.last_read_time);
        let history = state.histories.get(&group.id);
        settle_mention(&mut group, known, history, me_id);
        (group.id.clone(), group)
      })
      .collect();
    state.groups = groups;
    self.emit(Event::World(Arc::new(state.groups.values().cloned().collect())));
    // The people list goes out again only when someone was added or changed.
    let people = std::mem::take(&mut state.people_stale).then(|| Arc::clone(&state.users));
    drop(guard);
    if let Some(users) = people {
      self.emit(Event::People(people_list(users.values(), me_id)));
    }
    self.groups_changed.notify_one();
  }

  /// Reads the conversation list (`sync_world`); once it is in, the mention marks are seeded from the Mentions search
  /// (`seed_mentions`) in the background.
  async fn sync_world_and_mentions(self: &Arc<Self>, prefetch: bool) -> Result<()> {
    self.sync_world(prefetch).await?;
    tokio::spawn(self.clone().seed_mentions());
    Ok(())
  }

  /// Marks the conversations that the newest page of the Mentions search names (`seeds_mention`), for mentions that
  /// neither the stream nor a held history showed, such as one that came while the app was closed. One request, a
  /// read (so also under `WORKING_READONLY=1`), with no paging; the Mentions shortcut's hits, cursor, and cache file
  /// are left alone. Each mark goes through `update_group`, so a held history that reaches past the read cursor still
  /// decides, reading the conversation clears it, and a change is published. A failed request is logged and ignored.
  /// The search answers direct @mentions of the user only, never @all: checked read-only against a live account, the
  /// held histories had @all messages from others newer than the page's oldest hit and none of them was a hit, while
  /// every held direct mention in that span was. So an @all that came while the app was closed, in a conversation
  /// whose history is not held, still shows as plain unread.
  async fn seed_mentions(self: Arc<Self>) {
    let me = self.me_id();
    if me.is_empty() {
      return;
    }
    let hits = match self.search_page("", Some(&me), None).await {
      Ok((hits, _)) => hits,
      Err(error) => {
        tracing::warn!("mention seeding failed: {error:#}");
        return;
      },
    };
    for hit in &hits {
      self.update_group(&hit.group, |group| {
        if seeds_mention(hit, Some(group), &me) {
          group.mentioned = true;
        }
      });
    }
  }

  /// Saves the conversation list; the cache leaves the file alone when nothing in it changed.
  fn save_world(&self) {
    let (me, users, groups, sections) = {
      let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      (
        state.me.clone(),
        Arc::clone(&state.users),
        state.groups.values().cloned().collect::<Vec<Group>>(),
        state.sections.clone(),
      )
    };
    let now = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .map_or(0, |elapsed| elapsed.as_secs());
    let xsrf = self.http.xsrf().map(|token| (token, now));
    let cache = self.cache.clone();
    tokio::task::spawn_blocking(move || {
      cache.save_world(WorldCache {
        me,
        users: users.values().cloned().collect(),
        groups,
        xsrf,
        sections,
      });
    });
  }

  /// Coalesces bursts of list-changing events into one refresh.
  fn queue_world_refresh(self: &Arc<Self>) {
    {
      let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      if state.world_refresh_queued {
        return;
      }
      state.world_refresh_queued = true;
    }
    let engine = self.clone();
    tokio::spawn(async move {
      tokio::time::sleep(Duration::from_millis(800)).await;
      engine
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .world_refresh_queued = false;
      let result = engine.sync_world(false).await;
      engine.report(result);
    });
  }

  // ---- History ----

  /// Loads the latest messages of the conversations `prefetch_order` picks, three at a time, so their mention marks
  /// settle and the first click on them is instant.
  fn prefetch_histories(self: &Arc<Self>) {
    let mut guard = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let state = &mut *guard;
    let order = prefetch_order(state.groups.values(), |id| state.histories.get(id).is_some_and(History::loaded));
    for group in order {
      if !state.prefetching.insert(group.clone()) {
        continue;
      }
      let engine = self.clone();
      tokio::spawn(async move {
        let slot = engine.prefetch_slots.acquire().await;
        let loaded = engine
          .state
          .lock()
          .unwrap_or_else(std::sync::PoisonError::into_inner)
          .histories
          .get(&group)
          .is_some_and(History::loaded);
        // Prefetch is opportunistic: the conversation reports the failure itself when it is opened.
        if slot.is_ok()
          && !loaded
          && let Err(error) = engine.refresh_history(group.clone()).await
        {
          tracing::warn!("history prefetch failed: {error:#}");
        }
        drop(slot);
        engine
          .state
          .lock()
          .unwrap_or_else(std::sync::PoisonError::into_inner)
          .prefetching
          .remove(&group);
      });
    }
  }

  fn emit_history(&self, group: &GroupId, history: &History, prepended: usize) {
    self.emit_history_focused(group, history, prepended, None);
  }

  fn emit_history_focused(&self, group: &GroupId, history: &History, prepended: usize, focus: Option<String>) {
    self.emit(Event::History {
      group: group.clone(),
      messages: Arc::new(history.messages.clone()),
      reached_start: history.reached_start,
      reached_end: !history.detached,
      prepended,
      focus,
    });
  }

  /// Queues a conversation for the disk cache; `history_writer` writes it once its changes pause.
  fn save_history(&self, group: &GroupId, history: &History) {
    // A window into the past is not what the next start should show, and a partial one is not the conversation.
    if history.detached || history.partial {
      return;
    }
    self
      .unsaved
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .insert(group.clone(), history.saved());
    self.history_queued.notify_one();
  }

  /// Writes the queued histories once their changes pause for `SAVE_PAUSE`, and at the latest `SAVE_WAIT_MAX` after
  /// the first of them was queued.
  async fn history_writer(self: Arc<Self>) {
    loop {
      self.history_queued.notified().await;
      let deadline = tokio::time::Instant::now() + SAVE_WAIT_MAX;
      loop {
        let pause = (tokio::time::Instant::now() + SAVE_PAUSE).min(deadline);
        tokio::select! {
          () = tokio::time::sleep_until(pause) => break,
          () = self.history_queued.notified() => {},
        }
      }
      self.write_histories().await;
    }
  }

  /// Writes every queued history, the latest snapshot of each, one file at a time.
  async fn write_histories(&self) {
    let _writing = self.writing.lock().await;
    let unsaved = std::mem::take(&mut *self.unsaved.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    if unsaved.is_empty() {
      return;
    }
    let cache = self.cache.clone();
    let _ = tokio::task::spawn_blocking(move || {
      for (group, history) in &unsaved {
        cache.save_history(group, history);
      }
    })
    .await;
  }

  /// Publishes who has read `group`, unless that is what was published last (see `Receipts::updated`).
  fn note_receipts(&self, group: &GroupId, set: &proto::ReadReceiptSet, merge: bool) {
    let mut guard = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let state = &mut *guard;
    let me = state.me.as_ref().map_or("", |me| me.id.as_str());
    let Some(next) = Receipts::updated(state.receipts.get(group), set, me, &state.users, merge) else {
      return;
    };
    self.emit_receipts(group, &next);
    state.receipts.insert(group.clone(), next);
  }

  fn emit_receipts(&self, group: &GroupId, receipts: &Receipts) {
    self.emit(Event::ReadReceipts {
      group: group.clone(),
      enabled: receipts.enabled,
      receipts: Arc::new(receipts.people.clone()),
    });
  }

  /// The receipts that came with a page of messages, when it had any.
  fn note_page_receipts(&self, group: &GroupId, page: &proto::ListTopicsResponse) {
    if let Some(set) = &page.read_receipt_set {
      self.note_receipts(group, set, false);
    }
  }

  async fn list_topics(&self, group: &GroupId, page: Page) -> Result<proto::ListTopicsResponse> {
    use proto::list_topics_request::filter::Anchor;
    use proto::list_topics_request::{FetchOptions, Filter};
    let filter = match page {
      Page::Latest => None,
      Page::Before(time) => Some(Filter {
        topics_before_sort_time: Some(time),
        ..Default::default()
      }),
      Page::After(time) => Some(Filter {
        topics_after_sort_time: Some(time),
        ..Default::default()
      }),
      Page::At(time) => Some(Filter {
        anchor: Some(Anchor { sort_time: Some(time) }),
        ..Default::default()
      }),
    };
    let request = proto::ListTopicsRequest {
      request_header: Some(request_header()),
      group_id: Some(convert::proto_group_id(group)),
      page_size_for_topics: Some(if filter.is_some() {
        OLDER_PAGE
      } else {
        LATEST_PAGE
      }),
      page_size_for_unread_replies: Some(1000),
      page_size_for_read_replies: Some(20),
      fetch_options: vec![FetchOptions::User as i32, FetchOptions::ReadReceipts as i32],
      filter,
      ..Default::default()
    };
    self.http.api("list_topics", &request).await
  }

  /// Flattens topics into messages, oldest topic first, replies right after their first message. The conversion
  /// runs outside the state lock.
  fn convert_topics(&self, response: &proto::ListTopicsResponse) -> Converted {
    let users = self.users();
    let mut topics: Vec<&proto::Topic> = response.topics.iter().collect();
    topics.sort_by_key(|topic| topic.sort_time.unwrap_or(0));
    let mut converted = Converted {
      anchor: topics.first().and_then(|topic| topic.sort_time).unwrap_or(0),
      ..Default::default()
    };
    for topic in topics {
      let start = converted.messages.len();
      converted.messages.extend(
        topic
          .replies
          .iter()
          .filter_map(|message| convert::message(message, &users))
          .map(Arc::new),
      );
      if let Some(first) = converted.messages.get(start) {
        converted
          .topic_times
          .insert(first.topic_id.clone(), topic.sort_time.unwrap_or(first.time));
      }
    }
    converted
  }

  async fn refresh_history(&self, group: GroupId) -> Result<()> {
    // A window into the past stays as it is until the conversation is opened again.
    if self
      .state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .histories
      .get(&group)
      .is_some_and(|history| history.detached)
    {
      return Ok(());
    }
    let response = self.list_topics(&group, Page::Latest).await?;
    self.note_page_receipts(&group, &response);
    let Converted { messages: fetched, anchor, topic_times } = self.convert_topics(&response);
    let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let online = state.connection == Some(Connection::Online);
    let history = state.hold(&group);
    // A jump landed while this page was on its way: the window wins.
    if history.detached {
      return Ok(());
    }
    let fetched_ids: HashSet<&str> = fetched.iter().map(|message| message.id.as_str()).collect();
    // Older messages already held stay only when they connect to this page; a gap means they are stale.
    let overlaps = history.messages.iter().any(|message| fetched_ids.contains(message.id.as_str()));
    let oldest_fetched = fetched.first().map_or(Micros::MAX, |message| message.time);
    let pending: Vec<Arc<Message>> = history.messages.iter().filter(|message| message.pending).cloned().collect();
    let mut messages: Vec<Arc<Message>> = if overlaps {
      history
        .messages
        .iter()
        .filter(|message| {
          !message.pending && message.time < oldest_fetched && !fetched_ids.contains(message.id.as_str())
        })
        .cloned()
        .collect()
    } else {
      Vec::new()
    };
    let kept_older = !messages.is_empty();
    drop(fetched_ids);
    messages.extend(fetched);
    messages.extend(pending);
    if !kept_older {
      history.reached_start = response.contains_first_topic.unwrap_or(false);
      history.anchor = anchor;
      history.topic_times.clear();
    }
    history.topic_times.extend(topic_times);
    history.messages = messages;
    history.fresh = online;
    history.fetched = true;
    history.partial = false;
    self.emit_history(&group, history, 0);
    self.save_history(&group, history);
    drop(state);
    self.settle_held_mention(&group);
    Ok(())
  }

  async fn load_older(&self, group: GroupId) -> Result<()> {
    let anchor = {
      let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      let Some(history) = state.histories.get_mut(&group) else {
        return Ok(());
      };
      if history.reached_start || history.loading_older || history.anchor == 0 {
        return Ok(());
      }
      history.loading_older = true;
      history.anchor
    };
    let result = self.list_topics(&group, Page::Before(anchor)).await;
    let converted = result.as_ref().ok().map(|response| self.convert_topics(response));
    let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(history) = state.histories.get_mut(&group) else {
      return Ok(());
    };
    history.loading_older = false;
    let response = result?;
    let Converted {
      messages: older,
      anchor: new_anchor,
      topic_times,
    } = converted.unwrap_or_default();
    history.topic_times.extend(topic_times);
    let held: HashSet<&str> = history.messages.iter().map(|message| message.id.as_str()).collect();
    let mut merged: Vec<Arc<Message>> = older
      .into_iter()
      .filter(|message| !held.contains(message.id.as_str()))
      .collect();
    drop(held);
    let prepended = merged.len();
    merged.append(&mut history.messages);
    history.messages = merged;
    history.reached_start = response.contains_first_topic.unwrap_or(false) || response.topics.is_empty();
    if new_anchor > 0 {
      history.anchor = new_anchor;
    }
    let history = &state.histories[&group];
    self.emit_history(&group, history, prepended);
    self.save_history(&group, history);
    drop(state);
    self.settle_held_mention(&group);
    Ok(())
  }

  /// Replaces the conversation's messages with the window around one message.
  async fn jump(&self, group: GroupId, message_id: String, time: Micros) -> Result<()> {
    let (around, after) = tokio::join!(
      self.list_topics(&group, Page::At(time)),
      self.list_topics(&group, Page::After(time))
    );
    let (around, after) = (around?, after?);
    self.note_page_receipts(&group, &around);
    self.note_page_receipts(&group, &after);
    let Converted { mut messages, anchor, mut topic_times } = self.convert_topics(&around);
    let newer = self.convert_topics(&after);
    topic_times.extend(newer.topic_times);
    let newer = newer.messages;
    let held: HashSet<String> = messages.iter().map(|message| message.id.clone()).collect();
    messages.extend(newer.into_iter().filter(|message| !held.contains(&message.id)));
    let newest = around
      .topics
      .iter()
      .chain(&after.topics)
      .filter_map(|topic| topic.sort_time)
      .max()
      .unwrap_or(time);
    let complete = after.contains_last_topic.unwrap_or(false);
    let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let fresh = complete && state.connection == Some(Connection::Online);
    let history = state.hold(&group);
    *history = History {
      messages,
      reached_start: around.contains_first_topic.unwrap_or(false),
      anchor,
      fresh,
      detached: !complete,
      newest,
      topic_times,
      used: history.used,
      fetched: true,
      ..Default::default()
    };
    self.emit_history_focused(&group, history, 0, Some(message_id));
    Ok(())
  }

  async fn load_newer(&self, group: GroupId) -> Result<()> {
    let newest = {
      let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      let Some(history) = state.histories.get_mut(&group) else {
        return Ok(());
      };
      if !history.detached || history.loading_newer {
        return Ok(());
      }
      history.loading_newer = true;
      history.newest
    };
    let result = self.list_topics(&group, Page::After(newest)).await;
    let converted = result.as_ref().ok().map(|response| self.convert_topics(response));
    let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let online = state.connection == Some(Connection::Online);
    let Some(history) = state.histories.get_mut(&group) else {
      return Ok(());
    };
    history.loading_newer = false;
    let response = result?;
    let newer = converted.unwrap_or_default();
    history.topic_times.extend(newer.topic_times);
    let held: HashSet<String> = history.messages.iter().map(|message| message.id.clone()).collect();
    history
      .messages
      .extend(newer.messages.into_iter().filter(|message| !held.contains(&message.id)));
    history.newest = response
      .topics
      .iter()
      .filter_map(|topic| topic.sort_time)
      .max()
      .unwrap_or(newest)
      .max(newest);
    if response.contains_last_topic.unwrap_or(false) || response.topics.is_empty() {
      history.detached = false;
      history.fresh = online;
    }
    let history = &state.histories[&group];
    self.emit_history(&group, history, 0);
    self.save_history(&group, history);
    drop(state);
    self.settle_held_mention(&group);
    Ok(())
  }

  /// Searches messages in every conversation through the web app's search RPC.
  async fn search(&self, query: String) -> Result<()> {
    let hits = self.search_hits(&query).await?;
    self.emit(Event::SearchResults { query, hits: Arc::new(hits) });
    Ok(())
  }

  pub(crate) async fn search_hits(&self, query: &str) -> Result<Vec<SearchHit>> {
    Ok(self.search_page(query, None, None).await?.0)
  }

  /// One page of the web app's message search. With `mentioning`, only messages that @mention that user, newest
  /// first (the Mentions shortcut); otherwise best matches first. Answers the hits and the cursor of the next page.
  async fn search_page(
    &self,
    query: &str,
    mentioning: Option<&str>,
    cursor: Option<&str>,
  ) -> Result<(Vec<SearchHit>, Option<String>)> {
    const PAGE: usize = 20;
    let session = format!("{:032X}", rand::random::<u128>());
    let session = format!(
      "{}-{}-{}-{}-{}",
      &session[..8],
      &session[8..12],
      &session[12..16],
      &session[16..20],
      &session[20..]
    );
    // The mention filter names the user; sort 2 is newest first, 3 best match first.
    let mentions = mentioning.map(|me| serde_json::json!([[[[me]]]]));
    let sort = if mentioning.is_some() { 2 } else { 3 };
    let arguments = serde_json::json!([
      null,
      cursor,
      null,
      query,
      null,
      session,
      [[], null, null, null, session, mentions, 0, [[[[[[1]]]]], [[[1]]]]],
      null,
      [sort],
      [155],
      0,
      null,
      1
    ]);
    let answer = self.http.page_rpc("SBNmJb", &arguments).await?;
    let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // Results sit at index 9; each wraps a message whose fields are positional.
    let results = answer[9].as_array().map_or(&[][..], Vec::as_slice);
    let next = answer[0]
      .as_str()
      .filter(|next| !next.is_empty() && results.len() >= PAGE)
      .map(str::to_string);
    let hits: Vec<SearchHit> = results
      .iter()
      .filter_map(|result| {
        let message = &result[0];
        let group = match message[2][0].as_str()?.split_once('/')? {
          ("dm", id) => GroupId::Dm(id.to_string()),
          ("space", id) => GroupId::Space(id.to_string()),
          _ => return None,
        };
        let sender_id = message[4][0].as_str().unwrap_or_default().to_string();
        let sender = state.users.get(&sender_id);
        Some(SearchHit {
          group,
          message_id: message[17][0].as_str()?.to_string(),
          topic_id: message[17][2][0].as_str().unwrap_or_default().to_string(),
          sender_name: sender.map_or_default(|user| user.name.clone()),
          sender_avatar: sender.and_then(|user| user.avatar_url.clone()),
          sender_id,
          text: message[5].as_str().unwrap_or_default().to_string(),
          time: message[11]
            .as_str()
            .and_then(|time| time.parse().ok())
            .or_else(|| message[11].as_i64())?,
        })
      })
      .collect();
    Ok((hits, next))
  }

  /// One page of a shortcut's messages. The newest page is answered twice: from the disk cache at once, without a
  /// cursor, then from the network, which the cache keeps for the next time.
  async fn load_shortcut(&self, shortcut: Shortcut, cursor: Option<String>) -> Result<()> {
    let appended = cursor.is_some();
    if !appended {
      let cache = self.cache.clone();
      if let Some(hits) = tokio::task::spawn_blocking(move || cache.shortcut(shortcut)).await? {
        self.emit(Event::Shortcut {
          shortcut,
          hits: Arc::new(hits),
          appended: false,
          cursor: None,
        });
      }
    }
    let (hits, cursor) = match shortcut {
      Shortcut::Mentions => {
        let me = self.me_id();
        if me.is_empty() {
          bail!("Mentions need the signed-in user, who is not known yet.");
        }
        self.search_page("", Some(&me), cursor.as_deref()).await?
      },
      Shortcut::Starred => (self.starred().await?, None),
    };
    let hits = Arc::new(hits);
    if !appended {
      let cache = self.cache.clone();
      let hits = hits.clone();
      tokio::task::spawn_blocking(move || cache.save_shortcut(shortcut, &hits));
    }
    self.emit(Event::Shortcut { shortcut, hits, appended, cursor });
    Ok(())
  }

  /// The starred messages, newest first. The web app reads them as a world section filtered by the starred
  /// label; the protobuf schema here has no field for that filter, so the request is the web app's JSON.
  async fn starred(&self) -> Result<Vec<SearchHit>> {
    let header: serde_json::Value = serde_json::from_str(WEB_REQUEST_HEADER)?;
    let section: serde_json::Value = serde_json::from_str(STARRED_SECTION)?;
    let request = serde_json::json!([header, [section], null, [4, 2, 5, 6, 7, 3, 8], null, null, null, null, 0, [1]]);
    let answer = self.http.api_json("paginated_world", &request).await?;
    let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // Field 8 of the answer lists the section's entries; a starred message is an entry's third field.
    let mut hits: Vec<SearchHit> = answer[0][7]
      .as_array()
      .into_iter()
      .flatten()
      .filter_map(|entry| starred_hit(&entry[2], &state.users))
      .collect();
    hits.sort_by(|a, b| b.time.cmp(&a.time));
    Ok(hits)
  }

  async fn open(&self, group: GroupId) -> Result<()> {
    // A partial history holds only what was sent before the conversation was loaded: the cache goes under it.
    let needs_cache = self
      .state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .histories
      .get(&group)
      .is_none_or(|history| history.partial);
    if needs_cache {
      let cache = self.cache.clone();
      let key = group.clone();
      let cached = tokio::task::spawn_blocking(move || cache.history(&key)).await?;
      let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      let loaded = state.histories.get(&group).is_some_and(|history| !history.partial);
      if let Some(cached) = cached
        && !loaded
      {
        let history = state.hold(&group);
        let ids: HashSet<&str> = cached.messages.iter().map(|message| message.id.as_str()).collect();
        let sent: Vec<Arc<Message>> = std::mem::take(&mut history.messages)
          .into_iter()
          .filter(|message| !ids.contains(message.id.as_str()))
          .collect();
        drop(ids);
        let mut messages = cached.messages;
        messages.extend(sent);
        history.messages = messages;
        history.reached_start = cached.reached_start;
        history.anchor = cached.anchor;
        history.partial = false;
        self.emit_history(&group, history, 0);
      }
    }
    let fresh = self
      .state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .histories
      .get(&group)
      .is_some_and(|history| history.fresh);
    if !fresh {
      self.refresh_history(group.clone()).await?;
    }
    // Typing notices only arrive for subscribed conversations.
    let subscription = proto::StreamEventsRequest {
      group_subscription_event: Some(proto::GroupSubscriptionEvent {
        group_ids: vec![convert::proto_group_id(&group)],
      }),
      ..Default::default()
    };
    let _ = self.channel.send(&self.http, &subscription).await;
    Ok(())
  }

  // ---- Commands ----

  fn handle(self: &Arc<Self>, command: Command) {
    let engine = self.clone();
    match command {
      Command::Open(group) => {
        {
          let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
          state.open = Some(group.clone());
          // Opening always shows the latest messages: a window left by a jump is dropped.
          if state.histories.get(&group).is_some_and(|history| history.detached) {
            state.histories.remove(&group);
          }
          if let Some(held) = state.receipts.get(&group) {
            self.emit_receipts(&group, held);
          }
          state.touch(&group);
          // A partial history is not the conversation: `open` puts the cache under it first.
          if let Some(history) = state.histories.get(&group).filter(|history| !history.partial) {
            self.emit_history(&group, history, 0);
          }
        }
        tokio::spawn(async move {
          let result = engine.open(group).await;
          engine.report(result);
        });
      },
      Command::Close => self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).open = None,
      Command::LoadNewer(group) => {
        tokio::spawn(async move {
          let result = engine.load_newer(group).await;
          engine.report(result);
        });
      },
      Command::LoadShortcut { shortcut, cursor } => {
        tokio::spawn(async move {
          let result = engine.load_shortcut(shortcut, cursor).await;
          engine.report(result);
        });
      },
      Command::Search(query) => {
        tokio::spawn(async move {
          let result = engine.search(query).await;
          engine.report(result);
        });
      },
      Command::Jump { group, message_id, time } => {
        {
          let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
          state.open = Some(group.clone());
          // The view that jumps here may not hold who has read it yet, and `jump` publishes only what changed.
          if let Some(held) = state.receipts.get(&group) {
            self.emit_receipts(&group, held);
          }
        }
        tokio::spawn(async move {
          let result = engine.jump(group, message_id, time).await;
          engine.report(result);
        });
      },
      Command::LoadOlder(group) => {
        tokio::spawn(async move {
          let result = engine.load_older(group).await;
          engine.report(result);
        });
      },
      Command::SetMcp(config) => {
        tokio::spawn(engine.set_mcp(config));
      },
      Command::Connect => {
        tokio::spawn(async move {
          if let Err(error) = engine.http.reload_cookies() {
            engine.report(Err(error));
            return;
          }
          match engine.http.refresh_xsrf().await {
            Ok(_) => {
              engine.session_restored.notify_waiters();
              engine.emit(Event::Connection(Connection::Connecting));
              let result = engine.sync_world_and_mentions(true).await;
              engine.report(result);
              // Also when the list failed to load: the held one names the unread and recent conversations.
              engine.prefetch_histories();
            },
            Err(error) => engine.report(Err(error)),
          }
        });
      },
      Command::SetPresence(presence) => {
        if self.readonly {
          return;
        }
        let before = self
          .presence
          .lock()
          .unwrap_or_else(std::sync::PoisonError::into_inner)
          .replace(presence);
        if before == Some(presence) {
          return;
        }
        tokio::spawn(async move {
          // Away is Google Chat's "Set as away": the account stops sharing that it is there.
          let shared = presence != Presence::Away;
          if before.is_none_or(|before| (before != Presence::Away) != shared) {
            let request = proto::SetPresenceSharedRequest {
              request_header: Some(request_header()),
              presence_shared: Some(shared),
            };
            let result = engine
              .http
              .api::<_, proto::SetPresenceSharedResponse>("set_presence_shared", &request)
              .await;
            engine.report(result.map(|_| ()));
          }
          engine.heartbeat(presence).await;
        });
      },
      Command::LoadDnd => {
        tokio::spawn(async move {
          let result = engine.load_dnd().await;
          engine.report(result);
        });
      },
      Command::SaveDnd(schedules) => {
        if self.readonly {
          return self.emit(Event::Error("Read-only mode: the schedules were not saved.".into()));
        }
        tokio::spawn(async move {
          let result = engine.save_dnd(schedules).await;
          engine.report(result);
        });
      },
      Command::SetDnd(choice) => {
        if self.readonly {
          return self.emit(Event::Error("Read-only mode: do not disturb was not changed.".into()));
        }
        tokio::spawn(async move {
          let result = engine.set_dnd(choice).await;
          engine.report(result);
        });
      },
      Command::SetStatus { emoji, text, until } => {
        if self.readonly {
          return self.emit(Event::Error("Read-only mode: the status was not changed.".into()));
        }
        tokio::spawn(async move {
          let result = engine.set_status(emoji, text, until).await;
          engine.report(result);
        });
      },
      Command::RefreshWorld => {
        tokio::spawn(async move {
          let result = engine.sync_world(false).await;
          engine.report(result);
        });
      },
      Command::LoadMembers(group) => {
        tokio::spawn(async move {
          let result = engine.load_members(group).await;
          engine.report(result);
        });
      },
      Command::LoadSlashCommands(group) => {
        tokio::spawn(async move {
          let result = engine.load_slash_commands(group).await;
          engine.report(result);
        });
      },
      Command::Send {
        group,
        topic_id,
        text,
        quote,
        files,
        chips,
      } => {
        if self.readonly {
          return self.emit(Event::Error("Read-only mode: the message was not sent.".into()));
        }
        let quoted = quote.and_then(|id| self.held(&group, &id));
        let shown = quoted.as_ref().map(|quoted| Quote {
          sender_name: quoted.sender_name.clone(),
          text: quoted.text.clone(),
        });
        let attached = files.iter().map(|path| local_attachment(path)).collect();
        let typed = markup::parse(&text, &chips);
        let Some(pending) = self.add_pending(&group, topic_id.as_deref(), &typed, shown, attached) else {
          return;
        };
        tokio::spawn(async move {
          let result = async {
            let uploads = engine.upload_files(&group, files).await?;
            engine
              .send_message(&group, topic_id.as_deref(), &typed, quoted.as_deref(), uploads, &pending)
              .await
          }
          .await;
          if result.is_err() {
            engine.fail_pending(&group, &pending);
          }
          engine.report(result);
        });
      },
      Command::Edit { group, message_id, text } => {
        if self.readonly {
          return self.emit(Event::Error("Read-only mode: the message was not edited.".into()));
        }
        let Some(original) = self.held(&group, &message_id) else {
          return;
        };
        let typed = markup::parse(&text, &[]);
        // Shown at once; the server's answer brings the final message, a failure puts the old one back.
        let mut edited = (*original).clone();
        edited.text.clone_from(&typed.text);
        edited.spans = convert::pending_spans(&typed);
        edited.edited = true;
        self.upsert_message(Arc::new(edited), None);
        tokio::spawn(async move {
          let request = proto::EditMessageRequest {
            request_header: Some(request_header()),
            id: Some(proto_message_id(&group, &original.topic_id, &original.id)),
            annotations: convert::format_annotations(&typed.text, &typed.spans),
            text_body: Some(typed.text),
            message_info: Some(proto::MessageInfo {
              accept_format_annotations: Some(true),
              ..Default::default()
            }),
            ..Default::default()
          };
          let result = engine.http.api::<_, proto::EditMessageResponse>("edit_message", &request).await;
          let answered = result.as_ref().ok().and_then(|response| response.message.as_ref());
          let converted = answered.and_then(|message| convert::message(message, &engine.users()));
          match converted {
            Some(message) => engine.upsert_message(Arc::new(message), None),
            None if result.is_err() => engine.upsert_message(original, None),
            None => {},
          }
          engine.report(result.map(|_| ()));
        });
      },
      Command::Delete { group, message_id } => {
        if self.readonly {
          return self.emit(Event::Error("Read-only mode: the message was not deleted.".into()));
        }
        let Some(message) = self.held(&group, &message_id) else {
          return;
        };
        tokio::spawn(async move {
          let request = proto::DeleteMessageRequest {
            request_header: Some(request_header()),
            message_id: Some(proto_message_id(&group, &message.topic_id, &message.id)),
          };
          let result = engine
            .http
            .api::<_, proto::DeleteMessageResponse>("delete_message", &request)
            .await;
          if result.is_ok() {
            engine.remove_message(&group, &message.id);
          }
          engine.report(result.map(|_| ()));
        });
      },
      Command::StartMeet(group) => {
        if self.readonly {
          return self.emit(Event::Error("Read-only mode: the meeting was not started.".into()));
        }
        let status = markup::parse("Starting a video meeting…", &[]);
        let Some(pending) = self.add_pending(&group, None, &status, None, Vec::new()) else {
          return;
        };
        tokio::spawn(async move {
          let result = engine.start_meet(&group, &pending).await;
          if result.is_err() {
            engine.fail_pending(&group, &pending);
          }
          engine.report(result);
        });
      },
      Command::CreateMeeting => {
        if self.readonly {
          return self.emit(Event::MeetingCreated(Err("Read-only mode: no meeting was created.".into())));
        }
        tokio::spawn(async move { engine.create_meeting().await });
      },
      Command::MarkRead(group) => {
        if self.readonly {
          return;
        }
        let time = now_micros();
        let changed = self.update_group(&group, |group| {
          group.unread = false;
          group.last_read_time = time;
        });
        if !changed {
          return;
        }
        // A mark set with "Mark as unread" stays until it is cleared on its own.
        let marked = self.marked_unread(&group);
        if marked > 0 {
          self.set_marked_unread(&group, 0);
        }
        tokio::spawn(async move {
          if marked > 0 {
            let cleared = engine.set_mark_as_unread(&group, 0).await;
            if cleared.is_err() {
              engine.set_marked_unread(&group, marked);
            }
            engine.report(cleared);
          }
          let request = proto::MarkGroupReadstateRequest {
            request_header: Some(request_header()),
            id: Some(convert::proto_group_id(&group)),
            last_read_time: Some(time),
          };
          let result = engine
            .http
            .api::<_, proto::MarkGroupReadstateResponse>("mark_group_readstate", &request)
            .await;
          engine.report(result.map(|_| ()));
        });
      },
      Command::MarkUnread(group) => {
        if self.readonly {
          return self.emit(Event::Error("Read-only mode: the conversation was not marked unread.".into()));
        }
        let Some(before) = self.group(&group) else {
          return;
        };
        let time = before.snippet.as_ref().map_or(before.sort_time, |snippet| snippet.time);
        let marked = self.marked_unread(&group);
        self.set_marked_unread(&group, time);
        self.update_group(&group, |group| group.unread = true);
        tokio::spawn(async move {
          let result = engine.set_mark_as_unread(&group, time).await;
          if result.is_err() {
            engine.set_marked_unread(&group, marked);
            engine.update_group(&group, |group| {
              group.unread = before.unread;
              group.mentioned = before.mentioned;
            });
          }
          engine.report_change(result, "The conversation was not marked unread.");
        });
      },
      Command::SetPinned { group, pinned } => {
        if self.readonly {
          return self.emit(Event::Error("Read-only mode: the pin was not changed.".into()));
        }
        let Some(before) = self.group(&group) else {
          return;
        };
        self.update_group(&group, |group| group.pinned = pinned);
        tokio::spawn(async move {
          let arguments = serde_json::json!([page_group_id(&group), i32::from(pinned)]);
          let result = engine.http.page_rpc("XZhsdc", &arguments).await.map(|_| ());
          if result.is_err() {
            engine.update_group(&group, |group| group.pinned = before.pinned);
          }
          let failure = if pinned {
            "The conversation was not pinned."
          } else {
            "The conversation was not unpinned."
          };
          engine.report_change(result, failure);
        });
      },
      Command::SetMuted { group, muted } => {
        if self.readonly {
          return self.emit(Event::Error("Read-only mode: notifications were not changed.".into()));
        }
        let Some(before) = self.group(&group) else {
          return;
        };
        let room_state = self.room_state(&group);
        self.update_group(&group, |group| {
          group.muted = muted;
          group.notify = convert::notify(muted, room_state);
        });
        tokio::spawn(async move {
          let result = engine.update_notifications(&group, room_state, muted).await;
          if result.is_err() {
            engine.update_group(&group, |group| {
              group.muted = before.muted;
              group.notify = before.notify;
            });
          }
          let failure = if muted {
            "The conversation was not muted."
          } else {
            "The conversation was not unmuted."
          };
          engine.report_change(result, failure);
        });
      },
      Command::SetHidden { group, hidden } => self.set_hidden(group, hidden),
      Command::SetBlocked { group, blocked } => self.set_blocked(group, blocked),
      Command::Leave(group) => self.leave(group),
      Command::DeleteConversation(group) => self.delete_conversation(group),
      Command::SetNotify { group, notify } => self.set_notify(group, notify),
      Command::MoveToSection { group, section } => self.move_to_section(group, section),
      Command::ClickCard {
        group,
        topic_id,
        message_id,
        card_id,
        action,
      } => {
        if self.readonly {
          return self.emit(Event::Error("Read-only mode: the card button was not pressed.".into()));
        }
        tokio::spawn(async move {
          let request = click_card_request(&group, &topic_id, &message_id, &card_id, &action);
          let result = engine.http.api_json("click_card", &request).await.map(|_| ());
          engine.report(result);
        });
      },
      Command::Star { group, topic_id, message_id, starred } => {
        if self.readonly {
          return self.emit(Event::Error("Read-only mode: the star was not changed.".into()));
        }
        self.apply_star(&group, &message_id, starred);
        tokio::spawn(async move {
          let endpoint = if starred {
            "apply_message_label"
          } else {
            "remove_message_label"
          };
          let result = match star_request(&group, &topic_id, &message_id) {
            Ok(request) => engine.http.api_json(endpoint, &request).await.map(|_| ()),
            Err(error) => Err(error),
          };
          if result.is_err() {
            engine.apply_star(&group, &message_id, !starred);
          }
          engine.report(result);
        });
      },
      Command::React { group, topic_id, message_id, emoji, add } => {
        if self.readonly {
          return self.emit(Event::Error("Read-only mode: the reaction was not sent.".into()));
        }
        let me = self.me_id();
        self.apply_reaction(&group, &message_id, &emoji, &me, add);
        tokio::spawn(async move {
          let request = proto::UpdateReactionRequest {
            request_header: Some(request_header()),
            message_id: Some(wire_message_id(&group, topic_id, message_id.clone())),
            emoji: Some(wire_emoji(&emoji)),
            option: Some(
              (if add {
                proto::ReactionOption::Add
              } else {
                proto::ReactionOption::Remove
              }) as i32,
            ),
          };
          let result = engine
            .http
            .api::<_, proto::UpdateReactionResponse>("update_reaction", &request)
            .await;
          if result.is_err() {
            engine.apply_reaction(&group, &message_id, &emoji, &me, !add);
          }
          engine.report(result.map(|_| ()));
        });
      },
      Command::LoadReactors { group, topic_id, message_id, emoji } => {
        tokio::spawn(async move {
          let users = match engine.reactors(&group, &topic_id, &message_id, &emoji).await {
            Ok(users) => Some(Arc::new(users)),
            Err(error) => {
              match error.downcast_ref::<SignedOut>() {
                Some(signed_out) => engine.emit(Event::SignedOut(signed_out.0.clone())),
                None => tracing::warn!("the reactors were not read: {error:#}"),
              }
              None
            },
          };
          engine.emit(Event::Reactors { group, message_id, emoji, users });
        });
      },
      Command::SetTyping { group, typing } => {
        if self.readonly {
          return;
        }
        tokio::spawn(async move {
          let state = if typing {
            proto::TypingState::Typing
          } else {
            proto::TypingState::Stopped
          };
          let request = proto::SetTypingStateRequest {
            request_header: Some(request_header()),
            state: Some(state as i32),
            context: Some(proto::TypingContext {
              group_id: Some(convert::proto_group_id(&group)),
              ..Default::default()
            }),
          };
          // A lost typing notice is not worth telling the user about.
          let _ = engine
            .http
            .api::<_, proto::SetTypingStateResponse>("set_typing_state", &request)
            .await;
        });
      },
      Command::LoadCalendars => {
        tokio::spawn(async move {
          match engine.calendar.calendars().await {
            Ok(calendars) => engine.emit(Event::Calendars(Arc::new(calendars))),
            Err(error) => engine.emit(Event::CalendarFailed(format!("{error:#}"))),
          }
        });
      },
      Command::LoadEvents { start, end } => {
        *self.calendar_range.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((start, end));
        tokio::spawn(async move { engine.load_events(start, end).await });
      },
      Command::SaveEvent(draft) => {
        if self.readonly {
          return self.emit(Event::CalendarWritten(Err("Read-only mode: the event was not saved.".into())));
        }
        tokio::spawn(async move { engine.save_event(*draft).await });
      },
      Command::DeleteEvent { calendar_id, event_id, scope, notify } => {
        if self.readonly {
          return self.emit(Event::CalendarWritten(Err("Read-only mode: the event was not deleted.".into())));
        }
        tokio::spawn(async move { engine.delete_event(calendar_id, event_id, scope, notify).await });
      },
      Command::RespondEvent { calendar_id, event_id, scope, rsvp } => {
        if self.readonly {
          return self.emit(Event::CalendarWritten(Err("Read-only mode: the answer was not sent.".into())));
        }
        tokio::spawn(async move { engine.respond_event(calendar_id, event_id, scope, rsvp).await });
      },
      Command::LoadMail { mailbox, more } => self.load_mail(mailbox, more),
      Command::OpenMail(thread) => {
        tokio::spawn(async move { engine.open_mail(thread).await });
      },
      Command::StopMail => {
        self.stop_mail();
        tokio::spawn(async move { engine.mail.close().await });
      },
      Command::WatchEvents(on) => self.watch_events(on),
      Command::MarkMail { thread, read } => self.write_mail(MailChange::Mark { thread, read }),
      Command::ArchiveMail { thread, archive } => self.write_mail(MailChange::Archive { thread, archive }),
      Command::TrashMail { thread } => self.write_mail(MailChange::Trash { thread }),
      Command::ReplyMail(reply) => self.write_mail(MailChange::Reply(reply)),
      Command::ComposeMail(compose) => self.write_mail(MailChange::Compose {
        compose,
        thread: crate::mail::new_thread_id(),
      }),
      Command::CreateChat { user_ids } => {
        if self.readonly {
          return self.emit(Event::ChatFailed("Read-only mode: the chat was not created.".into()));
        }
        tokio::spawn(async move {
          match engine.create_chat(user_ids).await {
            Err(error) if error.downcast_ref::<SignedOut>().is_none() => {
              engine.emit(Event::ChatFailed(format!("{error:#}")));
            },
            result => engine.report(result),
          }
        });
      },
    }
  }

  /// Changes one conversation and publishes it. False when it is unknown or nothing changed. A hidden or blocked
  /// conversation is never unread, and its mention mark is settled (`settle_mention`) after every change.
  fn update_group(&self, id: &GroupId, change: impl FnOnce(&mut Group)) -> bool {
    let mut guard = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let state = &mut *guard;
    let me = state.me_id();
    let Some(group) = state.groups.get_mut(id) else {
      return false;
    };
    let before = group.clone();
    change(group);
    if group.hidden || group.blocked {
      group.unread = false;
    }
    // A mark known from the stream holds only while the read cursor stays where it was.
    let known = group.mentioned && group.last_read_time == before.last_read_time;
    settle_mention(group, known, state.histories.get(id), &me);
    if *group == before {
      return false;
    }
    self.emit(Event::GroupUpsert(group.clone()));
    true
  }

  /// Settles a conversation's mention mark against its held history after a page of it arrived, publishing the
  /// conversation when the mark changed.
  fn settle_held_mention(&self, id: &GroupId) {
    self.update_group(id, |_| {});
  }

  /// A conversation as held now.
  fn group(&self, id: &GroupId) -> Option<Group> {
    let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    state.groups.get(id).cloned()
  }

  /// When "Mark as unread" marked this conversation, as the server last listed it; 0 when it is not marked.
  fn marked_unread(&self, id: &GroupId) -> Micros {
    let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    state
      .items
      .get(id)
      .and_then(|item| item.read_state.as_ref())
      .and_then(|read| read.mark_as_unread_timestamp_usec)
      .unwrap_or(0)
  }

  /// Keeps the held list item's unread mark in step with a change made here or reported by the stream.
  fn set_marked_unread(&self, id: &GroupId, time: Micros) {
    let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(item) = state.items.get_mut(id) {
      item.read_state.get_or_insert_default().mark_as_unread_timestamp_usec = Some(time);
    }
  }

  /// The raw room notification state the server last listed for a conversation, echoed back when muting.
  fn room_state(&self, id: &GroupId) -> Option<i32> {
    let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    state
      .items
      .get(id)?
      .read_state
      .as_ref()?
      .notification_settings
      .as_ref()?
      .room_state
  }

  /// Keeps the held list item's room notification state in step with a change made here.
  fn set_room_state(&self, id: &GroupId, room_state: Option<i32>) {
    let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(item) = state.items.get_mut(id) {
      item
        .read_state
        .get_or_insert_default()
        .notification_settings
        .get_or_insert_default()
        .room_state = room_state;
    }
  }

  /// Takes a conversation the user is no longer in off the list, publishes the list without it, and saves it.
  fn remove_group(&self, id: &GroupId) {
    {
      let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      if state.groups.remove(id).is_none() {
        return;
      }
      state.items.remove(id);
      self.emit(Event::World(Arc::new(state.groups.values().cloned().collect())));
    }
    self.save_world();
  }

  /// `update_group_notification_settings`: the raw room state (left out when `None`) and the mute state.
  async fn update_notifications(&self, group: &GroupId, room_state: Option<i32>, muted: bool) -> Result<()> {
    use proto::group_mute_setting::MuteState;
    let mute = if muted {
      MuteState::Muted
    } else {
      MuteState::Unmuted
    };
    let request = proto::UpdateGroupNotificationSettingsRequest {
      request_header: Some(request_header()),
      group_id: Some(convert::proto_group_id(group)),
      settings: Some(proto::GroupNotificationSettings {
        state: None,
        room_state,
        mute: Some(proto::GroupMuteSetting { state: Some(mute as i32) }),
      }),
    };
    self
      .http
      .api::<_, proto::UpdateGroupNotificationSettingsResponse>("update_group_notification_settings", &request)
      .await
      .map(|_| ())
  }

  /// `set_mark_as_unread_timestamp`: marks the conversation unread from `time`, or clears the mark with 0.
  async fn set_mark_as_unread(&self, group: &GroupId, time: Micros) -> Result<()> {
    let request = proto::SetMarkAsUnreadTimestampRequest {
      request_header: Some(request_header()),
      group_id: Some(convert::proto_group_id(group)),
      mark_as_unread_timestamp_usec: Some(time),
    };
    self
      .http
      .api::<_, proto::SetMarkAsUnreadTimestampResponse>("set_mark_as_unread_timestamp", &request)
      .await
      .map(|_| ())
  }

  /// Reports a failed conversation change with `failure`, short enough to show; the cause goes to the log.
  fn report_change(&self, result: Result<()>, failure: &str) {
    let Err(error) = result else { return };
    match error.downcast_ref::<SignedOut>() {
      Some(signed_out) => self.emit(Event::SignedOut(signed_out.0.clone())),
      None => {
        tracing::warn!("{failure} {error:#}");
        self.emit(Event::Error(failure.to_string()));
      },
    }
  }

  /// `Command::SetHidden`: page RPC `n7ICcf` with 1 to hide, 0 to bring the conversation back.
  fn set_hidden(self: &Arc<Self>, group: GroupId, hidden: bool) {
    if self.readonly {
      let what = if hidden { "hidden" } else { "unhidden" };
      return self.emit(Event::Error(format!("Read-only mode: the conversation was not {what}.")));
    }
    let Some(before) = self.group(&group) else {
      return;
    };
    self.update_group(&group, |group| group.hidden = hidden);
    let engine = self.clone();
    tokio::spawn(async move {
      let arguments = serde_json::json!([page_group_id(&group), i32::from(hidden)]);
      let result = engine.http.page_rpc("n7ICcf", &arguments).await.map(|_| ());
      if result.is_err() {
        engine.update_group(&group, |group| {
          group.hidden = before.hidden;
          group.unread = before.unread;
          group.mentioned = before.mentioned;
        });
      }
      let failure = if hidden {
        "The conversation was not hidden."
      } else {
        "The conversation was not unhidden."
      };
      engine.report_change(result, failure);
    });
  }

  /// `Command::SetBlocked`: page RPC `BFhA9d`, naming the other person of a DM, or a space. Blocking a space also
  /// leaves it, so it goes from the list once the server agrees; a blocked DM stays, marked.
  fn set_blocked(self: &Arc<Self>, group: GroupId, blocked: bool) {
    let failure = if blocked {
      "The conversation was not blocked."
    } else {
      "The conversation was not unblocked."
    };
    if self.readonly {
      return self.emit(Event::Error(format!("Read-only mode: {}", failure.to_lowercase())));
    }
    let Some(before) = self.group(&group) else {
      return;
    };
    let arguments = match before.kind {
      GroupKind::Space if blocked => block_arguments(&group, None, true),
      GroupKind::Space => return self.emit(Event::Error("A space cannot be unblocked here.".into())),
      GroupKind::Dm => {
        let Some(user) = before.member_ids.first() else {
          return self.emit(Event::Error(format!("{failure} The other person is not known.")));
        };
        block_arguments(&group, Some(user.as_str()), blocked)
      },
      GroupKind::GroupDm | GroupKind::App => {
        return self.emit(Event::Error("Only a person or a space can be blocked.".into()));
      },
    };
    let space = before.kind == GroupKind::Space;
    if !space {
      self.update_group(&group, |group| group.blocked = blocked);
    }
    let engine = self.clone();
    tokio::spawn(async move {
      let result = engine.http.page_rpc("BFhA9d", &arguments).await.map(|_| ());
      match (&result, space) {
        (Ok(()), true) => engine.remove_group(&group),
        (Ok(()), false) => {},
        (Err(_), _) => {
          engine.update_group(&group, |group| {
            group.blocked = before.blocked;
            group.unread = before.unread;
            group.mentioned = before.mentioned;
          });
        },
      }
      engine.report_change(result, failure);
    });
  }

  /// `Command::Leave`: `remove_memberships` naming the signed-in user. The conversation goes from the list once the
  /// server agrees.
  fn leave(self: &Arc<Self>, group: GroupId) {
    if self.readonly {
      return self.emit(Event::Error("Read-only mode: the conversation was not left.".into()));
    }
    let Some(held) = self.group(&group) else {
      return;
    };
    if !matches!(held.kind, GroupKind::Space | GroupKind::GroupDm) {
      return self.emit(Event::Error("Only a space or a group conversation can be left.".into()));
    }
    let me = self.me_id();
    if me.is_empty() {
      return self.emit(Event::Error(
        "The conversation was not left. The signed-in user is not known yet.".into(),
      ));
    }
    let engine = self.clone();
    tokio::spawn(async move {
      let request = proto::RemoveMembershipsRequest {
        request_header: Some(request_header()),
        member_ids: vec![proto::MemberId {
          user_id: Some(proto::UserId { id: Some(me), ..Default::default() }),
          ..Default::default()
        }],
        group_id: Some(convert::proto_group_id(&group)),
        membership_state: None,
      };
      let result = engine
        .http
        .api::<_, proto::RemoveMembershipsResponse>("remove_memberships", &request)
        .await
        .map(|_| ());
      if result.is_ok() {
        engine.remove_group(&group);
      }
      engine.report_change(result, "The conversation was not left.");
    });
  }

  /// `Command::DeleteConversation`: page RPC `U3Vwwb`, spaces only. The space goes from the list once the server
  /// agrees.
  fn delete_conversation(self: &Arc<Self>, group: GroupId) {
    if self.readonly {
      return self.emit(Event::Error("Read-only mode: the space was not deleted.".into()));
    }
    let Some(held) = self.group(&group) else {
      return;
    };
    if held.kind != GroupKind::Space {
      return self.emit(Event::Error("Only a space can be deleted.".into()));
    }
    let engine = self.clone();
    tokio::spawn(async move {
      let arguments = delete_arguments(&group);
      let result = engine.http.page_rpc("U3Vwwb", &arguments).await.map(|_| ());
      if result.is_ok() {
        engine.remove_group(&group);
      }
      engine.report_change(result, "The space was not deleted.");
    });
  }

  /// `Command::SetNotify`: `update_group_notification_settings` with the level's raw room state and the current
  /// mute state, as the web client's dialog sends them.
  fn set_notify(self: &Arc<Self>, group: GroupId, notify: Notify) {
    if self.readonly {
      return self.emit(Event::Error("Read-only mode: notifications were not changed.".into()));
    }
    let Some(before) = self.group(&group) else {
      return;
    };
    let held_room_state = self.room_state(&group);
    let room_state = convert::room_state(notify);
    self.set_room_state(&group, Some(room_state));
    self.update_group(&group, |group| group.notify = convert::notify(group.muted, Some(room_state)));
    let engine = self.clone();
    tokio::spawn(async move {
      let result = engine.update_notifications(&group, Some(room_state), before.muted).await;
      if result.is_err() {
        engine.set_room_state(&group, held_room_state);
        engine.update_group(&group, |group| group.notify = before.notify);
      }
      engine.report_change(result, "Notifications were not changed.");
    });
  }

  /// `Command::MoveToSection`: `apply_group_label` for the new custom section, after `remove_group_label` for the
  /// user's section the conversation is in; back to the section of its kind is the removal alone. The web client was
  /// never seen removing the Meetings label, so a meeting chat only gains the new label and is never moved back.
  fn move_to_section(self: &Arc<Self>, group: GroupId, section: Option<String>) {
    if self.readonly {
      return self.emit(Event::Error("Read-only mode: the conversation was not moved.".into()));
    }
    let Some(before) = self.group(&group) else {
      return;
    };
    let current = match &before.placement {
      Placement::Custom { id, .. } => Some(id.clone()),
      Placement::ByKind | Placement::Meeting => None,
    };
    if current == section || (matches!(before.placement, Placement::Meeting) && section.is_none()) {
      return;
    }
    let placement = section.as_ref().map_or(Placement::ByKind, |id| Placement::Custom {
      id: id.clone(),
      name: self.section_name(id),
    });
    self.update_group(&group, |group| group.placement = placement);
    let engine = self.clone();
    tokio::spawn(async move {
      let result = engine.relabel(&group, current.as_deref(), section.as_deref()).await;
      if result.is_err() {
        engine.update_group(&group, |group| group.placement = before.placement);
      }
      engine.report_change(result, "The conversation was not moved.");
    });
  }

  /// The title of one of the user's sections, as the list names it.
  fn section_name(&self, id: &str) -> String {
    let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    state.sections.get(id).cloned().unwrap_or_else(|| "Section".into())
  }

  /// Takes a conversation out of the custom section `from` and puts it in `to`.
  async fn relabel(&self, group: &GroupId, from: Option<&str>, to: Option<&str>) -> Result<()> {
    if let Some(from) = from {
      let request = proto::RemoveGroupLabelRequest {
        request_header: Some(request_header()),
        group_label_id: Some(section_label(from)),
        group_id: Some(convert::proto_group_id(group)),
      };
      self
        .http
        .api::<_, proto::RemoveGroupLabelResponse>("remove_group_label", &request)
        .await?;
    }
    if let Some(to) = to {
      let request = proto::ApplyGroupLabelRequest {
        request_header: Some(request_header()),
        group_label_id: Some(section_label(to)),
        group_id: Some(convert::proto_group_id(group)),
      };
      self
        .http
        .api::<_, proto::ApplyGroupLabelResponse>("apply_group_label", &request)
        .await?;
    }
    Ok(())
  }

  /// A message held in memory for a conversation.
  fn held(&self, group: &GroupId, message_id: &str) -> Option<Arc<Message>> {
    let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    state
      .histories
      .get(group)?
      .messages
      .iter()
      .find(|message| message.id == message_id)
      .cloned()
  }

  fn remove_message(&self, group: &GroupId, message_id: &str) {
    let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(history) = state.histories.get_mut(group) {
      history.messages.retain(|message| message.id != message_id);
      self.save_history(group, &state.histories[group]);
    }
    self.emit(Event::MessageDeleted {
      group: group.clone(),
      id: message_id.to_string(),
    });
  }

  /// Shows the message at once, before the server knows it, with the formatting its markers asked for. Returns its
  /// local id.
  fn add_pending(
    &self,
    group: &GroupId,
    topic_id: Option<&str>,
    typed: &markup::Parsed,
    quote: Option<Quote>,
    attachments: Vec<Attachment>,
  ) -> Option<String> {
    let me = self
      .state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .me
      .clone()?;
    let local_id = format!("working%{}", rand::random::<u64>());
    let message = Arc::new(Message {
      id: local_id.clone(),
      topic_id: topic_id.unwrap_or_default().to_string(),
      reply: topic_id.is_some(),
      group: group.clone(),
      sender_id: me.id.clone(),
      sender_name: me.name.clone(),
      sender_avatar: me.avatar_url,
      text: typed.text.clone(),
      spans: convert::pending_spans(typed),
      time: now_micros(),
      update_time: 0,
      edited: false,
      system: false,
      reactions: Vec::new(),
      attachments,
      cards: Vec::new(),
      quote,
      pending: true,
      starred: false,
      failed: false,
    });
    self.upsert_message(message, None);
    Some(local_id)
  }

  fn fail_pending(&self, group: &GroupId, local_id: &str) {
    let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(history) = state.histories.get_mut(group) else {
      return;
    };
    let Some(slot) = history.messages.iter_mut().find(|message| message.id == local_id) else {
      return;
    };
    let mut failed = (**slot).clone();
    failed.failed = true;
    *slot = Arc::new(failed);
    self.emit(Event::MessageUpsert {
      message: slot.clone(),
      replaces: None,
      incoming: false,
    });
  }

  /// Uploads the files of one message, all at once, answering in the order they were given.
  async fn upload_files(self: &Arc<Self>, group: &GroupId, files: Vec<PathBuf>) -> Result<Vec<proto::UploadMetadata>> {
    let (GroupId::Dm(id) | GroupId::Space(id)) = group;
    let uploads: Vec<_> = files
      .into_iter()
      .map(|path| {
        let (engine, id) = (self.clone(), id.clone());
        tokio::spawn(async move {
          let name = file_name(&path);
          let bytes = tokio::fs::read(&path)
            .await
            .with_context(|| format!("{name} could not be read"))?;
          let mime = mime_guess::from_path(&path).first_or_octet_stream();
          let upload = engine.http.upload(&id, &name, mime.essence_str(), bytes).await;
          upload.with_context(|| format!("{name} could not be uploaded"))
        })
      })
      .collect();
    let mut done = Vec::with_capacity(uploads.len());
    for upload in uploads {
      done.push(upload.await??);
    }
    Ok(done)
  }

  #[allow(clippy::too_many_arguments, reason = "one message's independent parts")]
  async fn send_message(
    &self,
    group: &GroupId,
    topic_id: Option<&str>,
    typed: &markup::Parsed,
    quoted: Option<&Message>,
    uploads: Vec<proto::UploadMetadata>,
    local_id: &str,
  ) -> Result<()> {
    let mut annotations = convert::chip_annotations(&typed.text, &typed.chips);
    annotations.extend(convert::format_annotations(&typed.text, &typed.spans));
    annotations.extend(uploads.into_iter().map(|upload| proto::Annotation {
      r#type: Some(proto::AnnotationType::UploadMetadata as i32),
      chip_render_type: Some(proto::annotation::ChipRenderType::Render as i32),
      upload_metadata: Some(upload),
      ..Default::default()
    }));
    let sent = match topic_id {
      // A reply joins an existing topic (thread).
      Some(topic_id) => {
        let request = proto::CreateMessageRequest {
          request_header: Some(request_header()),
          parent_id: Some(proto::MessageParentId {
            topic_id: Some(proto::TopicId {
              group_id: Some(convert::proto_group_id(group)),
              topic_id: Some(topic_id.to_string()),
            }),
          }),
          text_body: Some(typed.text.clone()),
          annotations,
          local_id: Some(local_id.to_string()),
          ..Default::default()
        };
        let response: proto::CreateMessageResponse = self.http.api("create_message", &request).await?;
        response.message
      },
      // Anything else starts a topic of its own.
      None => {
        let request = proto::CreateTopicRequest {
          request_header: Some(request_header()),
          group_id: Some(convert::proto_group_id(group)),
          text_body: Some(typed.text.clone()),
          annotations,
          local_id: Some(local_id.to_string()),
          history_v2: Some(true),
          message_info: Some(proto::MessageInfo {
            accept_format_annotations: Some(true),
            reply_to: quoted.map(|quoted| proto::message_info::ReplyTo {
              id: Some(proto_message_id(group, &quoted.topic_id, &quoted.id)),
              last_update_time_micros: Some(if quoted.update_time > 0 {
                quoted.update_time
              } else {
                quoted.time
              }),
              r#type: Some(1),
            }),
          }),
          ..Default::default()
        };
        let response: proto::CreateTopicResponse = self.http.api("create_topic", &request).await?;
        response.topic.and_then(|topic| topic.replies.into_iter().next())
      },
    };
    let converted = sent.and_then(|message| convert::message(&message, &self.users()));
    let message = converted.ok_or_else(|| anyhow!("the server accepted the message but did not return it"))?;
    self.upsert_message(Arc::new(message), Some(local_id));
    Ok(())
  }

  /// Joined members of a conversation, people and apps, by name, without the signed-in user. Members whose
  /// profile the answer leaves out are looked up.
  async fn load_members(self: &Arc<Self>, group: GroupId) -> Result<()> {
    let users = self.members(&group).await?;
    self.emit(Event::Members { group, users: Arc::new(users) });
    Ok(())
  }

  /// Who reacted to a message with one emoji, every page of `list_reactors` followed, in the server's order. Ids the
  /// users map does not know are looked up; people without a name are left out.
  async fn reactors(
    self: &Arc<Self>,
    group: &GroupId,
    topic_id: &str,
    message_id: &str,
    emoji: &str,
  ) -> Result<Vec<User>> {
    let mut ids: Vec<String> = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
      let request = proto::ListReactorsRequest {
        message_id: Some(wire_message_id(group, topic_id.to_string(), message_id.to_string())),
        emoji: Some(wire_emoji(emoji)),
        page_size: None,
        page_token: page_token.clone(),
        request_header: Some(request_header()),
      };
      let response: proto::ListReactorsResponse = self.http.api("list_reactors", &request).await?;
      for id in response.reactors.into_iter().filter_map(|user| user.id) {
        if !ids.contains(&id) {
          ids.push(id);
        }
      }
      match response.next_page_token.filter(|token| !token.is_empty()) {
        // A token the server already gave would loop forever.
        Some(token) if page_token.as_ref() != Some(&token) => page_token = Some(token),
        _ => break,
      }
    }
    let unknown: Vec<String> = {
      let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      ids.iter().filter(|id| !state.users.contains_key(*id)).cloned().collect()
    };
    if !unknown.is_empty() {
      self.fetch_users(unknown).await?;
    }
    let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    Ok(
      ids
        .iter()
        .filter_map(|id| state.users.get(id))
        .filter(|user| !user.name.is_empty())
        .cloned()
        .collect(),
    )
  }

  /// Joined members of a conversation, as `load_members` gives them to the UI.
  pub(crate) async fn members(self: &Arc<Self>, group: &GroupId) -> Result<Vec<User>> {
    let mut ids = Vec::new();
    let mut page_token = None;
    loop {
      let request = proto::ListMembersRequest {
        request_header: Some(request_header()),
        group_id: Some(convert::proto_group_id(group)),
        page_size: Some(500),
        page_token: page_token.take(),
        ..Default::default()
      };
      let response: proto::ListMembersResponse = self.http.api("list_members", &request).await?;
      let users = response
        .members
        .iter()
        .filter_map(|member| member.user.as_ref())
        .filter_map(convert::user);
      self
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .add_users(users);
      let joined = proto::MembershipState::MemberJoined as i32;
      ids.extend(
        response
          .memberships
          .iter()
          .filter(|membership| membership.membership_state.is_none_or(|state| state == joined))
          .filter_map(|membership| membership.id.as_ref()?.member_id.as_ref()?.user_id.as_ref()?.id.clone()),
      );
      match response.next_page_token.filter(|token| !token.is_empty()) {
        Some(token) => page_token = Some(token),
        None => break,
      }
    }
    let me = self.me_id();
    ids.retain(|id| *id != me);
    ids.sort();
    ids.dedup();
    let unknown: Vec<String> = {
      let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      ids.iter().filter(|id| !state.users.contains_key(*id)).cloned().collect()
    };
    if !unknown.is_empty() {
      self.fetch_users(unknown).await?;
    }
    let mut users: Vec<User> = {
      let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      ids
        .iter()
        .filter_map(|id| state.users.get(id))
        .filter(|user| !user.name.is_empty() && !user.deleted)
        .cloned()
        .collect()
    };
    users.sort_by_cached_key(|user| user.name.to_lowercase());
    Ok(users)
  }

  /// The commands of the apps in a conversation. The web app's other `/` entries (Drive, Calendar, Docs) are not
  /// apps of the conversation and are not asked for.
  async fn load_slash_commands(&self, group: GroupId) -> Result<()> {
    let request = proto::AutocompleteSlashCommandsRequest {
      request_header: Some(request_header()),
      query: Some(String::new()),
      group_id: Some(convert::proto_group_id(&group)),
      max_num_results: Some(200),
      restrict_to_bots_in_group: Some(true),
      ..Default::default()
    };
    let response: proto::AutocompleteSlashCommandsResponse =
      self.http.api("autocomplete_slash_commands", &request).await?;
    let commands = convert::slash_commands(&response);
    self.emit(Event::SlashCommands { group, commands: Arc::new(commands) });
    Ok(())
  }

  /// Asks the server for a new meeting, then posts the annotation it answers with as a message of its own.
  async fn start_meet(&self, group: &GroupId, local_id: &str) -> Result<()> {
    let request = proto::CreateVideoCallRequest {
      group_id: Some(convert::proto_group_id(group)),
      request_header: Some(request_header()),
    };
    let call: proto::CreateVideoCallResponse = self.http.api("create_video_call", &request).await?;
    let annotation = call.annotation.ok_or_else(|| anyhow!("the server did not return a meeting"))?;
    let request = proto::CreateTopicRequest {
      request_header: Some(request_header()),
      group_id: Some(convert::proto_group_id(group)),
      text_body: Some(String::new()),
      annotations: vec![annotation],
      local_id: Some(local_id.to_string()),
      history_v2: Some(true),
      message_info: Some(proto::MessageInfo {
        accept_format_annotations: Some(true),
        ..Default::default()
      }),
      ..Default::default()
    };
    let response: proto::CreateTopicResponse = self.http.api("create_topic", &request).await?;
    let sent = response.topic.and_then(|topic| topic.replies.into_iter().next());
    let converted = sent.and_then(|message| convert::message(&message, &self.users()));
    let message = converted.ok_or_else(|| anyhow!("the server accepted the meeting but did not return it"))?;
    let url = message
      .attachments
      .iter()
      .find(|att| att.kind == AttachmentKind::Meet)
      .map(|att| att.url.clone());
    self.upsert_message(Arc::new(message), Some(local_id));
    if let Some(url) = url {
      self.emit(Event::MeetStarted(url));
    }
    Ok(())
  }

  /// Meet's "New meeting": a meeting of its own, in no conversation and on no calendar. Answers
  /// `Event::MeetingCreated` whether or not it worked, so whoever waits for it can stop.
  async fn create_meeting(&self) {
    let result = self.request_meeting().await.map_err(|error| self.meeting_failure(&error));
    self.emit(Event::MeetingCreated(result));
  }

  /// The web app's call button without a conversation to post the call in: the server makes the meeting and answers
  /// with the video call it would have posted, which is never posted. The answer is the meeting's link.
  async fn request_meeting(&self) -> Result<String> {
    let request = proto::CreateVideoCallRequest {
      group_id: None,
      request_header: Some(request_header()),
    };
    let call: proto::CreateVideoCallResponse = self.http.api("create_video_call", &request).await?;
    meeting_link(&call).context("the server did not return a meeting")
  }

  /// What the user reads when no meeting was made, the cause going to the log. A refused session also signs the app
  /// out, as it does for any command.
  fn meeting_failure(&self, error: &anyhow::Error) -> String {
    if let Some(signed_out) = error.downcast_ref::<SignedOut>() {
      self.emit(Event::SignedOut(signed_out.0.clone()));
      return signed_out.0.clone();
    }
    tracing::warn!("the meeting was not created: {error:#}");
    "The meeting could not be created. Try again in a moment.".into()
  }

  /// Stores a message and publishes it, updating the conversation's preview. `local_id` names the pending
  /// message this one confirms.
  fn upsert_message(&self, message: Arc<Message>, local_id: Option<&str>) {
    self.upsert_listed(message, local_id);
  }

  /// Puts a message in its conversation and tells the UI. Answers whether the conversation is in the list and not
  /// hidden. A conversation not loaded keeps its disk cache as it is: only a message being sent is held before it
  /// is loaded, in a partial history.
  fn upsert_listed(&self, message: Arc<Message>, local_id: Option<&str>) -> bool {
    let mut guard = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let state = &mut *guard;
    let me_id = state.me_id();
    let open = state.open.as_ref() == Some(&message.group);
    let listed = state.groups.get(&message.group).is_some_and(|group| !group.hidden);
    if message.pending && !state.histories.contains_key(&message.group) {
      state.hold(&message.group).partial = true;
    }
    let mut replaces = None;
    let mut shown = true;
    // Without a history to compare with, an edit or the message the list already shows is not new.
    let mut is_new = !message.edited
      && state.groups.get(&message.group).is_none_or(|group| {
        group
          .snippet
          .as_ref()
          .is_none_or(|snippet| snippet.time != message.time || snippet.sender_id != message.sender_id)
      });
    if let Some(history) = state.histories.get_mut(&message.group) {
      let by_id = history.messages.iter().position(|held| held.id == message.id);
      let by_local =
        local_id.and_then(|local| history.messages.iter().position(|held| held.pending && held.id == local));
      is_new = by_id.is_none() && by_local.is_none();
      match (by_id, by_local) {
        (Some(index), stale_pending) => {
          history.messages[index] = message.clone();
          // The stream confirmed it first; the pending copy is now a duplicate.
          if let Some(pending) = stale_pending.filter(|pending| *pending != index) {
            replaces = Some(history.messages.remove(pending).id.clone());
          }
        },
        (None, Some(index)) => {
          replaces = Some(history.messages[index].id.clone());
          history.messages[index] = message.clone();
        },
        (None, None) => {
          // A reply joins its topic; everything else is the newest message.
          let topic_end = (!message.topic_id.is_empty())
            .then(|| history.messages.iter().rposition(|held| held.topic_id == message.topic_id))
            .flatten();
          match topic_end {
            Some(index) => history.messages.insert(index + 1, message.clone()),
            // A window into the past does not grow at the end; the message shows once the latest are loaded.
            None if history.detached => shown = false,
            None => history.messages.push(message.clone()),
          }
        },
      }
      if is_new && !message.pending {
        history.note_topic(&message);
      }
      if !message.pending {
        self.save_history(&message.group, history);
      }
    }
    if shown {
      // A hidden or blocked conversation does not notify.
      let quiet = state
        .groups
        .get(&message.group)
        .is_some_and(|group| group.hidden || group.blocked);
      let incoming = is_new && !message.pending && message.sender_id != me_id && !quiet;
      self.emit(Event::MessageUpsert {
        message: message.clone(),
        replaces,
        incoming,
      });
    }

    if is_new && let Some(group) = state.groups.get_mut(&message.group) {
      group.sort_time = group.sort_time.max(message.time);
      group.snippet = Some(Snippet {
        sender_id: message.sender_id.clone(),
        sender_name: message.sender_name.clone(),
        text: message.preview(),
        time: message.time,
      });
      let mut known = group.mentioned;
      if message.sender_id == me_id {
        group.unread = false;
      } else if !open && !group.hidden && !group.blocked {
        group.unread = true;
        known |= message.mentions(&me_id);
      }
      settle_mention(group, known, state.histories.get(&message.group), &me_id);
      self.emit(Event::GroupUpsert(group.clone()));
    }
    listed
  }

  /// Marks a held message starred or not and tells the UI.
  fn apply_star(&self, group: &GroupId, message_id: &str, starred: bool) {
    let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(slot) = state
      .histories
      .get_mut(group)
      .and_then(|history| history.messages.iter_mut().find(|message| message.id == message_id))
    else {
      return;
    };
    if slot.starred == starred {
      return;
    }
    let mut message = (**slot).clone();
    message.starred = starred;
    *slot = Arc::new(message);
    self.emit(Event::MessageUpsert {
      message: slot.clone(),
      replaces: None,
      incoming: false,
    });
  }

  fn apply_reaction(&self, group: &GroupId, message_id: &str, emoji: &str, reactor: &str, add: bool) {
    let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mine = state.me.as_ref().is_some_and(|me| me.id == reactor);
    let Some(history) = state.histories.get_mut(group) else {
      return;
    };
    let Some(slot) = history.messages.iter_mut().find(|message| message.id == message_id) else {
      return;
    };
    let mut message = (**slot).clone();
    let index = message.reactions.iter().position(|reaction| reaction.emoji == emoji);
    match (index, add) {
      (Some(index), true) => {
        let reaction = &mut message.reactions[index];
        // The signed-in user's own reaction arrives twice: applied on click, then echoed by the stream.
        if mine && reaction.mine {
          return;
        }
        reaction.count += 1;
        reaction.mine |= mine;
      },
      (None, true) => message.reactions.push(Reaction { emoji: emoji.to_string(), count: 1, mine }),
      (Some(index), false) => {
        let reaction = &mut message.reactions[index];
        if mine && !reaction.mine {
          return;
        }
        reaction.count = reaction.count.saturating_sub(1);
        reaction.mine &= !mine;
        if reaction.count == 0 {
          message.reactions.remove(index);
        }
      },
      (None, false) => return,
    }
    *slot = Arc::new(message);
    self.emit(Event::MessageUpsert {
      message: slot.clone(),
      replaces: None,
      incoming: false,
    });
  }

  // ---- Agents (the MCP server) ----

  async fn set_mcp(self: Arc<Self>, config: Option<McpConfig>) {
    let mut running = self.mcp.lock().await;
    if let Some(previous) = running.take() {
      previous.stop().await;
    }
    // Nothing to serve when every tool group is off: the port stays free.
    let Some(config) = config.filter(|config| config.chat || config.mail || config.calendar || config.meet) else {
      return self.emit(Event::Mcp(McpStatus::Off));
    };
    match mcp::serve(self.clone(), config).await {
      Ok(server) => {
        let address = server.address;
        *running = Some(server);
        self.emit(Event::Mcp(McpStatus::Running { address }));
      },
      Err(error) => self.emit(Event::Mcp(McpStatus::Failed(error.to_string()))),
    }
  }

  pub(crate) fn me(&self) -> Option<User> {
    self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).me.clone()
  }

  /// Every conversation the user sees in the lists (not hidden, not blocked) with the text it can be found by: its
  /// name and, for a one-to-one chat, the other side's email.
  pub(crate) fn directory(&self) -> Vec<(Group, String)> {
    let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    state
      .groups
      .values()
      .filter(|group| !group.hidden && !group.blocked)
      .map(|group| {
        let mut text = group.name.clone();
        if matches!(group.kind, GroupKind::Dm | GroupKind::App) {
          let emails = group
            .member_ids
            .iter()
            .filter_map(|id| state.users.get(id))
            .map(|user| user.email.as_str());
          for email in emails.filter(|email| !email.is_empty()) {
            text.push(' ');
            text.push_str(email);
          }
        }
        (group.clone(), text)
      })
      .collect()
  }

  /// One page of a conversation without touching what the app shows. The latest page of a conversation the stream
  /// has kept complete since it was loaded comes from what is held; anything else is fetched.
  pub(crate) async fn topics(&self, group: &GroupId, page: Page) -> Result<TopicPage> {
    if matches!(page, Page::Latest)
      && let Some(held) = self.held_latest(group)
    {
      return Ok(held);
    }
    let response = self.list_topics(group, page).await?;
    let users = self.users();
    let mut topics: Vec<(Micros, Vec<Arc<Message>>)> = response
      .topics
      .iter()
      .map(|topic| {
        let messages = topic.replies.iter().filter_map(|message| convert::message(message, &users));
        (topic.sort_time.unwrap_or(0), messages.map(Arc::new).collect())
      })
      .collect();
    topics.sort_by_key(|(time, _)| *time);
    Ok(TopicPage {
      topics,
      reached_start: response.contains_first_topic.unwrap_or(false),
      reached_end: response.contains_last_topic.unwrap_or(false),
    })
  }

  /// The newest `LATEST_PAGE` topics of a conversation held complete and current, as `list_topics` would answer them.
  fn held_latest(&self, group: &GroupId) -> Option<TopicPage> {
    let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let history = state
      .histories
      .get(group)
      .filter(|history| history.fresh && !history.detached && !history.partial)?;
    let mut topics = history.topics();
    let older = topics.len().saturating_sub(SAVED_TOPICS);
    let reached_start = history.reached_start && older == 0;
    drop(state);
    Some(TopicPage {
      topics: topics.split_off(older),
      reached_start,
      reached_end: true,
    })
  }

  /// Sends a message and waits for the server to take it. The app shows it like one typed there, and its text is
  /// read for Google Chat's markers in the same way. `chips` are the mentions in `text`.
  pub(crate) async fn send_now(
    self: &Arc<Self>,
    group: &GroupId,
    topic_id: Option<&str>,
    text: &str,
    chips: &[Chip],
    files: Vec<PathBuf>,
  ) -> Result<()> {
    if self.readonly {
      return Err(anyhow!("Working runs in read-only mode"));
    }
    let attached = files.iter().map(|path| local_attachment(path)).collect();
    let typed = markup::parse(text, chips);
    let pending = self
      .add_pending(group, topic_id, &typed, None, attached)
      .ok_or_else(|| anyhow!("the account is not loaded yet"))?;
    let result = async {
      let uploads = self.upload_files(group, files).await?;
      self.send_message(group, topic_id, &typed, None, uploads, &pending).await
    }
    .await;
    if result.is_err() {
      self.fail_pending(group, &pending);
    }
    result
  }

  // ---- Stream ----

  async fn stream_loop(self: Arc<Self>, mut stream: UnboundedReceiver<ChannelEvent>) {
    while let Some(event) = stream.recv().await {
      match event {
        ChannelEvent::Online => {
          let previous = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .connection
            .replace(Connection::Online);
          if previous == Some(Connection::Online) {
            continue;
          }
          self.emit(Event::Connection(Connection::Online));
          if previous == Some(Connection::Offline) {
            // Events were missed while offline: reload the list (and the mention marks with it) and the conversation
            // on screen.
            tokio::spawn({
              let engine = self.clone();
              async move {
                let result = engine.sync_world_and_mentions(false).await;
                engine.report(result);
              }
            });
            let open = self
              .state
              .lock()
              .unwrap_or_else(std::sync::PoisonError::into_inner)
              .open
              .clone();
            if let Some(group) = open {
              let engine = self.clone();
              tokio::spawn(async move {
                let result = engine.refresh_history(group).await;
                engine.report(result);
              });
            }
          }
        },
        ChannelEvent::Offline => {
          let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
          if state.connection.replace(Connection::Offline) != Some(Connection::Offline) {
            self.emit(Event::Connection(Connection::Offline));
          }
          for history in state.histories.values_mut() {
            history.fresh = false;
            history.fetched = false;
          }
        },
        ChannelEvent::Event(event) => self.on_event(*event),
      }
    }
  }

  fn on_event(self: &Arc<Self>, event: proto::Event) {
    use proto::event::EventType;
    let event_group = event.group_id.as_ref().and_then(convert::group_id);
    let bodies = event.body.iter().chain(&event.bodies);
    // One look at the state for the whole event; messages convert outside the lock.
    let (users, me) = {
      let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      (Arc::clone(&state.users), state.me_id())
    };
    for body in bodies {
      if let Some(posted) = body.message_posted.as_ref().and_then(|posted| posted.message.as_ref())
        && let Some(message) = convert::message(posted, &users)
      {
        // A new message may bring a hidden conversation back; the server's list says whether it did.
        if !self.upsert_listed(Arc::new(message), posted.local_id.as_deref()) {
          self.queue_world_refresh();
        }
      }
      if let Some(deleted) = &body.message_deleted {
        self.on_deleted(deleted);
      }
      if let Some(reaction) = &body.message_reaction {
        self.on_reaction(reaction);
      }
      if let Some(changed) = &body.read_receipt_changed {
        self.on_read_receipts(changed, event_group.as_ref());
      }
      // Read state moved, here or on another device.
      let viewed = body
        .group_viewed
        .as_ref()
        .map(|viewed| (viewed.group_id.as_ref(), viewed.view_time, None));
      let read_state = body.group_read_state_updated_event.as_ref().map(|updated| {
        (
          updated.group_id.as_ref(),
          updated.most_recent_read_time,
          updated.mark_as_unread_time,
        )
      });
      for (group, read_time, mark) in viewed.into_iter().chain(read_state) {
        let Some(group) = group.and_then(convert::group_id).or_else(|| event_group.clone()) else {
          continue;
        };
        if let Some(mark) = mark {
          self.set_marked_unread(&group, mark);
        }
        let marked_unread = mark.unwrap_or(0) > 0;
        let me = me.as_str();
        self.update_group(&group, |group| {
          group.last_read_time = group.last_read_time.max(read_time.unwrap_or(0));
          let newer = group
            .snippet
            .as_ref()
            .is_some_and(|snippet| snippet.time > group.last_read_time && snippet.sender_id != me);
          group.unread = marked_unread || newer;
        });
      }
      if let Some(marked) = &body.mark_as_unread_event {
        let group = marked
          .group_id
          .as_ref()
          .and_then(convert::group_id)
          .or_else(|| event_group.clone());
        if let Some(group) = group {
          // 0 is the mark being cleared, as "Mark as read" does.
          let time = marked.updated_mark_as_unread_timestamp_usec;
          if let Some(time) = time {
            self.set_marked_unread(&group, time);
          }
          if time != Some(0) {
            self.update_group(&group, |group| group.unread = true);
          }
        }
      }
      if let Some(typing) = &body.typing_state_changed_event {
        self.on_typing(typing);
      }
      if let Some(updated) = &body.user_status_updated_event
        && let Some(status) = &updated.user_status
      {
        let id = status.user_id.as_ref().and_then(|id| id.id.as_deref()).unwrap_or("");
        if id.is_empty() || id == me {
          self.emit(Event::Mine(mine_from(status)));
        }
      }
      let kind = body
        .event_type
        .or(event.r#type)
        .and_then(|value| EventType::try_from(value).ok());
      // Anything that reshapes the list (pins, renames, joins, leaves) is cheaper to reload than to mirror.
      if matches!(
        kind,
        Some(
          EventType::GroupUpdated
            | EventType::GroupStarred
            | EventType::MembershipChanged
            | EventType::GroupHideChanged
            | EventType::GroupDeleted
            | EventType::UserAddedToGroup
            | EventType::UserRemovedFromGroup
            | EventType::GroupNotificationSettingsUpdated
            | EventType::WorldRefreshed
            | EventType::GroupLabelApplied
            | EventType::GroupLabelRemoved
            | EventType::ClearHistory
        )
      ) {
        self.queue_world_refresh();
      }
    }
  }

  fn on_deleted(&self, deleted: &proto::MessageDeletedEvent) {
    let Some(id) = &deleted.message_id else {
      return;
    };
    let topic = id.parent_id.as_ref().and_then(|parent| parent.topic_id.as_ref());
    let Some(group) = topic.and_then(|topic| topic.group_id.as_ref()).and_then(convert::group_id) else {
      return;
    };
    let Some(message_id) = id.message_id.clone() else {
      return;
    };
    self.remove_message(&group, &message_id);
  }

  fn on_reaction(&self, reaction: &proto::MessageReactionEvent) {
    let Some(id) = &reaction.message_id else {
      return;
    };
    let topic = id.parent_id.as_ref().and_then(|parent| parent.topic_id.as_ref());
    let Some(group) = topic.and_then(|topic| topic.group_id.as_ref()).and_then(convert::group_id) else {
      return;
    };
    let Some(emoji) = reaction.emoji.as_ref().and_then(convert::emoji_text) else {
      return;
    };
    let reactor = reaction
      .reactor
      .as_ref()
      .and_then(|reactor| reactor.id.clone())
      .unwrap_or_default();
    let add = reaction.option != Some(proto::ReactionOption::Remove as i32);
    self.apply_reaction(&group, id.message_id.as_deref().unwrap_or_default(), &emoji, &reactor, add);
  }

  fn on_read_receipts(&self, changed: &proto::ReadReceiptChangedEvent, event_group: Option<&GroupId>) {
    let group = changed
      .group_id
      .as_ref()
      .and_then(convert::group_id)
      .or_else(|| event_group.cloned());
    if let (Some(group), Some(set)) = (group, &changed.read_receipt_set) {
      self.note_receipts(&group, set, true);
    }
  }

  fn on_typing(&self, typing: &proto::TypingStateChangedEvent) {
    let context = typing.context.as_ref();
    let group = context
      .and_then(|context| {
        context
          .group_id
          .as_ref()
          .or(context.topic_id.as_ref().and_then(|topic| topic.group_id.as_ref()))
      })
      .and_then(convert::group_id);
    let Some(group) = group else { return };
    let user_id = typing.user_id.as_ref().and_then(|id| id.id.clone()).unwrap_or_default();
    let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if state.me.as_ref().is_some_and(|me| me.id == user_id) {
      return;
    }
    let user_name = state.users.get(&user_id).map_or_default(|user| user.name.clone());
    self.emit(Event::Typing {
      group,
      user_id,
      user_name,
      typing: typing.state == Some(proto::TypingState::Typing as i32),
    });
  }
}

/// Google Calendar: reads answer with the events, writes with `Event::CalendarWritten` and then the events again.
impl Engine {
  async fn load_events(&self, start: Micros, end: Micros) {
    match self.calendar.events(start, end).await {
      Ok(events) => self.emit(Event::CalendarEvents { start, end, events: Arc::new(events) }),
      Err(error) => self.emit(Event::CalendarFailed(format!("{error:#}"))),
    }
  }

  /// Tells the UI how a write went and, when it went through, reads the range on screen again.
  async fn calendar_written(&self, result: Result<()>) {
    let written = result.is_ok();
    self.emit(Event::CalendarWritten(result.map_err(|error| format!("{error:#}"))));
    let range = *self.calendar_range.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if written {
      self.events_written.notify_one();
    }
    if let Some((start, end)) = range.filter(|_| written) {
      self.load_events(start, end).await;
    }
  }

  async fn save_event(&self, draft: EventDraft) {
    let result = self.calendar.save(&draft).await;
    self.calendar_written(result).await;
  }

  async fn delete_event(&self, calendar_id: String, event_id: String, scope: EditScope, notify: bool) {
    let result = self.calendar.delete(&calendar_id, &event_id, scope, notify).await;
    self.calendar_written(result).await;
  }

  async fn respond_event(&self, calendar_id: String, event_id: String, scope: EditScope, rsvp: Rsvp) {
    let result = self.calendar.respond(&calendar_id, &event_id, scope, rsvp).await;
    self.calendar_written(result).await;
  }

  /// `Command::WatchEvents`: on starts the reminder watch unless it runs; off stops it, and what it told is forgotten
  /// with it.
  fn watch_events(self: &Arc<Self>, on: bool) {
    let mut watch = self.event_watch.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match (on, watch.take()) {
      (true, Some(running)) => *watch = Some(running),
      (true, None) => *watch = Some(tokio::spawn(self.clone().event_watch()).abort_handle()),
      (false, Some(running)) => running.abort(),
      (false, None) => {},
    }
  }

  /// Reads the next `EVENT_WINDOW` of events every `EVENT_REFRESH` (and after each calendar write) and tells
  /// `Event::EventReminder` when an event's notification time comes, each occurrence and start once.
  async fn event_watch(self: Arc<Self>) {
    let started = now_micros();
    let mut sent = HashSet::new();
    let mut events = Vec::new();
    let mut read_at: Option<tokio::time::Instant> = None;
    loop {
      if read_at.is_none_or(|at| at.elapsed() >= EVENT_REFRESH) {
        let now = now_micros();
        match self.calendar.events(now, now.saturating_add(EVENT_WINDOW)).await {
          Ok(list) => events = list,
          Err(error) => tracing::warn!("the event reminders could not read the calendars: {error:#}"),
        }
        read_at = Some(tokio::time::Instant::now());
      }
      let defaults = self.calendar.default_reminders();
      let now = now_micros();
      for (event, minutes) in crate::calendar::due_reminders(&events, &defaults, &mut sent, started, now) {
        self.emit(Event::EventReminder { event, minutes });
      }
      let refresh_in = EVENT_REFRESH.saturating_sub(read_at.map_or(Duration::ZERO, |at| at.elapsed()));
      let wait = crate::calendar::next_reminder(&events, &defaults, &sent, started, now).map_or(refresh_in, |at| {
        refresh_in.min(Duration::from_micros(u64::try_from(at.saturating_sub(now)).unwrap_or(0)))
      });
      tokio::select! {
        () = tokio::time::sleep(wait) => {},
        () = self.events_written.notified() => read_at = None,
      }
    }
  }
}

/// How often the event reminder watch reads the events again.
const EVENT_REFRESH: Duration = Duration::from_secs(300);
/// How far ahead the event reminder watch reads.
const EVENT_WINDOW: Micros = 24 * 3600 * 1_000_000;

/// How often the watched inbox is read again.
const MAIL_POLL: Duration = Duration::from_secs(60);
/// Inbox threads the disk cache keeps: about the first page.
const SAVED_MAIL: usize = 50;

/// Gmail as the engine holds it.
#[derive(Default)]
struct MailHeld {
  /// Bumped by `Command::StopMail`: an answer to a call made before it is dropped.
  generation: u64,
  /// Each mailbox's threads as last told.
  lists: HashMap<Mailbox, MailList>,
  /// The inbox watch while it runs.
  watch: Option<tokio::task::AbortHandle>,
  /// The cached inbox was shown since the start or the last `StopMail`.
  cache_shown: bool,
  /// Unread inbox threads as last told.
  unread: Option<u32>,
  /// The user's labels as last told.
  labels: Option<Vec<MailLabel>>,
  /// What counts as new mail.
  arrivals: crate::mail::Arrivals,
  /// The watch's last read failed: a failure is told once until a read works again.
  watch_failed: bool,
  /// The thread last opened, with its emails as last told, so a change shows in it at once.
  open: Option<(String, Vec<MailMessage>)>,
  /// The user's changes, laid over list answers that may predate them.
  changes: crate::mail::Changes,
  /// Threads the user archived, as they were in the inbox, to put back at once when they are moved back.
  archived: HashMap<String, MailThread>,
  /// The signatures last told with `Event::MailSignatures` (for new emails, for replies), once the inbox was read.
  signatures: Option<(Option<MailSignature>, Option<MailSignature>)>,
  /// Drafts an agent saved and has not sent, by message id: what each send needs. Kept through `StopMail` (the
  /// drafts stay in Gmail); gone when Working stops, leaving them in Gmail's Drafts.
  drafts: HashMap<String, crate::mail::Draft>,
}

/// A change to Gmail the user asked for.
enum MailChange {
  Mark {
    thread: String,
    read: bool,
  },
  Archive {
    thread: String,
    archive: bool,
  },
  /// Move a thread to Trash.
  Trash {
    thread: String,
  },
  Reply(Box<MailReply>),
  /// A new email, in the thread `thread` the engine minted for it.
  Compose {
    compose: Box<MailCompose>,
    thread: String,
  },
  /// A forward, a new email in the thread `thread` the engine minted for it.
  Forward {
    forward: Box<MailForward>,
    thread: String,
  },
  /// Star or unstar a thread.
  Star {
    thread: String,
    starred: bool,
  },
  /// The user's labels (ids `^x_…`) put on every email of a thread and taken off.
  Label {
    thread: String,
    add: Vec<String>,
    remove: Vec<String>,
  },
  /// A new draft saved as the email `id` in `thread` (a reply's own, a new email's minted one) and held for its send.
  Draft {
    draft: Box<MailDraft>,
    thread: String,
    id: String,
  },
  /// The held draft `id`, in `thread`, sent.
  SendDraft {
    id: String,
    thread: String,
  },
}

/// One entry of the mail write queue: a change to write, or a command already refused (read-only, Mail off) whose
/// answer keeps its place in line.
enum MailJob {
  Write {
    change: MailChange,
    /// `MailHeld::generation` when it was shown.
    generation: u64,
    undo: Box<MailUndo>,
    /// An agent's write (MCP) is answered here, not with `Event::MailWritten`, which the app matches to its own
    /// commands in order.
    answer: Option<MailAnswer>,
  },
  Refused(&'static str),
}

/// Where an agent's mail write learns how it went.
type MailAnswer = flume::Sender<Result<(), String>>;

/// What a mail change altered at once, to put back when Gmail does not take it.
#[derive(Default)]
struct MailUndo {
  /// Threads as they were, by mailbox.
  threads: Vec<(Mailbox, MailThread)>,
  /// A thread taken out of the inbox list, with its place there.
  inbox_removed: Option<(usize, MailThread)>,
  /// A thread put into the inbox list.
  inbox_added: Option<String>,
  /// Threads taken out of another list (Starred, a label's), with their place there.
  removed: Vec<(Mailbox, usize, MailThread)>,
  /// Threads put into another list (Starred, a label's).
  added: Vec<(Mailbox, String)>,
  /// The open thread's emails as they were.
  open: Option<(String, Vec<MailMessage>)>,
  unread: Option<u32>,
  labels: Option<Vec<MailLabel>>,
}

/// What a mail write answers under `WORKING_READONLY=1`.
const MAIL_READONLY: &str = "Working runs in read-only mode";
/// What a mail write answers while Mail is off.
const MAIL_OFF: &str = "Mail is turned off in Working.";
/// How long after a reply its thread does not count as new mail (Gmail's clock may run ahead of this one).
const OWN_REPLY_MARGIN: Micros = 60_000_000;

/// Until when a thread the user just wrote into is theirs, not new mail.
fn own_until() -> Micros {
  now_micros().saturating_add(OWN_REPLY_MARGIN)
}

/// One more or one fewer unread thread on the label `id`.
fn shift_label(labels: &mut Option<Vec<MailLabel>>, id: &str, more: bool) {
  if let Some(label) = labels.iter_mut().flatten().find(|label| label.id == id) {
    label.unread = if more {
      label.unread.saturating_add(1)
    } else {
      label.unread.saturating_sub(1)
    };
  }
}

/// What sending or saving over a draft Working does not hold answers.
fn draft_gone(id: &str) -> String {
  format!(
    "Working does not hold the draft {id}: it was sent, or Working restarted since it was saved. A draft not sent is \
     still in Gmail's Drafts, for the user to send there."
  )
}

/// Puts `thread` into `list` in time order. False when it is older than every thread held while Gmail has older ones
/// still to read: its place may be on a page not read yet.
fn insert_by_time(list: &mut MailList, thread: MailThread) -> bool {
  let newer = list.threads.iter().take_while(|other| other.time >= thread.time).count();
  if newer == list.threads.len() && list.more {
    return false;
  }
  list.threads.insert(newer, thread);
  true
}

/// One mailbox's threads as held.
#[derive(Default)]
struct MailList {
  /// Newest first.
  threads: Vec<MailThread>,
  /// Pages read: the index of the next older page.
  pages: u32,
  /// Gmail has older threads than these.
  more: bool,
  /// An older page is on its way.
  loading_more: bool,
}

/// Who asked for a page of threads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MailRead {
  /// `Command::LoadMail`: always answered.
  Asked,
  /// The inbox watch: told only when something changed.
  Watched,
}

impl MailList {
  /// The page a `LoadMail` reads: the newest, or with `more` the next older one. `None` when it reads none and what
  /// is held is its answer: nothing older is there, or the next page is already on its way. Every `LoadMail` gets
  /// one answer, so a request is never left without one.
  const fn next_read(&mut self, more: bool) -> Option<u32> {
    if !more || self.pages == 0 {
      return Some(0);
    }
    if self.loading_more || !self.more {
      return None;
    }
    self.loading_more = true;
    Some(self.pages)
  }

  /// Puts page `page` into the list. True when the threads or `more` changed.
  fn take(&mut self, page: u32, threads: Vec<MailThread>, more: bool, read: MailRead) -> bool {
    let before = (self.threads.len(), self.more);
    let changed = if page == 0 {
      let threads = match read {
        MailRead::Asked => threads,
        MailRead::Watched => crate::mail::merge_newest(&self.threads, threads, !more),
      };
      // Older pages held keep their own `more`, unless the newest page is the whole mailbox.
      if read == MailRead::Asked || self.pages <= 1 || !more {
        self.more = more;
      }
      self.pages = if read == MailRead::Asked {
        1
      } else {
        self.pages.max(1)
      };
      let changed = threads != self.threads;
      self.threads = threads;
      changed
    } else {
      crate::mail::append_older(&mut self.threads, threads);
      self.pages = page.saturating_add(1);
      self.more = more;
      self.loading_more = false;
      self.threads.len() != before.0
    };
    changed || self.more != before.1
  }
}

/// Gmail, read only: lists answer with `Event::MailThreads`, threads with `Event::MailMessages`, and every failure
/// with `Event::MailFailed`. A Gmail that asks to sign in again signs the whole app out (`ServiceSession`).
impl Engine {
  fn mail_lock(&self) -> std::sync::MutexGuard<'_, MailHeld> {
    self.mail_held.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
  }

  fn load_mail(self: &Arc<Self>, mailbox: Mailbox, more: bool) {
    let mut guard = self.mail_lock();
    let held = &mut *guard;
    let generation = held.generation;
    let inbox = mailbox == Mailbox::Inbox;
    if inbox && held.watch.is_none() {
      held.watch = Some(tokio::spawn(self.clone().mail_watch(generation)).abort_handle());
    }
    let show_cache = inbox && !std::mem::replace(&mut held.cache_shown, true);
    let list = held.lists.entry(mailbox.clone()).or_default();
    let Some(page) = list.next_read(more) else {
      self.emit(Event::MailThreads {
        mailbox,
        threads: Arc::new(list.threads.clone()),
        more: list.more,
      });
      return;
    };
    drop(guard);
    let engine = self.clone();
    tokio::spawn(async move {
      if show_cache {
        engine.show_cached_mail(generation).await;
      }
      engine.read_mail_page(&mailbox, page, generation, MailRead::Asked).await;
    });
  }

  /// Shows the inbox as last saved, unless Gmail answered first.
  async fn show_cached_mail(&self, generation: u64) {
    let cache = self.cache.clone();
    let Some(cached) = tokio::task::spawn_blocking(move || cache.mail()).await.ok().flatten() else {
      return;
    };
    let held = self.mail_lock();
    let loaded = held.lists.get(&Mailbox::Inbox).is_some_and(|list| list.pages > 0);
    if held.generation == generation && !loaded {
      // Told under the lock, so Gmail's answer cannot overtake it.
      self.emit(Event::MailThreads {
        mailbox: Mailbox::Inbox,
        threads: Arc::new(cached.threads),
        more: cached.more,
      });
    }
  }

  /// Reads the inbox again every `MAIL_POLL` until `Command::StopMail` or the core stops.
  async fn mail_watch(self: Arc<Self>, generation: u64) {
    loop {
      tokio::time::sleep(MAIL_POLL).await;
      self.read_mail_page(&Mailbox::Inbox, 0, generation, MailRead::Watched).await;
    }
  }

  /// Reads page `page` of a mailbox and takes it in, the user's changes Gmail may not have had yet laid over it.
  async fn read_mail_page(&self, mailbox: &Mailbox, page: u32, generation: u64, read: MailRead) {
    let requested = self.mail_lock().changes.request();
    let result = self.mail.threads(mailbox, page).await;
    let read_well = result.is_ok();
    self.mail_read(mailbox, page, result, generation, read, requested);
    if read_well && *mailbox == Mailbox::Inbox && page == 0 {
      // The session that answered holds the page's settings, so this asks Gmail for nothing more.
      if let Ok(signatures) = self.mail.signatures().await {
        self.take_signatures(generation, signatures);
      }
    }
  }

  /// Tells the signatures after the first inbox answer, and again when a new session's page gave others.
  fn take_signatures(&self, generation: u64, signatures: crate::mail::Signatures) {
    let mut held = self.mail_lock();
    let pair = (signatures.new, signatures.reply);
    if held.generation != generation || held.signatures.as_ref() == Some(&pair) {
      return;
    }
    self.emit(Event::MailSignatures {
      new: pair.0.clone(),
      reply: pair.1.clone(),
    });
    held.signatures = Some(pair);
  }

  /// Takes page `page` of a mailbox into what is held and tells the UI: the list, the unread count when it changed,
  /// and new inbox mail. Events go out under the lock, so two answers are told in the order they were taken.
  fn mail_read(
    &self,
    mailbox: &Mailbox,
    page: u32,
    result: Result<crate::mail::ThreadPage>,
    generation: u64,
    read: MailRead,
    requested: u64,
  ) {
    let mut guard = self.mail_lock();
    let held = &mut *guard;
    if held.generation != generation {
      return;
    }
    let mut fresh = match result {
      Ok(fresh) => fresh,
      Err(error) => return self.mail_failed(held, mailbox, page, read, &error),
    };
    if read == MailRead::Watched {
      held.watch_failed = false;
    }
    held.changes.apply(&mut fresh, *mailbox == Mailbox::Inbox, requested);
    let crate::mail::ThreadPage { threads, more, inbox_unread, labels } = fresh;
    if let Some(labels) = labels {
      self.take_mail_labels(held, labels);
    }
    let list = held.lists.entry(mailbox.clone()).or_default();
    let changed = list.take(page, threads, more, read);
    if read == MailRead::Asked || changed {
      self.emit(Event::MailThreads {
        mailbox: mailbox.clone(),
        threads: Arc::new(list.threads.clone()),
        more: list.more,
      });
    }
    let inbox = *mailbox == Mailbox::Inbox;
    if inbox && changed {
      let saved = MailCache {
        threads: list.threads.iter().take(SAVED_MAIL).cloned().collect(),
        more: list.more || list.threads.len() > SAVED_MAIL,
      };
      let cache = self.cache.clone();
      tokio::task::spawn_blocking(move || cache.save_mail(&saved));
    }
    if let Some(unread) = inbox_unread.filter(|unread| held.unread != Some(*unread)) {
      held.unread = Some(unread);
      self.emit(Event::MailUnread(unread));
    }
    if inbox && page == 0 {
      for thread in held.arrivals.arrived(&list.threads, now_micros()) {
        self.emit(Event::MailIncoming(thread));
      }
    }
  }

  /// Tells the labels when they or a count changed. Under the lock, as `mail_read`.
  fn take_mail_labels(&self, held: &mut MailHeld, labels: Vec<MailLabel>) {
    if held.labels.as_ref() == Some(&labels) {
      return;
    }
    self.emit(Event::MailLabels(Arc::new(labels.clone())));
    held.labels = Some(labels);
  }

  /// Tells of a failed read, once in a row for the watch, which also goes to the log.
  fn mail_failed(&self, held: &mut MailHeld, mailbox: &Mailbox, page: u32, read: MailRead, error: &anyhow::Error) {
    if let Some(list) = held.lists.get_mut(mailbox).filter(|_| page > 0) {
      list.loading_more = false;
    }
    if read == MailRead::Watched {
      tracing::warn!("Gmail: the inbox watch could not read the inbox: {error:#}");
      if std::mem::replace(&mut held.watch_failed, true) {
        return;
      }
    }
    self.emit(Event::MailFailed(format!("{error:#}")));
  }

  async fn open_mail(&self, thread: String) {
    let generation = self.mail_lock().generation;
    let result = self.mail.messages(&thread).await;
    let mut held = self.mail_lock();
    if held.generation != generation {
      return;
    }
    match result {
      Ok(messages) => {
        held.open = Some((thread.clone(), messages.clone()));
        self.emit(Event::MailMessages { thread, messages: Arc::new(messages) });
      },
      Err(error) => self.emit(Event::MailFailed(format!("{error:#}"))),
    }
  }

  /// `MarkMail`, `ArchiveMail`, `TrashMail`, `ReplyMail`, `ComposeMail`: shows the change at once and queues it for
  /// `mail_writer`. A refusal is queued too, so every `MailWritten` comes in the order of its command.
  fn write_mail(self: &Arc<Self>, change: MailChange) {
    let job = self.mail_job(change, None).unwrap_or_else(MailJob::Refused);
    let _ = self.mail_writes.send(job);
  }

  /// The queue entry of `change`, its change shown at once in what is held; the refusal while read-only or with Mail
  /// off, before anything is shown.
  fn mail_job(&self, change: MailChange, answer: Option<MailAnswer>) -> Result<MailJob, &'static str> {
    if self.readonly {
      return Err(MAIL_READONLY);
    }
    let mut guard = self.mail_lock();
    let held = &mut *guard;
    if held.watch.is_none() {
      return Err(MAIL_OFF);
    }
    let undo = self.apply_mail_change(held, &change);
    Ok(MailJob::Write {
      change,
      generation: held.generation,
      undo: Box::new(undo),
      answer,
    })
  }

  /// An agent's write (MCP): the same queue, changes shown, and follow-up reads as the app's commands, answered
  /// to the agent once Gmail took it or not, with a text safe to show. No `MailWritten` goes to the app for it.
  async fn agent_mail_write(&self, change: MailChange) -> Result<(), String> {
    let (answer, answered) = flume::bounded(1);
    let job = self.mail_job(change, Some(answer))?;
    self
      .mail_writes
      .send(job)
      .map_err(|_| "Working is shutting down: nothing was changed.".to_string())?;
    answered
      .recv_async()
      .await
      .unwrap_or_else(|_| Err("Working stopped before Gmail answered.".into()))
  }

  /// An agent's new email. Answers the new thread's id.
  pub(crate) async fn agent_compose(&self, compose: MailCompose) -> Result<String, String> {
    let thread = crate::mail::new_thread_id();
    self
      .agent_mail_write(MailChange::Compose {
        compose: Box::new(compose),
        thread: thread.clone(),
      })
      .await?;
    Ok(thread)
  }

  /// An agent forwards an email: a new email that quotes it, without its files. Answers the new thread's id.
  pub(crate) async fn agent_forward(&self, forward: MailForward) -> Result<String, String> {
    let thread = crate::mail::new_thread_id();
    self
      .agent_mail_write(MailChange::Forward {
        forward: Box::new(forward),
        thread: thread.clone(),
      })
      .await?;
    Ok(thread)
  }

  /// An agent's reply, its recipients already chosen.
  pub(crate) async fn agent_reply(&self, reply: MailReply) -> Result<(), String> {
    self.agent_mail_write(MailChange::Reply(Box::new(reply))).await
  }

  /// An agent archives a thread, or moves it back to the inbox.
  pub(crate) async fn agent_archive(&self, thread: String, archive: bool) -> Result<(), String> {
    self.agent_mail_write(MailChange::Archive { thread, archive }).await
  }

  /// An agent marks a thread read or unread.
  pub(crate) async fn agent_mark(&self, thread: String, read: bool) -> Result<(), String> {
    self.agent_mail_write(MailChange::Mark { thread, read }).await
  }

  /// An agent stars a thread (`^t` on its newest email), or unstars it (off every email).
  pub(crate) async fn agent_star(&self, thread: String, starred: bool) -> Result<(), String> {
    self.agent_mail_write(MailChange::Star { thread, starred }).await
  }

  /// An agent puts the user's labels (ids `^x_…`) on every email of a thread and takes others off. Any other id is
  /// refused before anything is shown or sent.
  pub(crate) async fn agent_label(&self, thread: String, add: Vec<String>, remove: Vec<String>) -> Result<(), String> {
    if let Some(refused) = self.mail_refused() {
      return Err(refused.into());
    }
    crate::mail::check_user_labels(&add, &remove).map_err(|error| format!("{error:#}"))?;
    self.agent_mail_write(MailChange::Label { thread, add, remove }).await
  }

  /// An agent saves a new draft in Gmail (a new email in a thread of its own, or a reply in its thread) and Working
  /// holds it for `agent_send_draft`. Answers the draft's id: its message id (`msg-a:r…`).
  pub(crate) async fn agent_draft(&self, draft: MailDraft) -> Result<String, String> {
    let thread = match &draft {
      MailDraft::New(_) => crate::mail::new_thread_id(),
      MailDraft::Reply(reply) => reply.thread.clone(),
    };
    let id = crate::mail::new_message_id();
    self
      .agent_mail_write(MailChange::Draft {
        draft: Box::new(draft),
        thread,
        id: id.clone(),
      })
      .await?;
    Ok(id)
  }

  /// An agent sends a held draft exactly as saved; Working forgets it once sent. An id not held (sent, or Working
  /// restarted since) is refused with where the draft still is.
  pub(crate) async fn agent_send_draft(&self, id: &str) -> Result<MailDraftSent, String> {
    if let Some(refused) = self.mail_refused() {
      return Err(refused.into());
    }
    let sent = self
      .mail_lock()
      .drafts
      .get(id)
      .map(|draft| MailDraftSent {
        thread: draft.thread.clone(),
        to: draft.to.clone(),
        cc: draft.cc.clone(),
        bcc: draft.bcc.clone(),
      })
      .ok_or_else(|| draft_gone(id))?;
    self
      .agent_mail_write(MailChange::SendDraft {
        id: id.to_string(),
        thread: sent.thread.clone(),
      })
      .await?;
    Ok(sent)
  }

  /// Why a mail write is refused now, in `mail_job`'s order: read-only, then Mail off. For an agent's write that
  /// checks more before it is queued.
  fn mail_refused(&self) -> Option<&'static str> {
    if self.readonly {
      Some(MAIL_READONLY)
    } else if self.mail_on() {
      None
    } else {
      Some(MAIL_OFF)
    }
  }

  /// Saves a new draft and holds what its send needs.
  async fn save_draft(&self, draft: &MailDraft, thread: &str, id: &str) -> Result<()> {
    let saved = self.mail.save_draft(draft, thread, id).await?;
    self.mail_lock().drafts.insert(id.to_string(), saved);
    Ok(())
  }

  /// Sends the held draft `id` and forgets it; one Gmail did not take stays held, to send again.
  async fn send_draft(&self, id: &str) -> Result<crate::mail::Draft> {
    let draft = self
      .mail_lock()
      .drafts
      .get(id)
      .cloned()
      .ok_or_else(|| anyhow!(draft_gone(id)))?;
    self.mail.send_draft(&draft).await?;
    self.mail_lock().drafts.remove(id);
    Ok(draft)
  }

  /// The address mail goes from, as the Gmail page said. Callers check `mail_on` first.
  pub(crate) async fn mail_me(&self) -> Result<Option<MailAddress>> {
    self.mail.me().await
  }

  /// Runs the mail writes one at a time, in the order they came: each writes to Gmail, answers `MailWritten` (or the
  /// agent that asked) and does its follow-up reads before the next starts, so an answer is never taken for another
  /// write's. A change Gmail does not take is put back and shown again.
  async fn mail_writer(self: Arc<Self>, mut jobs: tokio::sync::mpsc::UnboundedReceiver<MailJob>) {
    while let Some(job) = jobs.recv().await {
      match job {
        MailJob::Refused(reason) => self.emit(Event::MailWritten(Err(reason.into()))),
        MailJob::Write { change, generation, undo, answer } => {
          self.run_mail_write(change, generation, *undo, answer).await;
        },
      }
    }
  }

  /// Tells how a write went: to the agent that asked, else to the app.
  fn mail_written(&self, answer: Option<MailAnswer>, result: Result<(), String>) {
    match answer {
      Some(answer) => {
        let _ = answer.send(result);
      },
      None => self.emit(Event::MailWritten(result)),
    }
  }

  async fn run_mail_write(&self, change: MailChange, generation: u64, undo: MailUndo, answer: Option<MailAnswer>) {
    let mut to_me = false;
    let mut sent_reply = false;
    let result = match &change {
      MailChange::Mark { thread, read } => self.mail.mark(thread, *read).await,
      MailChange::Archive { thread, archive } => self.mail.archive(thread, *archive).await,
      MailChange::Trash { thread } => self.mail.trash(thread).await,
      MailChange::Reply(reply) => self.mail.reply(reply).await,
      MailChange::Compose { compose, thread } => self.mail.compose(compose, thread).await.map(|mine| to_me = mine),
      MailChange::Forward { forward, thread } => self.mail.forward(forward, thread).await.map(|mine| to_me = mine),
      MailChange::Star { thread, starred } => self.mail.star(thread, *starred).await,
      MailChange::Label { thread, add, remove } => self.mail.label(thread, add, remove).await,
      MailChange::Draft { draft, thread, id } => self.save_draft(draft, thread, id).await,
      MailChange::SendDraft { id, .. } => self.send_draft(id).await.map(|draft| {
        to_me = draft.to_me;
        sent_reply = draft.is_reply();
      }),
    };
    let thread = match &change {
      MailChange::Mark { thread, .. }
      | MailChange::Archive { thread, .. }
      | MailChange::Trash { thread }
      | MailChange::Compose { thread, .. }
      | MailChange::Forward { thread, .. }
      | MailChange::Star { thread, .. }
      | MailChange::Label { thread, .. }
      | MailChange::Draft { thread, .. }
      | MailChange::SendDraft { thread, .. } => thread.clone(),
      MailChange::Reply(reply) => reply.thread.clone(),
    };
    match result {
      Ok(()) => {
        self.mail_lock().changes.written(&thread);
        self.mail_written(answer, Ok(()));
        match change {
          MailChange::Reply(_) => self.open_mail(thread).await,
          MailChange::SendDraft { .. } if sent_reply => self.open_mail(thread).await,
          // A reply's draft shows in its thread when that thread is open.
          MailChange::Draft { draft, .. } if matches!(*draft, MailDraft::Reply(_)) => {
            let open = self.mail_lock().open.as_ref().is_some_and(|(open, _)| *open == thread);
            if open {
              self.open_mail(thread).await;
            }
          },
          // A thread moved back that the engine did not hold shows once the inbox is read again, now.
          MailChange::Archive { archive: false, .. } if undo.inbox_added.is_none() => {
            self.read_mail_page(&Mailbox::Inbox, 0, generation, MailRead::Watched).await;
          },
          MailChange::Compose { .. } | MailChange::Forward { .. } | MailChange::SendDraft { .. } => {
            self.show_composed(generation, to_me).await;
          },
          MailChange::Star { .. } => self.read_held_lists(generation, vec![Mailbox::Starred]).await,
          // The inbox read again for Gmail's counts, and every search held: one of the Trash, or of every mailbox,
          // shows the thread again.
          MailChange::Trash { .. } => {
            let lists = {
              let held = self.mail_lock();
              let searches = held.lists.keys().filter(|mailbox| matches!(mailbox, Mailbox::Search(_)));
              std::iter::once(Mailbox::Inbox).chain(searches.cloned()).collect()
            };
            self.read_held_lists(generation, lists).await;
          },
          // The label lists held read again, with every label's count; the inbox, for the counts, when none is held.
          MailChange::Label { add, remove, .. } => {
            let lists: Vec<Mailbox> = add.into_iter().chain(remove).map(Mailbox::Label).collect();
            let held = {
              let held = self.mail_lock();
              lists
                .iter()
                .any(|mailbox| held.lists.get(mailbox).is_some_and(|list| list.pages > 0))
            };
            let lists = if held { lists } else { vec![Mailbox::Inbox] };
            self.read_held_lists(generation, lists).await;
          },
          _ => {},
        }
      },
      Err(error) => {
        tracing::warn!("Gmail: a change was not written: {error:#}");
        self.mail_lock().changes.forget(&thread);
        self.undo_mail_change(generation, undo);
        self.mail_written(answer, Err(format!("{error:#}")));
      },
    }
  }

  /// A new email went: the held lists it belongs in are read again, so it shows in them (Sent and All Mail, and the
  /// inbox when the user is a recipient). Lists never asked for stay unread.
  async fn show_composed(&self, generation: u64, to_me: bool) {
    let lists = [Mailbox::Sent, Mailbox::All, Mailbox::Inbox]
      .into_iter()
      .filter(|mailbox| to_me || *mailbox != Mailbox::Inbox)
      .collect();
    self.read_held_lists(generation, lists).await;
  }

  /// Reads again the first page of each of `lists` the engine holds (asked for since Mail started), told when it
  /// changed. Lists never asked for stay unread.
  async fn read_held_lists(&self, generation: u64, lists: Vec<Mailbox>) {
    let held: Vec<Mailbox> = {
      let held = self.mail_lock();
      lists
        .into_iter()
        .filter(|mailbox| held.lists.get(mailbox).is_some_and(|list| list.pages > 0))
        .collect()
    };
    for mailbox in held {
      self.read_mail_page(&mailbox, 0, generation, MailRead::Watched).await;
    }
  }

  /// Shows `change` in what is held and answers how to put it back. Under the mail lock.
  fn apply_mail_change(&self, held: &mut MailHeld, change: &MailChange) -> MailUndo {
    let mut undo = MailUndo {
      unread: held.unread,
      labels: held.labels.clone(),
      ..MailUndo::default()
    };
    match change {
      MailChange::Mark { thread, read } => {
        held.changes.note(thread, Some(!read), None);
        self.mark_held(held, &mut undo, thread, *read);
      },
      MailChange::Archive { thread, archive } => {
        held.changes.note(thread, None, Some(!archive));
        self.archive_held(held, &mut undo, thread, *archive);
      },
      MailChange::Trash { thread } => {
        held.changes.note_trashed(thread);
        self.trash_held(held, &mut undo, thread);
      },
      MailChange::Star { thread, starred } => {
        held.changes.note_starred(thread, *starred);
        self.star_held(held, &mut undo, thread, *starred);
      },
      MailChange::Label { thread, add, remove } => self.label_held(held, &mut undo, thread, add, remove),
      // A reply, a new email, a forward, or a draft shows once Gmail has it; its thread must not come back as new
      // mail.
      MailChange::Reply(reply) => held.arrivals.note_own(&reply.thread, own_until()),
      MailChange::Compose { thread, .. }
      | MailChange::Forward { thread, .. }
      | MailChange::Draft { thread, .. }
      | MailChange::SendDraft { thread, .. } => held.arrivals.note_own(thread, own_until()),
    }
    undo
  }

  /// Shows a thread starred or not in every held list that has it, and puts it in or takes it out of the Starred list
  /// when that is held.
  fn star_held(&self, held: &mut MailHeld, undo: &mut MailUndo, thread_id: &str, starred: bool) {
    let known = held
      .lists
      .values()
      .flat_map(|list| &list.threads)
      .find(|thread| thread.id == thread_id)
      .cloned();
    for (mailbox, list) in &mut held.lists {
      if *mailbox == Mailbox::Starred {
        continue;
      }
      let Some(thread) = list
        .threads
        .iter_mut()
        .find(|thread| thread.id == thread_id && thread.starred != starred)
      else {
        continue;
      };
      undo.threads.push((mailbox.clone(), thread.clone()));
      thread.starred = starred;
      self.emit_mail_list(mailbox, list);
    }
    let Some(list) = held.lists.get_mut(&Mailbox::Starred) else {
      return;
    };
    let place = list.threads.iter().position(|thread| thread.id == thread_id);
    match (starred, place, known) {
      (false, Some(place), _) => {
        let thread = list.threads.remove(place);
        undo.removed.push((Mailbox::Starred, place, thread));
      },
      (true, None, Some(mut thread)) => {
        thread.starred = true;
        if !insert_by_time(list, thread) {
          return;
        }
        undo.added.push((Mailbox::Starred, thread_id.to_string()));
      },
      _ => return,
    }
    self.emit_mail_list(&Mailbox::Starred, list);
  }

  /// Shows a thread in the held lists of the labels put on it and out of those of the labels taken off. A label's
  /// unread count moves only where the held list says the thread was on it (taken off), or that it was not (put on,
  /// a list held whole); the read after the write brings the rest.
  fn label_held(&self, held: &mut MailHeld, undo: &mut MailUndo, thread_id: &str, add: &[String], remove: &[String]) {
    let known = held
      .lists
      .values()
      .flat_map(|list| &list.threads)
      .find(|thread| thread.id == thread_id)
      .cloned();
    let changes = add.iter().map(|id| (id, true)).chain(remove.iter().map(|id| (id, false)));
    for (id, put) in changes {
      let mailbox = Mailbox::Label(id.clone());
      let Some(list) = held.lists.get_mut(&mailbox) else {
        continue;
      };
      let place = list.threads.iter().position(|thread| thread.id == thread_id);
      let shift = match (put, place, &known) {
        (false, Some(place), _) => {
          let thread = list.threads.remove(place);
          let unread = thread.unread;
          undo.removed.push((mailbox.clone(), place, thread));
          unread
        },
        (true, None, Some(thread)) => {
          let whole = !list.more;
          if !insert_by_time(list, thread.clone()) {
            continue;
          }
          undo.added.push((mailbox.clone(), thread_id.to_string()));
          whole && thread.unread
        },
        _ => continue,
      };
      self.emit_mail_list(&mailbox, list);
      if shift {
        shift_label(&mut held.labels, id, put);
      }
    }
    if held.labels != undo.labels
      && let Some(labels) = &held.labels
    {
      self.emit(Event::MailLabels(Arc::new(labels.clone())));
    }
  }

  fn mark_held(&self, held: &mut MailHeld, undo: &mut MailUndo, thread_id: &str, read: bool) {
    let unread = !read;
    let mut inbox_changed = false;
    for (mailbox, list) in &mut held.lists {
      let Some(thread) = list
        .threads
        .iter_mut()
        .find(|thread| thread.id == thread_id && thread.unread != unread)
      else {
        continue;
      };
      undo.threads.push((mailbox.clone(), thread.clone()));
      thread.unread = unread;
      match mailbox {
        Mailbox::Inbox => inbox_changed = true,
        Mailbox::Label(id) => shift_label(&mut held.labels, id, unread),
        _ => {},
      }
      self.emit_mail_list(mailbox, list);
    }
    if inbox_changed {
      self.shift_unread(held, unread);
    }
    if held.labels != undo.labels
      && let Some(labels) = &held.labels
    {
      self.emit(Event::MailLabels(Arc::new(labels.clone())));
    }
    if let Some((open, messages)) = held.open.as_mut().filter(|(open, _)| open == thread_id) {
      undo.open = Some((open.clone(), messages.clone()));
      if read {
        for message in messages.iter_mut() {
          message.unread = false;
        }
      } else if let Some(newest) = messages.last_mut() {
        newest.unread = true;
      }
      self.emit(Event::MailMessages {
        thread: open.clone(),
        messages: Arc::new(messages.clone()),
      });
    }
  }

  fn archive_held(&self, held: &mut MailHeld, undo: &mut MailUndo, thread_id: &str, archive: bool) {
    let known = held
      .lists
      .values()
      .flat_map(|list| &list.threads)
      .find(|thread| thread.id == thread_id)
      .cloned();
    let kept = held.archived.remove(thread_id);
    let Some(inbox) = held.lists.get_mut(&Mailbox::Inbox) else {
      return;
    };
    let place = inbox.threads.iter().position(|thread| thread.id == thread_id);
    let unread = match (archive, place, kept.or(known)) {
      (true, Some(place), _) => {
        let thread = inbox.threads.remove(place);
        let unread = thread.unread;
        held.archived.insert(thread_id.to_string(), thread.clone());
        undo.inbox_removed = Some((place, thread));
        unread
      },
      (false, None, Some(thread)) => {
        let newer = inbox.threads.iter().take_while(|other| other.time >= thread.time).count();
        let unread = thread.unread;
        inbox.threads.insert(newer, thread);
        undo.inbox_added = Some(thread_id.to_string());
        unread
      },
      _ => return,
    };
    self.emit_mail_list(&Mailbox::Inbox, inbox);
    if unread {
      self.shift_unread(held, !archive);
    }
  }

  /// Takes a trashed thread out of every held list, the inbox's unread count and the labels' with it.
  fn trash_held(&self, held: &mut MailHeld, undo: &mut MailUndo, thread_id: &str) {
    held.archived.remove(thread_id);
    let mut inbox_unread = false;
    for (mailbox, list) in &mut held.lists {
      let Some(place) = list.threads.iter().position(|thread| thread.id == thread_id) else {
        continue;
      };
      let thread = list.threads.remove(place);
      if thread.unread {
        match mailbox {
          Mailbox::Inbox => inbox_unread = true,
          Mailbox::Label(id) => shift_label(&mut held.labels, id, false),
          _ => {},
        }
      }
      undo.removed.push((mailbox.clone(), place, thread));
      self.emit_mail_list(mailbox, list);
    }
    if inbox_unread {
      self.shift_unread(held, false);
    }
    if held.labels != undo.labels
      && let Some(labels) = &held.labels
    {
      self.emit(Event::MailLabels(Arc::new(labels.clone())));
    }
  }

  /// One more or one fewer unread inbox thread, told.
  fn shift_unread(&self, held: &mut MailHeld, more: bool) {
    if let Some(count) = held.unread.as_mut() {
      *count = if more {
        count.saturating_add(1)
      } else {
        count.saturating_sub(1)
      };
      self.emit(Event::MailUnread(*count));
    }
  }

  fn emit_mail_list(&self, mailbox: &Mailbox, list: &MailList) {
    self.emit(Event::MailThreads {
      mailbox: mailbox.clone(),
      threads: Arc::new(list.threads.clone()),
      more: list.more,
    });
  }

  /// Puts back what a change Gmail did not take altered, and tells it again.
  fn undo_mail_change(&self, generation: u64, undo: MailUndo) {
    let mut guard = self.mail_lock();
    let held = &mut *guard;
    if held.generation != generation {
      return;
    }
    let mut touched: Vec<Mailbox> = Vec::new();
    for (mailbox, old) in undo.threads {
      if let Some(thread) = held
        .lists
        .get_mut(&mailbox)
        .and_then(|list| list.threads.iter_mut().find(|thread| thread.id == old.id))
      {
        *thread = old;
        touched.push(mailbox);
      }
    }
    if let Some(inbox) = held.lists.get_mut(&Mailbox::Inbox) {
      let before = inbox.threads.len();
      if let Some((place, thread)) = undo.inbox_removed
        && !inbox.threads.iter().any(|other| other.id == thread.id)
      {
        inbox.threads.insert(place.min(inbox.threads.len()), thread);
      }
      if let Some(id) = undo.inbox_added {
        inbox.threads.retain(|thread| thread.id != id);
      }
      if inbox.threads.len() != before && !touched.contains(&Mailbox::Inbox) {
        touched.push(Mailbox::Inbox);
      }
    }
    for (mailbox, place, thread) in undo.removed {
      if let Some(list) = held.lists.get_mut(&mailbox)
        && !list.threads.iter().any(|other| other.id == thread.id)
      {
        list.threads.insert(place.min(list.threads.len()), thread);
        if !touched.contains(&mailbox) {
          touched.push(mailbox);
        }
      }
    }
    for (mailbox, id) in undo.added {
      if let Some(list) = held.lists.get_mut(&mailbox) {
        list.threads.retain(|thread| thread.id != id);
        if !touched.contains(&mailbox) {
          touched.push(mailbox);
        }
      }
    }
    for mailbox in touched {
      if let Some(list) = held.lists.get(&mailbox) {
        self.emit_mail_list(&mailbox, list);
      }
    }
    if let Some((thread, messages)) = undo.open
      && held.open.as_ref().is_some_and(|(open, _)| *open == thread)
    {
      self.emit(Event::MailMessages {
        thread: thread.clone(),
        messages: Arc::new(messages.clone()),
      });
      held.open = Some((thread, messages));
    }
    if held.unread != undo.unread {
      held.unread = undo.unread;
      if let Some(count) = held.unread {
        self.emit(Event::MailUnread(count));
      }
    }
    if held.labels != undo.labels {
      held.labels = undo.labels;
      if let Some(labels) = &held.labels {
        self.emit(Event::MailLabels(Arc::new(labels.clone())));
      }
    }
  }

  /// `Command::StopMail`: the watch stops and the mail held is forgotten, but for the drafts an agent saved (still in
  /// Gmail, to send later); the disk cache stays.
  fn stop_mail(&self) {
    let mut held = self.mail_lock();
    if let Some(watch) = held.watch.take() {
      watch.abort();
    }
    let drafts = std::mem::take(&mut held.drafts);
    *held = MailHeld {
      generation: held.generation.wrapping_add(1),
      drafts,
      ..MailHeld::default()
    };
  }

  fn stop_mail_watch(&self) {
    let watch = self.mail_lock().watch.take();
    if let Some(watch) = watch {
      watch.abort();
    }
  }

  /// Mail is on in the app: an inbox `LoadMail` started the watch and no `StopMail` came since. Agents read Gmail
  /// only then.
  pub(crate) fn mail_on(&self) -> bool {
    self.mail_lock().watch.is_some()
  }

  /// Page `page` of a mailbox read now, for agents, leaving what the app holds alone. Callers check `mail_on`
  /// first.
  pub(crate) async fn mail_threads(&self, mailbox: &Mailbox, page: u32) -> Result<crate::mail::ThreadPage> {
    self.mail.threads(mailbox, page).await
  }

  /// A thread's emails read now, for agents. Leaves them unread. Callers check `mail_on` first.
  pub(crate) async fn mail_messages(&self, thread: &str) -> Result<Vec<crate::model::MailMessage>> {
    self.mail.messages(thread).await
  }

  /// An email's file (`MailAttachment::url`) downloaded now with Gmail's cookies, for agents, kept out of the image
  /// cache. Callers check `mail_on` first.
  pub(crate) async fn mail_file(&self, url: &str) -> Result<Vec<u8>> {
    self.mail.fetch(url).await
  }

  /// An email as it was sent (`mail::Client::original`), downloaded now, for agents. Callers check `mail_on` first.
  pub(crate) async fn mail_original(&self, message: &str) -> Result<Vec<u8>> {
    self.mail.original(message).await
  }

  /// The user's labels for agents: as last told, or read now from the inbox's first page when none were. Callers
  /// check `mail_on` first.
  pub(crate) async fn mail_labels(&self) -> Result<Vec<MailLabel>> {
    let held = self.mail_lock().labels.clone();
    if let Some(labels) = held {
      return Ok(labels);
    }
    Ok(self.mail.threads(&Mailbox::Inbox, 0).await?.labels.unwrap_or_default())
  }
}

/// Google Calendar and Meet for agents: reads straight from Google, writes that leave the app's calendar range and the
/// reminder watch up to date.
impl Engine {
  /// `WORKING_READONLY=1`: agents' changes are refused.
  pub(crate) const fn readonly(&self) -> bool {
    self.readonly
  }

  /// The user's calendars, as `Client::calendars` orders them.
  pub(crate) async fn calendar_list(&self) -> Result<Vec<crate::model::Calendar>> {
    self.calendar.calendars().await
  }

  /// Every calendar's events that overlap `start..end`, by start.
  pub(crate) async fn calendar_events(&self, start: Micros, end: Micros) -> Result<Vec<crate::model::CalendarEvent>> {
    self.calendar.events(start, end).await
  }

  /// One event by its calendar and id.
  pub(crate) async fn calendar_event(&self, calendar_id: &str, event_id: &str) -> Result<crate::model::CalendarEvent> {
    self.calendar.event(calendar_id, event_id).await
  }

  /// Creates or changes an event for an agent.
  pub(crate) async fn calendar_save(&self, draft: &EventDraft) -> Result<()> {
    let result = self.calendar.save(draft).await;
    self.calendar_changed(result.is_ok()).await;
    result
  }

  /// Deletes an event (or its series with `EditScope::All`) for an agent.
  pub(crate) async fn calendar_delete(
    &self,
    calendar_id: &str,
    event_id: &str,
    scope: EditScope,
    notify: bool,
  ) -> Result<()> {
    let result = self.calendar.delete(calendar_id, event_id, scope, notify).await;
    self.calendar_changed(result.is_ok()).await;
    result
  }

  /// Answers an invitation for an agent.
  pub(crate) async fn calendar_respond(
    &self,
    calendar_id: &str,
    event_id: &str,
    scope: EditScope,
    rsvp: Rsvp,
  ) -> Result<()> {
    let result = self.calendar.respond(calendar_id, event_id, scope, rsvp).await;
    self.calendar_changed(result.is_ok()).await;
    result
  }

  /// After an agent's change: the reminder watch and the range the app shows read the events again.
  async fn calendar_changed(&self, changed: bool) {
    if !changed {
      return;
    }
    self.events_written.notify_one();
    let range = *self.calendar_range.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((start, end)) = range {
      self.load_events(start, end).await;
    }
  }

  /// A new Meet meeting of its own for an agent, as `Command::CreateMeeting` makes one: its link, or a short reason.
  pub(crate) async fn create_meeting_link(&self) -> Result<String, String> {
    self.request_meeting().await.map_err(|error| self.meeting_failure(&error))
  }
}

fn proto_message_id(group: &GroupId, topic_id: &str, message_id: &str) -> proto::MessageId {
  proto::MessageId {
    parent_id: Some(proto::MessageParentId {
      topic_id: Some(proto::TopicId {
        group_id: Some(convert::proto_group_id(group)),
        topic_id: Some(topic_id.to_string()),
      }),
    }),
    message_id: Some(message_id.to_string()),
  }
}

/// The link of the meeting a video call answer holds.
fn meeting_link(call: &proto::CreateVideoCallResponse) -> Option<String> {
  let annotation = call.annotation.as_ref()?;
  let space = annotation.video_call_metadata.as_ref()?.meeting_space.as_ref()?;
  space.meeting_url.clone().filter(|url| !url.is_empty())
}

/// How often the server is told the user is still there (or still gone).
const HEARTBEAT: Duration = Duration::from_secs(120);

/// How often the cookies Google refreshed go into the session file.
const SESSION_SAVE: Duration = Duration::from_secs(120);

/// The time zone of this computer, as Google Chat names zones.
pub fn device_time_zone() -> String {
  iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into())
}

// Presence and do-not-disturb schedules.
impl Engine {
  /// Keeps the session file at what the session is now: Google refreshes cookies while the chat runs.
  async fn session_loop(self: Arc<Self>) {
    loop {
      tokio::time::sleep(SESSION_SAVE).await;
      self.http.save_refreshed();
    }
  }

  async fn presence_loop(self: Arc<Self>) {
    let mut refused: u32 = 0;
    loop {
      tokio::select! {
        () = tokio::time::sleep(backoff(HEARTBEAT, refused)) => {},
        () = self.session_restored.notified() => refused = 0,
      }
      let presence = *self.presence.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      if let Some(presence) = presence {
        refused = if self.heartbeat(presence).await {
          refused.saturating_add(1)
        } else {
          0
        };
      }
    }
  }

  /// Tells the server whether the user is there. True when the session was refused.
  async fn heartbeat(&self, presence: Presence) -> bool {
    use proto::heartbeat_request::presence_update_request::UserState;
    let state = if presence == Presence::Active {
      UserState::Active
    } else {
      UserState::Inactive
    };
    let request = proto::HeartbeatRequest {
      request_header: Some(request_header()),
      presence_update_request: Some(proto::heartbeat_request::PresenceUpdateRequest {
        user_state: Some(state as i32),
        ..Default::default()
      }),
    };
    // A lost heartbeat is made up for by the next one.
    self
      .http
      .api::<_, proto::HeartbeatResponse>("heartbeat", &request)
      .await
      .err()
      .is_some_and(|error| is_signed_out(&error))
  }

  /// The schedules sit behind two of the web app's page RPCs. Both speak the same shape:
  /// `[_, match time zone, time zone, [[_, days, start minute, end minute, enabled], ...]]`.
  async fn load_dnd(&self) -> Result<()> {
    let answer = self.http.page_rpc("bqAG7c", &serde_json::json!([device_time_zone()])).await?;
    let settings = &answer[0];
    let flag = |value: &serde_json::Value| value.as_bool().unwrap_or(value.as_i64() == Some(1));
    let schedules = settings[3]
      .as_array()
      .map_or_default(Vec::as_slice)
      .iter()
      .map(|schedule| DndSchedule {
        days: schedule[1]
          .as_array()
          .into_iter()
          .flatten()
          .filter_map(serde_json::Value::as_u64)
          .map(|day| day as u8)
          .collect(),
        start: schedule[2].as_u64().unwrap_or(0) as u16,
        end: schedule[3].as_u64().unwrap_or(0) as u16,
        enabled: flag(&schedule[4]),
      });
    self.emit(Event::Dnd(DndSchedules {
      match_time_zone: flag(&settings[1]),
      time_zone: settings[2].as_str().map_or_else(device_time_zone, str::to_string),
      schedules: schedules.collect(),
    }));
    Ok(())
  }

  async fn save_dnd(&self, mut schedules: DndSchedules) -> Result<()> {
    if schedules.match_time_zone || schedules.time_zone.is_empty() {
      schedules.time_zone = device_time_zone();
    }
    let rows: Vec<serde_json::Value> = schedules
      .schedules
      .iter()
      .map(|schedule| {
        serde_json::json!([null, schedule.days, schedule.start, schedule.end, u8::from(schedule.enabled)])
      })
      .collect();
    let arguments = serde_json::json!([[null, u8::from(schedules.match_time_zone), schedules.time_zone, rows]]);
    self.http.page_rpc("cnDGIf", &arguments).await?;
    self.emit(Event::Dnd(schedules));
    Ok(())
  }
}

/// How often other people's status is read again.
const STATUSES_EVERY: Duration = Duration::from_secs(60);

impl Engine {
  /// Keeps the status of the people the user talks to one-to-one current.
  async fn statuses_loop(self: Arc<Self>) {
    let mut mine_at: Option<tokio::time::Instant> = None;
    let mut refused: u32 = 0;
    loop {
      // The stream reports changes to the user's own status; this read catches what it missed.
      let mut signed_out = if mine_at.is_none_or(|at| at.elapsed() >= STATUSES_EVERY) {
        mine_at = Some(tokio::time::Instant::now());
        self.refresh_mine().await
      } else {
        false
      };
      let people: Vec<String> = {
        let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let direct = state
          .groups
          .values()
          .filter(|group| group.kind == GroupKind::Dm && !group.hidden && !group.blocked);
        direct
          .flat_map(|group| group.member_ids.iter().cloned())
          .collect::<HashSet<_>>()
          .into_iter()
          .collect()
      };
      if !signed_out && !people.is_empty() {
        signed_out = self.refresh_statuses(&people).await;
      }
      refused = if signed_out {
        refused.saturating_add(1)
      } else {
        0
      };
      // Until the conversations name somebody to ask about, a rebuilt list wakes the loop early.
      tokio::select! {
        () = tokio::time::sleep(backoff(STATUSES_EVERY, refused)) => {},
        () = self.session_restored.notified() => refused = 0,
        () = self.groups_changed.notified(), if people.is_empty() => {},
      }
    }
  }

  /// Reads the status of `people`. True when the session was refused.
  async fn refresh_statuses(&self, people: &[String]) -> bool {
    let request = proto::GetUserPresenceRequest {
      request_header: Some(request_header()),
      user_ids: people
        .iter()
        .map(|id| proto::UserId {
          id: Some(id.clone()),
          ..Default::default()
        })
        .collect(),
      include_active_until: Some(true),
      include_user_status: Some(true),
    };
    // A failed read leaves the last known statuses up until the next one.
    match self
      .http
      .api::<_, proto::GetUserPresenceResponse>("get_user_presence", &request)
      .await
    {
      Ok(answer) => {
        let statuses = answer.user_presences.iter().filter_map(|person| {
          let id = person.user_id.as_ref()?.id.clone()?;
          let status = if person.dnd_state == Some(proto::DndStateState::Dnd as i32) {
            PersonStatus::DoNotDisturb
          } else if person.presence == Some(proto::Presence::Active as i32) {
            PersonStatus::Active
          } else {
            PersonStatus::Away
          };
          Some((id, status))
        });
        self.emit(Event::Statuses(statuses.collect()));
        false
      },
      Err(error) => is_signed_out(&error),
    }
  }

  /// Reads the user's own status. True when the session was refused.
  async fn refresh_mine(&self) -> bool {
    let request = proto::GetSelfUserStatusRequest {
      request_header: Some(request_header()),
      ..Default::default()
    };
    match self
      .http
      .api::<_, proto::GetSelfUserStatusResponse>("get_self_user_status", &request)
      .await
    {
      Ok(answer) => {
        if let Some(status) = answer.user_status {
          self.emit(Event::Mine(mine_from(&status)));
        }
        false
      },
      Err(error) => is_signed_out(&error),
    }
  }

  async fn set_dnd(&self, choice: DndFor) -> Result<()> {
    // The web client writes this through a page RPC, not the protobuf endpoint.
    let usec = match choice {
      DndFor::Off => 0,
      DndFor::For(usec) => usec,
    };
    self.http.page_rpc("zEAHrd", &serde_json::json!([usec])).await?;
    self.refresh_mine().await;
    Ok(())
  }

  async fn set_status(&self, emoji: String, text: String, until: Option<i64>) -> Result<()> {
    let clearing = emoji.is_empty() && text.is_empty();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_micros() as i64);
    let remaining = until.map_or(7 * 24 * 3600 * 1_000_000, |at| (at - now).max(1_000_000));
    let arguments = if clearing {
      serde_json::json!([null, null, "0"])
    } else {
      serde_json::json!([null, [text, null, null, [emoji]], remaining.to_string()])
    };
    self.http.page_rpc("f8a8Te", &arguments).await?;
    self.refresh_mine().await;
    Ok(())
  }

  async fn create_chat(self: &Arc<Self>, user_ids: Vec<String>) -> Result<()> {
    let me = self.me_id();
    let mut ids: Vec<String> = user_ids.into_iter().filter(|id| id != &me).collect();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
      bail!("pick at least one person");
    }
    if let Some(group) = self.existing_chat(&ids) {
      self.emit(Event::OpenChat(group));
      return Ok(());
    }
    // Nothing holds the chat back from the screen while Google creates it.
    let starting = {
      let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      created_group(GroupId::starting(), ids.clone(), &me, &state.users)
    };
    self.emit(Event::ChatStarting(starting));
    let invitees: Vec<proto::InviteeInfo> = ids
      .iter()
      .map(|id| proto::InviteeInfo {
        user_id: Some(proto::UserId {
          id: Some(id.clone()),
          ..Default::default()
        }),
        ..Default::default()
      })
      .collect();
    let local_id = Some(format!("working%{}", rand::random::<u64>()));
    // `create_dm` makes one-to-one chats only. A chat with several people is an unnamed space, whose name the server
    // writes from its members.
    let created = if invitees.len() == 1 {
      let request = proto::CreateDmRequest {
        request_header: Some(request_header()),
        members: invitees.iter().filter_map(|invitee| invitee.user_id.clone()).collect(),
        invitees,
        retention_settings: Some(proto::RetentionSettings {
          state: Some(proto::retention_settings::RetentionState::Permanent as i32),
          ..Default::default()
        }),
        local_id,
        force_empty_dm_visible: Some(true),
        ..Default::default()
      };
      let answer: proto::CreateDmResponse = self.http.api("create_dm", &request).await?;
      answer.dm
    } else {
      let request = proto::CreateGroupRequest {
        request_header: Some(request_header()),
        space: Some(proto::SpaceCreationInfo {
          flat_group: Some(proto::space_creation_info::FlatGroup {}),
          has_server_generated_name: Some(true),
          invitee_member_infos: invitees
            .into_iter()
            .map(|invitee_info| proto::InviteeMemberInfo { invitee_info: Some(invitee_info) })
            .collect(),
          ..Default::default()
        }),
        local_id,
        should_find_existing_space: Some(true),
      };
      let answer: proto::CreateGroupResponse = self.http.api("create_group", &request).await?;
      answer.group
    };
    let group_id = created
      .as_ref()
      .and_then(|group| group.group_id.as_ref())
      .and_then(convert::group_id);
    let Some(group_id) = group_id else {
      bail!("Google Chat did not return the new conversation")
    };
    // The people picker offers known people, so this lookup, which would hold the new chat back for another round
    // trip, is only for ids that came from somewhere else.
    let unknown: Vec<String> = {
      let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      ids.iter().filter(|id| state.users.get(*id).is_none()).cloned().collect()
    };
    if !unknown.is_empty() {
      self.fetch_users(unknown).await.ok();
    }
    let group = {
      let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      created_group(group_id.clone(), ids, &me, &state.users)
    };
    {
      let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      state.groups.insert(group.id.clone(), group.clone());
    }
    self.emit(Event::GroupUpsert(group));
    self.emit(Event::OpenChat(group_id));
    self.queue_world_refresh();
    Ok(())
  }

  fn existing_chat(&self, ids: &[String]) -> Option<GroupId> {
    let want: HashSet<&String> = ids.iter().collect();
    let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    state.groups.values().find_map(|group| {
      let members: HashSet<&String> = group.member_ids.iter().collect();
      let kind_ok = if ids.len() == 1 {
        group.kind == GroupKind::Dm
      } else {
        group.kind == GroupKind::GroupDm
      };
      (kind_ok && members == want).then(|| group.id.clone())
    })
  }
}

fn mine_from(status: &proto::UserStatus) -> MyStatus {
  let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_micros() as i64);
  let settings = status.dnd_settings.as_ref();
  let dnd_on =
    settings.is_some_and(|settings| settings.dnd_state == Some(proto::dnd_settings::DndStateState::Dnd as i32));
  let expiry = settings
    .and_then(|settings| settings.dnd_expiry_time_usec)
    .filter(|until| *until > 0);
  let dnd = dnd_on && expiry.is_none_or(|until| until > now);
  let custom = status.custom_status.as_ref();
  let text = custom.and_then(|custom| custom.status_text.clone()).unwrap_or_default();
  let emoji = custom
    .and_then(|custom| {
      custom
        .status_emoji
        .clone()
        .filter(|emoji| !emoji.is_empty())
        .or_else(|| custom.emoji.as_ref().and_then(|emoji| emoji.unicode.clone()))
    })
    .unwrap_or_default();
  let status_until = custom
    .and_then(|custom| custom.state_expiry_timestamp_usec)
    .filter(|until| *until > now);
  let clear = text.is_empty() && emoji.is_empty()
    || status_until.is_none()
      && custom
        .and_then(|custom| custom.state_expiry_timestamp_usec)
        .is_some_and(|until| until <= now);
  MyStatus {
    dnd,
    dnd_until: dnd.then_some(expiry).flatten(),
    emoji: if clear { String::new() } else { emoji },
    text: if clear { String::new() } else { text },
    status_until: if clear { None } else { status_until },
  }
}

/// Whether an error is the session being refused.
fn is_signed_out(error: &anyhow::Error) -> bool {
  error.downcast_ref::<SignedOut>().is_some()
}

/// How long a polling loop waits after `refused` refusals of the session in a row: twice as long each time, up to 16
/// times `every`.
fn backoff(every: Duration, refused: u32) -> Duration {
  every.saturating_mul(1 << refused.min(4))
}

fn people_list<'a>(users: impl IntoIterator<Item = &'a User>, me: &str) -> Vec<User> {
  let mut people: Vec<User> = users
    .into_iter()
    .filter(|user| !user.is_bot && !user.deleted && user.id != me && !user.name.is_empty())
    .cloned()
    .collect();
  people.sort_by_cached_key(|user| user.name.to_lowercase());
  people
}

fn created_group(id: GroupId, member_ids: Vec<String>, me: &str, users: &convert::Users) -> Group {
  let _ = me;
  let members: Vec<&User> = member_ids.iter().filter_map(|id| users.get(id)).collect();
  let kind = if member_ids.len() <= 1 {
    GroupKind::Dm
  } else {
    GroupKind::GroupDm
  };
  let name = if members.len() == 1 {
    members[0].name.clone()
  } else {
    members
      .iter()
      .map(|user| {
        if user.first_name.is_empty() {
          user.name.as_str()
        } else {
          user.first_name.as_str()
        }
      })
      .collect::<Vec<_>>()
      .join(", ")
  };
  let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_micros() as i64);
  Group {
    id,
    kind,
    name,
    avatar_url: members.first().and_then(|user| user.avatar_url.clone()),
    emoji: None,
    member_ids,
    member_avatars: members.iter().filter_map(|user| user.avatar_url.clone()).take(2).collect(),
    sort_time: now,
    last_read_time: now,
    unread: false,
    mentioned: false,
    pinned: false,
    muted: false,
    notify: Notify::All,
    snippet: None,
    placement: Placement::ByKind,
    hidden: false,
    blocked: false,
    can_delete: false,
    // Every direct message and group chat seen live has in-line threading on.
    threads: true,
  }
}

/// The request header of the web app's JSON API calls, with the client capabilities it declares.
const WEB_REQUEST_HEADER: &str = r#"[0,3,1,"en",[null,null,null,null,2,2,null,2,2,2,2,null,null,null,null,2,2,2,2,2,2,2,2,2,2,2,2,2,2,null,null,2,2,null,null,2,2,2,null,null,null,null,2,2,2,2,null,2,null,null,2,null,2,2,2,2,null,2,null,2,2,2,2,2,2,2,2,2,2,2,null,2,2,null,2,2,null,2]]"#;

/// The web app's world section of starred messages: up to 100 entries filtered by the starred label.
const STARRED_SECTION: &str = r#"[100,null,null,null,null,null,null,null,null,[[1,1],[1]],[4],null,null,null,null,[[[3,[null,null,null,null,[2,[null,null,null,null,null,null,null,"label"],[null,null,null,null,null,null,null,"100G"]]]]]]]"#;

/// The label that puts a conversation in a custom section.
fn section_label(id: &str) -> proto::GroupLabelId {
  proto::GroupLabelId {
    group_label_type: Some(proto::group_label_id::GroupLabelType::CustomSection as i32),
    label_secondary_key: Some(id.to_string()),
  }
}

/// A conversation as the web app's page RPCs name it: its path, its id, and its kind (2 a space, 5 a DM).
fn page_group_id(group: &GroupId) -> serde_json::Value {
  match group {
    GroupId::Space(id) => serde_json::json!([format!("space/{id}"), id, 2]),
    GroupId::Dm(id) => serde_json::json!([format!("dm/{id}"), id, 5]),
  }
}

/// A message's id on the wire: the message inside its topic inside its conversation.
fn wire_message_id(group: &GroupId, topic_id: String, message_id: String) -> proto::MessageId {
  proto::MessageId {
    parent_id: Some(proto::MessageParentId {
      topic_id: Some(proto::TopicId {
        group_id: Some(convert::proto_group_id(group)),
        topic_id: Some(topic_id),
      }),
    }),
    message_id: Some(message_id),
  }
}

/// A reaction's emoji on the wire, as the model holds it: Unicode characters.
fn wire_emoji(emoji: &str) -> proto::Emoji {
  proto::Emoji {
    unicode: Some(emoji.to_string()),
    ..Default::default()
  }
}

/// `apply_message_label` / `remove_message_label` for the star, in the web app's JSON: the message id (topic and
/// conversation inside), the label, and the request header as field 100.
fn star_request(group: &GroupId, topic_id: &str, message_id: &str) -> Result<serde_json::Value> {
  let conversation = match group {
    GroupId::Dm(id) => serde_json::json!([null, null, [id]]),
    GroupId::Space(id) => serde_json::json!([[id]]),
  };
  let mut request = vec![serde_json::Value::Null; 100];
  request[0] = serde_json::json!([[null, null, null, [null, topic_id, conversation]], message_id]);
  request[1] = serde_json::json!([convert::STARRED_LABEL]);
  request[99] = serde_json::from_str(WEB_REQUEST_HEADER)?;
  Ok(serde_json::Value::Array(request))
}

/// `click_card`, in the web app's JSON, as captured from it: the message id (topic and conversation inside), the
/// button's form action as the card carried it, and the card id. No request header.
fn click_card_request(
  group: &GroupId,
  topic_id: &str,
  message_id: &str,
  card_id: &str,
  action: &CardAction,
) -> serde_json::Value {
  let conversation = match group {
    GroupId::Dm(id) => serde_json::json!([null, null, [id]]),
    GroupId::Space(id) => serde_json::json!([[id]]),
  };
  let parameters: Vec<serde_json::Value> = action
    .parameters
    .iter()
    .map(|(key, value)| serde_json::json!([key, value]))
    .collect();
  serde_json::json!([
    [[null, null, null, [null, topic_id, conversation]], message_id],
    [
      action.method,
      null,
      null,
      parameters,
      action.load_indicator,
      i32::from(action.persist_values),
      null,
      action.interaction,
      null,
      i32::from(action.all_widgets_are_required)
    ],
    null,
    card_id
  ])
}

/// A starred message of the world's starred section, as a hit. Its fields are positional: the id (topic and
/// conversation inside), the sender, the creation time, and the text.
fn starred_hit(message: &serde_json::Value, users: &Users) -> Option<SearchHit> {
  let topic = &message[0][0][3];
  let conversation = &topic[2];
  let group = if let Some(space) = conversation[0][0].as_str() {
    GroupId::Space(space.to_string())
  } else {
    GroupId::Dm(conversation[2][0].as_str()?.to_string())
  };
  let sender_id = message[1][0][0].as_str().unwrap_or_default().to_string();
  let sender = users.get(&sender_id);
  Some(SearchHit {
    group,
    message_id: message[0][1].as_str()?.to_string(),
    topic_id: topic[1].as_str().unwrap_or_default().to_string(),
    sender_name: sender.map_or_default(|user| user.name.clone()),
    sender_avatar: sender.and_then(|user| user.avatar_url.clone()),
    sender_id,
    text: message[9].as_str().unwrap_or_default().to_string(),
    time: message[2].as_str().and_then(|time| time.parse().ok())?,
  })
}

/// Page RPC `BFhA9d`'s arguments: block a space (`person` `None`), or block or unblock the other person of a DM.
fn block_arguments(group: &GroupId, person: Option<&str>, blocked: bool) -> serde_json::Value {
  let triple = page_group_id(group);
  let Some(user) = person else {
    return serde_json::json!([null, triple, 1, 0]);
  };
  let person = serde_json::json!([user, format!("human/{user}"), 0]);
  if blocked {
    serde_json::json!([person, null, 1, 0, null, triple])
  } else {
    serde_json::json!([person, null, 0, null, null, triple])
  }
}

/// Page RPC `U3Vwwb`'s arguments: delete a space.
fn delete_arguments(group: &GroupId) -> serde_json::Value {
  serde_json::json!([page_group_id(group)])
}

#[cfg(test)]
mod tests {
  use super::{
    MailList, Receipts, block_arguments, click_card_request, delete_arguments, file_name, meeting_link, page_group_id,
    people_list, starred_hit,
  };
  use crate::convert::Users;
  use crate::model::GroupId;
  use crate::model::{CardAction, User};
  use crate::proto;
  use std::path::Path;

  /// Every `LoadMail` reads a page or is answered at once with what is held, a second `more` while one is on its way
  /// too (it used to get no answer at all).
  #[test]
  fn every_load_of_a_list_reads_a_page_or_answers_what_is_held() {
    let mut list = MailList::default();
    assert_eq!(list.next_read(true), Some(0), "nothing held: the newest page");
    list.pages = 1;
    list.more = true;
    assert_eq!(list.next_read(false), Some(0));
    assert_eq!(list.next_read(true), Some(1));
    assert!(list.loading_more);
    assert_eq!(
      list.next_read(true),
      None,
      "the next page is on its way: answered with what is held"
    );
    list.loading_more = false;
    list.more = false;
    assert_eq!(list.next_read(true), None, "nothing older: answered with what is held");
  }

  /// The id triple the web app's pin and hide page RPCs carry, captured with the ids replaced.
  #[test]
  fn page_group_id_matches_the_web_app() {
    assert_eq!(
      page_group_id(&GroupId::Space("AAAA1".into())),
      serde_json::json!(["space/AAAA1", "AAAA1", 2])
    );
    assert_eq!(
      page_group_id(&GroupId::Dm("DM1".into())),
      serde_json::json!(["dm/DM1", "DM1", 5])
    );
  }

  /// The block, unblock, and delete arguments the web app sent, captured with the ids replaced.
  #[test]
  fn block_and_delete_arguments_match_the_web_app() {
    let space = GroupId::Space("AAAA1".into());
    let dm = GroupId::Dm("DM1".into());
    assert_eq!(
      block_arguments(&space, None, true),
      serde_json::json!([null, ["space/AAAA1", "AAAA1", 2], 1, 0])
    );
    assert_eq!(
      block_arguments(&dm, Some("111"), true),
      serde_json::json!([["111", "human/111", 0], null, 1, 0, null, ["dm/DM1", "DM1", 5]])
    );
    assert_eq!(
      block_arguments(&dm, Some("111"), false),
      serde_json::json!([["111", "human/111", 0], null, 0, null, null, ["dm/DM1", "DM1", 5]])
    );
    assert_eq!(delete_arguments(&space), serde_json::json!([["space/AAAA1", "AAAA1", 2]]));
  }

  /// The body the web app sent for a card button, captured with its ids replaced.
  #[test]
  fn click_card_request_matches_the_web_app() {
    let action = CardAction {
      method: "https://app.example/google-chat/events".into(),
      parameters: vec![("action".into(), "accept".into()), ("post_id".into(), "p1".into())],
      ..CardAction::default()
    };
    let request = click_card_request(&GroupId::Dm("DM1".into()), "T1", "T1", "card-1", &action);
    let expected = serde_json::json!([
      [[null, null, null, [null, "T1", [null, null, ["DM1"]]]], "T1"],
      [
        "https://app.example/google-chat/events",
        null,
        null,
        [["action", "accept"], ["post_id", "p1"]],
        0,
        0,
        null,
        0,
        null,
        0
      ],
      null,
      "card-1"
    ]);
    assert_eq!(request, expected);
  }

  fn user(id: &str, name: &str) -> User {
    User {
      id: id.into(),
      name: name.into(),
      first_name: String::new(),
      email: String::new(),
      avatar_url: None,
      is_bot: false,
      deleted: false,
    }
  }

  #[test]
  fn file_name_uses_the_last_component() {
    assert_eq!(file_name(Path::new("/tmp/photo.png")), "photo.png");
    assert_eq!(file_name(Path::new("photo.png")), "photo.png");
  }

  #[test]
  fn people_list_drops_self_bots_and_empty_names() {
    let mut bot = user("b", "Bot");
    bot.is_bot = true;
    let mut gone = user("g", "Gone");
    gone.deleted = true;
    let people = people_list(&[user("me", "Me"), user("a", "Ada"), bot, gone, user("z", "")], "me");
    assert_eq!(people.iter().map(|user| user.id.as_str()).collect::<Vec<_>>(), vec!["a"]);
  }

  #[test]
  fn starred_entries_resolve_dm_and_space_messages() {
    let mut users = Users::new();
    users.insert("u1".into(), user("u1", "Ana"));
    let dm = serde_json::json!([
      [[null, null, null, [null, "t1", [null, null, ["d1"]]]], "m1"],
      [["u1"]],
      "1700000000000000",
      null,
      null,
      null,
      null,
      null,
      null,
      "hello"
    ]);
    let hit = starred_hit(&dm, &users).expect("a DM message");
    assert_eq!(hit.group, GroupId::Dm("d1".into()));
    assert_eq!((hit.message_id.as_str(), hit.topic_id.as_str()), ("m1", "t1"));
    assert_eq!(
      (hit.sender_name.as_str(), hit.text.as_str(), hit.time),
      ("Ana", "hello", 1_700_000_000_000_000)
    );

    let space = serde_json::json!([[[null, null, null, [null, "t2", [["s1"]]]], "m2"], [["u9"]], "5"]);
    let hit = starred_hit(&space, &users).expect("a space message");
    assert_eq!(hit.group, GroupId::Space("s1".into()));
    assert_eq!(hit.sender_name, "");

    assert!(starred_hit(&serde_json::Value::Null, &users).is_none());
  }

  #[test]
  fn a_meeting_link_comes_from_the_video_call_answer() {
    let answer = |url: Option<&str>| proto::CreateVideoCallResponse {
      annotation: Some(proto::Annotation {
        video_call_metadata: Some(proto::VideoCallMetadata {
          meeting_space: Some(proto::MeetingSpace {
            meeting_code: Some("abc-defg-hij".into()),
            meeting_url: url.map(str::to_string),
            ..Default::default()
          }),
          ..Default::default()
        }),
        ..Default::default()
      }),
    };
    assert_eq!(
      meeting_link(&answer(Some("https://meet.google.com/abc-defg-hij"))).as_deref(),
      Some("https://meet.google.com/abc-defg-hij")
    );
    assert_eq!(meeting_link(&answer(None)), None, "a space without a URL has no link");
    assert_eq!(meeting_link(&answer(Some(""))), None);
    assert_eq!(
      meeting_link(&proto::CreateVideoCallResponse::default()),
      None,
      "no meeting was made"
    );
  }

  /// The shape a live call answered, with made-up values: the annotation is field 1, its video call field 12, the
  /// meeting space field 1 of that, and the meeting URL field 3 of the space.
  #[test]
  fn a_meeting_link_comes_from_the_wire_answer() {
    use prost::Message as _;
    // A length-delimited field: its tag byte, its length (every payload here is short), then the payload.
    let field = |tag: u8, payload: &[u8]| {
      let mut bytes = vec![tag, u8::try_from(payload.len()).expect("a short payload")];
      bytes.extend_from_slice(payload);
      bytes
    };
    let space = [
      field(0x0a, b"spaces/abc"),
      field(0x12, b"abc-defg-hij"),
      field(0x1a, b"https://meet.google.com/abc-defg-hij"),
    ]
    .concat();
    // The annotation: type 11 (a video call), then the video call metadata that holds the space.
    let annotation = [vec![0x08, 0x0b], field(0x62, &field(0x0a, &space))].concat();
    let wire = field(0x0a, &annotation);
    let answer = proto::CreateVideoCallResponse::decode(wire.as_slice()).expect("the answer decodes");
    assert_eq!(meeting_link(&answer).as_deref(), Some("https://meet.google.com/abc-defg-hij"));
  }

  /// A set of receipts as the wire has it, with made-up people.
  fn readers(enabled: Option<bool>, people: &[(&str, i64)]) -> proto::ReadReceiptSet {
    proto::ReadReceiptSet {
      enabled,
      read_receipts: people
        .iter()
        .map(|(id, time)| proto::ReadReceipt {
          read_time_micros: Some(*time),
          user: Some(proto::User {
            user_id: Some(proto::UserId {
              id: Some((*id).into()),
              ..Default::default()
            }),
            ..Default::default()
          }),
        })
        .collect(),
    }
  }

  /// Who has read up to when, in the order published.
  fn who(receipts: &Receipts) -> Vec<(&str, i64)> {
    receipts
      .people
      .iter()
      .map(|person| (person.user_id.as_str(), person.read_time))
      .collect()
  }

  #[test]
  fn receipts_are_published_when_they_change_and_a_stream_event_adds_to_them() {
    let users = Users::new();
    let publish =
      |held: Option<&Receipts>, set: &proto::ReadReceiptSet, merge| Receipts::updated(held, set, "me", &users, merge);
    // The first answer of a load is published, and the same answer again is not.
    let whole = readers(Some(true), &[("jordan", 100), ("riley", 200)]);
    let loaded = publish(None, &whole, false).expect("the first answer is new");
    assert!(loaded.enabled);
    assert_eq!(who(&loaded), [("jordan", 100), ("riley", 200)]);
    assert!(publish(Some(&loaded), &whole, false).is_none());
    // An event that names only who just read is added; repeating it, or an older time, changes nothing.
    let read_on = readers(Some(true), &[("jordan", 300)]);
    let moved = publish(Some(&loaded), &read_on, true).expect("Jordan read on");
    assert_eq!(who(&moved), [("riley", 200), ("jordan", 300)]);
    assert!(publish(Some(&moved), &read_on, true).is_none());
    assert!(publish(Some(&moved), &readers(Some(true), &[("jordan", 150)]), true).is_none());
    // The signed-in user reading is no one's receipt.
    assert!(publish(Some(&moved), &readers(Some(true), &[("me", 400)]), true).is_none());
    // A later load answers the whole set again, so whoever is not in it goes.
    let reloaded = publish(Some(&moved), &readers(Some(true), &[("sam", 50)]), false).expect("a new set");
    assert_eq!(who(&reloaded), [("sam", 50)]);
    // Receipts turned off empty the set, once; a conversation that never had any says so as well.
    let off = readers(Some(false), &[("sam", 50)]);
    let cleared = publish(Some(&reloaded), &off, true).expect("receipts went off");
    assert!(!cleared.enabled && cleared.people.is_empty());
    assert!(publish(Some(&cleared), &off, false).is_none());
    assert!(publish(None, &off, false).is_some_and(|first| !first.enabled));
  }

  fn posted(id: &str, topic: &str, time: i64) -> std::sync::Arc<crate::model::Message> {
    std::sync::Arc::new(crate::model::Message {
      id: id.into(),
      topic_id: topic.into(),
      reply: id != topic,
      group: GroupId::Space("AAAA".into()),
      sender_id: "u1".into(),
      sender_name: "Jordan Park".into(),
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
  }

  #[test]
  fn the_disk_cache_keeps_the_newest_topics_and_loads_the_rest_again() {
    // Topic 0 has a reply; 41 more topics follow, then a message still being sent.
    let mut messages = vec![posted("m0", "t0", 100), posted("r0", "t0", 150)];
    messages.extend((1..=41).map(|n| posted(&format!("m{n}"), &format!("t{n}"), 100 + n * 10)));
    let mut sending = (*posted("local", "", 999)).clone();
    sending.pending = true;
    messages.push(std::sync::Arc::new(sending));
    let history = super::History {
      messages,
      reached_start: true,
      anchor: 100,
      topic_times: std::collections::HashMap::from([("t2".to_string(), 125)]),
      ..Default::default()
    };
    let saved = history.saved();
    let ids: Vec<&str> = saved.messages.iter().map(|message| message.id.as_str()).collect();
    assert_eq!(ids.len(), 40);
    assert_eq!(ids.first(), Some(&"m2"));
    assert_eq!(ids.last(), Some(&"m41"));
    assert!(!saved.reached_start);
    assert_eq!(saved.anchor, 125);

    // A history that fits keeps where it starts; a reply stays with its topic.
    let short = super::History {
      messages: vec![posted("m0", "t0", 100), posted("r0", "t0", 150), posted("m1", "t1", 200)],
      reached_start: true,
      anchor: 90,
      ..Default::default()
    };
    let saved = short.saved();
    assert_eq!(saved.messages.len(), 3);
    assert!(saved.reached_start);
    assert_eq!(saved.anchor, 90);
    let topics = short.topics();
    assert_eq!(topics.len(), 2);
    assert_eq!(topics.first().map(|(time, messages)| (*time, messages.len())), Some((90, 2)));
    assert_eq!(topics.last().map(|(time, _)| *time), Some(200));
  }

  /// `posted` with an @mention of `who` (or @all for `None`).
  fn mentioning(id: &str, time: i64, who: Option<&str>) -> std::sync::Arc<crate::model::Message> {
    let mut message = (*posted(id, id, time)).clone();
    message.spans.push(crate::model::Span {
      range: 0..0,
      kind: who.map_or(crate::model::SpanKind::MentionAll, |who| {
        crate::model::SpanKind::Mention(who.into())
      }),
    });
    std::sync::Arc::new(message)
  }

  fn unread_group(read: i64) -> crate::model::Group {
    let mut group = super::created_group(GroupId::Dm("DM1".into()), Vec::new(), "me", &Users::default());
    group.unread = true;
    group.last_read_time = read;
    group
  }

  #[test]
  fn the_mention_mark_holds_only_while_unread() {
    let mut group = unread_group(100);
    super::settle_mention(&mut group, true, None, "me");
    assert!(group.mentioned, "a mention the stream told stays while unread");
    group.unread = false;
    super::settle_mention(&mut group, true, None, "me");
    assert!(!group.mentioned, "read clears it");
    let mut group = unread_group(100);
    super::settle_mention(&mut group, false, None, "me");
    assert!(!group.mentioned, "nothing known, nothing held: plain unread");
  }

  #[test]
  fn the_mention_mark_comes_from_the_held_history_past_the_read_cursor() {
    let loaded = |messages| super::History {
      messages,
      reached_start: true,
      fetched: true,
      ..Default::default()
    };
    let history = loaded(vec![posted("m1", "m1", 100), mentioning("m2", 200, Some("me"))]);
    let mut group = unread_group(150);
    super::settle_mention(&mut group, false, Some(&history), "me");
    assert!(group.mentioned, "an unread mention held");
    let mut group = unread_group(250);
    super::settle_mention(&mut group, true, Some(&history), "me");
    assert!(
      !group.mentioned,
      "the mention was read: the whole history overrides what was known"
    );

    let others = loaded(vec![mentioning("m1", 200, Some("rbrooks")), posted("m2", "m2", 300)]);
    let mut group = unread_group(150);
    super::settle_mention(&mut group, false, Some(&others), "me");
    assert!(!group.mentioned, "someone else mentioned");
    let mut own = (*mentioning("m1", 200, None)).clone();
    own.sender_id = "me".into();
    let mine = loaded(vec![std::sync::Arc::new(own)]);
    super::settle_mention(&mut group, false, Some(&mine), "me");
    assert!(!group.mentioned, "the user's own @all");
    let everyone = loaded(vec![mentioning("m1", 200, None)]);
    super::settle_mention(&mut group, false, Some(&everyone), "me");
    assert!(group.mentioned, "@all from someone else");

    // A history that starts after the read cursor, or one not loaded, cannot rule a known mention out.
    let mut partial = loaded(vec![posted("m3", "m3", 400)]);
    partial.reached_start = false;
    partial.anchor = 400;
    let mut group = unread_group(150);
    super::settle_mention(&mut group, true, Some(&partial), "me");
    assert!(group.mentioned, "a later message without a mention keeps the earlier one");
    let stale = super::History {
      messages: vec![posted("m3", "m3", 400)],
      reached_start: true,
      ..Default::default()
    };
    super::settle_mention(&mut group, true, Some(&stale), "me");
    assert!(group.mentioned, "a history the stream may have missed messages of");
  }

  #[test]
  fn a_mentions_search_hit_marks_only_an_unread_conversation_past_its_read_cursor() {
    let hit = |sender: &str, time| crate::model::SearchHit {
      group: GroupId::Dm("DM1".into()),
      message_id: "m1".into(),
      topic_id: "m1".into(),
      sender_id: sender.into(),
      sender_name: "Jordan Park".into(),
      sender_avatar: None,
      text: String::new(),
      time,
    };
    let seeds = |sender, time, group| super::seeds_mention(&hit(sender, time), group, "me");
    let unread = unread_group(150);
    assert!(seeds("jpark", 200, Some(&unread)), "after the read time");
    assert!(!seeds("jpark", 150, Some(&unread)), "at the read time");
    assert!(!seeds("jpark", 100, Some(&unread)), "before the read time");
    assert!(!seeds("me", 200, Some(&unread)), "the user's own message");
    let mut read = unread_group(150);
    read.unread = false;
    assert!(!seeds("jpark", 200, Some(&read)), "a read conversation");
    assert!(!seeds("jpark", 200, None), "a conversation not listed");
  }

  fn listed_group(id: &str, time: i64, unread: bool) -> crate::model::Group {
    let mut group = super::created_group(GroupId::Space(id.into()), Vec::new(), "me", &Users::default());
    group.sort_time = time;
    group.unread = unread;
    group
  }

  fn spaces(ids: &[&str]) -> Vec<GroupId> {
    ids.iter().map(|id| GroupId::Space((*id).into())).collect()
  }

  #[test]
  fn prefetch_takes_unread_conversations_before_recent_ones() {
    let mut muted = listed_group("muted", 30, true);
    muted.muted = true;
    let mut hidden = listed_group("hidden", 90, true);
    hidden.hidden = true;
    let mut blocked = listed_group("blocked", 80, true);
    blocked.blocked = true;
    let groups = vec![
      listed_group("read new", 100, false),
      listed_group("unread old", 10, true),
      muted,
      listed_group("unread new", 50, true),
      hidden,
      blocked,
      listed_group("read old", 5, false),
    ];
    assert_eq!(
      super::prefetch_order(&groups, |_| false),
      spaces(&["unread new", "muted", "unread old", "read new", "read old"]),
      "unread first, newest first within each, hidden and blocked left out"
    );
    let loaded = spaces(&["muted", "read new"]);
    assert_eq!(
      super::prefetch_order(&groups, |id| loaded.contains(id)),
      spaces(&["unread new", "unread old", "read old"]),
      "loaded ones left out"
    );
  }

  #[test]
  fn prefetch_takes_the_most_recent_and_stops_at_the_cap() {
    let mut groups: Vec<_> = (0..20_i64)
      .map(|n| listed_group(&format!("read {n:02}"), 100 + n, false))
      .collect();
    groups.push(listed_group("unread oldest", 1, true));
    let order = super::prefetch_order(&groups, |_| false);
    assert_eq!(order.len(), 1 + super::PREFETCH_RECENT);
    assert_eq!(
      order.first(),
      Some(&GroupId::Space("unread oldest".into())),
      "an unread one outside the recent"
    );
    assert_eq!(order.get(1), Some(&GroupId::Space("read 19".into())));
    assert_eq!(
      order.last(),
      Some(&GroupId::Space("read 04".into())),
      "only the most recent read ones"
    );

    let unread: Vec<_> = (0..40_i64).map(|n| listed_group(&format!("unread {n:02}"), n, true)).collect();
    let order = super::prefetch_order(&unread, |_| false);
    assert_eq!(order.len(), super::PREFETCH_MAX);
    assert_eq!(order.first(), Some(&GroupId::Space("unread 39".into())));
    assert_eq!(
      order.last(),
      Some(&GroupId::Space("unread 16".into())),
      "the newest unread ones"
    );
  }
}
