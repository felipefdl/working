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
use tokio::sync::Semaphore;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::cache::{Cache, HistoryCache, WorldCache};
use crate::channel::{Channel, ChannelEvent};
use crate::convert::{self, Users};
use crate::http::{Http, SignedOut, request_header};
use crate::mcp;
use crate::model::{
  Attachment, AttachmentKind, CardAction, Chip, Command, Connection, DndFor, DndSchedule, DndSchedules, EditScope,
  Event, EventDraft, Group, GroupId, GroupKind, McpConfig, McpStatus, Message, Micros, MyStatus, Notify, PersonStatus,
  Placement, Presence, Quote, Reaction, Rsvp, SearchHit, Shortcut, Snippet, User,
};
use crate::proto;

/// Topics per history page, as the web client asks.
const LATEST_PAGE: i32 = 40;
const OLDER_PAGE: i32 = 30;
/// Conversations whose history is fetched right after start, so the first click on them is instant.
const PREFETCH_GROUPS: usize = 16;
const XSRF_MAX_AGE_SECS: u64 = 6 * 3600;

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

/// Starts the core thread and returns a handle for image fetches.
pub fn spawn(commands: flume::Receiver<Command>, events: flume::Sender<Event>) -> Arc<Fetcher> {
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
      runtime.block_on(run(commands, events, fetches));
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
  path
    .file_name()
    .map(|name| name.to_string_lossy().into_owned())
    .unwrap_or_default()
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
  users: Users,
  items: HashMap<GroupId, proto::WorldItemLite>,
  groups: HashMap<GroupId, Group>,
  histories: HashMap<GroupId, History>,
  open: Option<GroupId>,
  connection: Option<Connection>,
  world_refresh_queued: bool,
  /// Names of the user's own sidebar sections, by section id.
  sections: HashMap<String, String>,
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

async fn wait_for_session(
  commands: &flume::Receiver<Command>,
  fetches: &flume::Receiver<(String, FetchReply)>,
  cache: &Cache,
  events: &flume::Sender<Event>,
) -> Option<(Http, PendingStart)> {
  let mut pending = PendingStart::default();
  loop {
    tokio::select! {
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
) {
  let cache = Cache::open();
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
      match wait_for_session(&commands, &fetches, &cache, &events).await {
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
    users: world.users.into_iter().map(|user| (user.id.clone(), user)).collect(),
    sections: world.sections,
    groups: world.groups.into_iter().map(|group| (group.id.clone(), group)).collect(),
    ..Default::default()
  };
  let engine = Arc::new(Engine {
    http,
    channel: Channel::new(),
    cache,
    events,
    state: Mutex::new(state),
    readonly: std::env::var("WORKING_READONLY").is_ok_and(|value| value == "1"),
    mcp: tokio::sync::Mutex::new(None),
    presence: Mutex::new(None),
    calendar: crate::calendar::Client::new(crate::config_dir().join("cookies.json")),
    calendar_range: Mutex::new(None),
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
  tokio::spawn({
    let engine = engine.clone();
    async move {
      let result = engine.sync_world().await;
      engine.report(result);
      engine.prefetch_histories().await;
    }
  });

  if pending.has_mcp {
    engine.handle(Command::SetMcp(pending.mcp));
  }
  if let Some(presence) = pending.presence {
    engine.handle(Command::SetPresence(presence));
  }

  while let Ok(command) = commands.recv_async().await {
    engine.handle(command);
  }
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
        match engine.http.get_bytes(&url).await {
          Ok(bytes) => {
            let bytes = Arc::new(bytes);
            let cache = engine.cache.clone();
            let stored = bytes.clone();
            tokio::task::spawn_blocking(move || cache.save_image(&url, &stored));
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
    self
      .state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .me
      .as_ref()
      .map(|me| me.id.clone())
      .unwrap_or_default()
  }

  // ---- Conversation list ----

  async fn sync_world(self: &Arc<Self>) -> Result<()> {
    let mut me_id = self.me_id();
    if me_id.is_empty() {
      let request = proto::GetSelfUserStatusRequest {
        request_header: Some(request_header()),
        ..Default::default()
      };
      let status: proto::GetSelfUserStatusResponse = self.http.api("get_self_user_status", &request).await?;
      me_id = status
        .user_status
        .and_then(|status| status.user_id)
        .and_then(|id| id.id)
        .unwrap_or_default();
    }

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
    let response: proto::PaginatedWorldResponse = self.http.api("paginated_world", &request).await?;
    let items: Vec<proto::WorldItemLite> = response.world_items.into_iter().filter(convert::is_listed).collect();

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
      let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      needed.retain(|id| !id.is_empty() && !state.users.contains_key(id));
    }

    {
      let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      state.items = items
        .into_iter()
        .filter_map(|item| Some((convert::group_id(item.group_id.as_ref()?)?, item)))
        .collect();
    }
    // Paint with the names already known, then again once the missing people are resolved.
    self.rebuild_groups(&me_id);
    if !needed.is_empty() {
      self.fetch_users(needed.into_iter().collect()).await?;
      self.rebuild_groups(&me_id);
    }
    self.save_world();
    // Section names are not in the list answer; they are read from the web app's page when one is unknown.
    let unnamed = {
      let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      state.groups.values().any(|group| match &group.placement {
        Placement::Custom { id, .. } => !state.sections.contains_key(id),
        _ => false,
      })
    };
    if unnamed {
      let page = self.http.get_bytes("https://chat.google.com/u/0/app/home").await?;
      let names = convert::section_names(&String::from_utf8_lossy(&page));
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
    Ok(())
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
          let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
          for user in response
            .members
            .iter()
            .filter_map(|member| member.user.as_ref())
            .filter_map(convert::user)
          {
            state.users.insert(user.id.clone(), user);
          }
        },
        Err(error) => first_error = first_error.or(Some(error)),
      }
    }
    first_error.map_or(Ok(()), Err)
  }

  /// Converts the raw conversation list with the users known now and publishes it.
  fn rebuild_groups(&self, me_id: &str) {
    let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
      .map(|group| (group.id.clone(), group))
      .collect();
    state.groups = groups;
    self.emit(Event::World(Arc::new(state.groups.values().cloned().collect())));
    let me = me_id.to_string();
    let users: Vec<User> = state.users.values().cloned().collect();
    drop(state);
    self.emit(Event::People(people_list(&users, &me)));
  }

  fn save_world(&self) {
    let world = {
      let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
      WorldCache {
        me: state.me.clone(),
        users: state.users.values().cloned().collect(),
        groups: state.groups.values().cloned().collect(),
        xsrf: self.http.xsrf().map(|token| (token, now)),
        sections: state.sections.clone(),
      }
    };
    let cache = self.cache.clone();
    tokio::task::spawn_blocking(move || cache.save_world(&world));
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
      let result = engine.sync_world().await;
      engine.report(result);
    });
  }

  // ---- History ----

  async fn prefetch_histories(self: &Arc<Self>) {
    let mut recent: Vec<(Micros, GroupId)> = {
      let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      state.groups.values().map(|group| (group.sort_time, group.id.clone())).collect()
    };
    recent.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    let slots = Arc::new(Semaphore::new(3));
    for (_, group) in recent.into_iter().take(PREFETCH_GROUPS) {
      let engine = self.clone();
      let slots = slots.clone();
      tokio::spawn(async move {
        let _slot = slots.acquire().await;
        if !engine
          .state
          .lock()
          .unwrap_or_else(std::sync::PoisonError::into_inner)
          .histories
          .get(&group)
          .is_some_and(|history| history.fresh)
        {
          // Prefetch is opportunistic: the conversation reports the failure itself when it is opened.
          if let Err(error) = engine.refresh_history(group).await {
            tracing::warn!("history prefetch failed: {error:#}");
          }
        }
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

  fn save_history(&self, group: &GroupId, history: &History) {
    // A window into the past is not what the next start should show.
    if history.detached {
      return;
    }
    let snapshot = HistoryCache {
      messages: history.messages.iter().filter(|message| !message.pending).cloned().collect(),
      reached_start: history.reached_start,
      anchor: history.anchor,
    };
    let cache = self.cache.clone();
    let group = group.clone();
    tokio::task::spawn_blocking(move || cache.save_history(&group, &snapshot));
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

  /// Flattens topics into messages, oldest topic first, replies right after their first message.
  fn convert_topics(&self, response: &proto::ListTopicsResponse) -> (Vec<Arc<Message>>, Micros) {
    let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut topics: Vec<&proto::Topic> = response.topics.iter().collect();
    topics.sort_by_key(|topic| topic.sort_time.unwrap_or(0));
    let anchor = topics.first().and_then(|topic| topic.sort_time).unwrap_or(0);
    let messages = topics
      .iter()
      .flat_map(|topic| &topic.replies)
      .filter_map(|message| convert::message(message, &state.users))
      .map(Arc::new)
      .collect();
    (messages, anchor)
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
    let (fetched, anchor) = self.convert_topics(&response);
    let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let online = state.connection == Some(Connection::Online);
    let history = state.histories.entry(group.clone()).or_default();
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
    }
    history.messages = messages;
    history.fresh = online;
    let history = &state.histories[&group];
    self.emit_history(&group, history, 0);
    self.save_history(&group, history);
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
    let (older, new_anchor) = converted.unwrap_or_default();
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
    Ok(())
  }

  /// Replaces the conversation's messages with the window around one message.
  async fn jump(&self, group: GroupId, message_id: String, time: Micros) -> Result<()> {
    let (around, after) = tokio::join!(
      self.list_topics(&group, Page::At(time)),
      self.list_topics(&group, Page::After(time))
    );
    let (around, after) = (around?, after?);
    let (mut messages, anchor) = self.convert_topics(&around);
    let (newer, _) = self.convert_topics(&after);
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
    let history = History {
      messages,
      reached_start: around.contains_first_topic.unwrap_or(false),
      anchor,
      fresh: complete && state.connection == Some(Connection::Online),
      detached: !complete,
      newest,
      ..Default::default()
    };
    self.emit_history_focused(&group, &history, 0, Some(message_id));
    state.histories.insert(group, history);
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
    let (newer, _) = converted.unwrap_or_default();
    let held: HashSet<String> = history.messages.iter().map(|message| message.id.clone()).collect();
    history
      .messages
      .extend(newer.into_iter().filter(|message| !held.contains(&message.id)));
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
          sender_name: sender.map(|user| user.name.clone()).unwrap_or_default(),
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

  /// One page of a shortcut's messages.
  async fn load_shortcut(&self, shortcut: Shortcut, cursor: Option<String>) -> Result<()> {
    let appended = cursor.is_some();
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
    self.emit(Event::Shortcut {
      shortcut,
      hits: Arc::new(hits),
      appended,
      cursor,
    });
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
    let held = self
      .state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .histories
      .contains_key(&group);
    if !held {
      let cache = self.cache.clone();
      let key = group.clone();
      let cached = tokio::task::spawn_blocking(move || cache.history(&key)).await?;
      let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      if let Some(cached) = cached
        && !state.histories.contains_key(&group)
      {
        let history = History {
          messages: cached.messages,
          reached_start: cached.reached_start,
          anchor: cached.anchor,
          ..Default::default()
        };
        self.emit_history(&group, &history, 0);
        state.histories.insert(group.clone(), history);
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
          if let Some(history) = state.histories.get(&group) {
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
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).open = Some(group.clone());
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
              engine.emit(Event::Connection(Connection::Connecting));
              let result = engine.sync_world().await;
              engine.report(result);
              engine.prefetch_histories().await;
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
          let result = engine.sync_world().await;
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
        let Some(pending) = self.add_pending(&group, topic_id.as_deref(), &text, &chips, shown, attached) else {
          return;
        };
        tokio::spawn(async move {
          let result = async {
            let uploads = engine.upload_files(&group, files).await?;
            engine
              .send_message(&group, topic_id.as_deref(), &text, &chips, quoted.as_deref(), uploads, &pending)
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
        // Shown at once; the server's answer brings the formatting, a failure puts the old text back.
        let mut edited = (*original).clone();
        edited.text = text.clone();
        edited.spans = Vec::new();
        edited.edited = true;
        self.upsert_message(Arc::new(edited), None);
        tokio::spawn(async move {
          let request = proto::EditMessageRequest {
            request_header: Some(request_header()),
            id: Some(proto_message_id(&group, &original.topic_id, &original.id)),
            text_body: Some(text),
            message_info: Some(proto::MessageInfo {
              accept_format_annotations: Some(true),
              ..Default::default()
            }),
            ..Default::default()
          };
          let result = engine.http.api::<_, proto::EditMessageResponse>("edit_message", &request).await;
          let answered = result.as_ref().ok().and_then(|response| response.message.as_ref());
          let converted = answered.and_then(|message| {
            convert::message(
              message,
              &engine.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).users,
            )
          });
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
        let Some(pending) = self.add_pending(&group, None, "Starting a video meeting…", &[], None, Vec::new()) else {
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
        tokio::spawn(async move {
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
            message_id: Some(proto::MessageId {
              parent_id: Some(proto::MessageParentId {
                topic_id: Some(proto::TopicId {
                  group_id: Some(convert::proto_group_id(&group)),
                  topic_id: Some(topic_id),
                }),
              }),
              message_id: Some(message_id.clone()),
            }),
            emoji: Some(proto::Emoji {
              unicode: Some(emoji.clone()),
              ..Default::default()
            }),
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
      Command::CreateChat { user_ids } => {
        if self.readonly {
          return self.emit(Event::Error("Read-only mode: the chat was not created.".into()));
        }
        tokio::spawn(async move {
          let result = engine.create_chat(user_ids).await;
          engine.report(result);
        });
      },
    }
  }

  /// Changes one conversation and publishes it. False when it is unknown or nothing changed.
  fn update_group(&self, id: &GroupId, change: impl FnOnce(&mut Group)) -> bool {
    let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(group) = state.groups.get_mut(id) else {
      return false;
    };
    let before = group.clone();
    change(group);
    if *group == before {
      return false;
    }
    self.emit(Event::GroupUpsert(group.clone()));
    true
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

  /// Shows the message at once, before the server knows it. Returns its local id.
  fn add_pending(
    &self,
    group: &GroupId,
    topic_id: Option<&str>,
    text: &str,
    chips: &[Chip],
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
      group: group.clone(),
      sender_id: me.id.clone(),
      sender_name: me.name.clone(),
      sender_avatar: me.avatar_url,
      text: text.to_string(),
      spans: convert::chip_spans(chips),
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
    text: &str,
    chips: &[Chip],
    quoted: Option<&Message>,
    uploads: Vec<proto::UploadMetadata>,
    local_id: &str,
  ) -> Result<()> {
    let mut annotations = convert::chip_annotations(text, chips);
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
          text_body: Some(text.to_string()),
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
          text_body: Some(text.to_string()),
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
    let converted = sent.and_then(|message| {
      convert::message(
        &message,
        &self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).users,
      )
    });
    let message = converted.ok_or_else(|| anyhow!("the server accepted the message but did not return it"))?;
    self.upsert_message(Arc::new(message), Some(local_id));
    Ok(())
  }

  /// Joined members of a conversation, people and apps, by name, without the signed-in user. Members whose
  /// profile the answer leaves out are looked up.
  async fn load_members(self: &Arc<Self>, group: GroupId) -> Result<()> {
    let mut ids = Vec::new();
    let mut page_token = None;
    loop {
      let request = proto::ListMembersRequest {
        request_header: Some(request_header()),
        group_id: Some(convert::proto_group_id(&group)),
        page_size: Some(500),
        page_token: page_token.take(),
        ..Default::default()
      };
      let response: proto::ListMembersResponse = self.http.api("list_members", &request).await?;
      {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for user in response
          .members
          .iter()
          .filter_map(|member| member.user.as_ref())
          .filter_map(convert::user)
        {
          state.users.insert(user.id.clone(), user);
        }
      }
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
    self.emit(Event::Members { group, users: Arc::new(users) });
    Ok(())
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
    let converted = sent.and_then(|message| {
      convert::message(
        &message,
        &self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).users,
      )
    });
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
    let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let me_id = state.me.as_ref().map(|me| me.id.clone()).unwrap_or_default();
    let open = state.open.as_ref() == Some(&message.group);
    let history = state.histories.entry(message.group.clone()).or_default();
    let by_id = history.messages.iter().position(|held| held.id == message.id);
    let by_local = local_id.and_then(|local| history.messages.iter().position(|held| held.pending && held.id == local));
    let is_new = by_id.is_none() && by_local.is_none();
    let mut replaces = None;
    let mut shown = true;
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
    if !message.pending {
      self.save_history(&message.group, &state.histories[&message.group]);
    }
    if shown {
      let incoming = is_new && !message.pending && message.sender_id != me_id;
      self.emit(Event::MessageUpsert {
        message: message.clone(),
        replaces,
        incoming,
      });
    }

    if is_new && let Some(group) = state.groups.get_mut(&message.group) {
      let text = if message.text.trim().is_empty() {
        "Sent an attachment".to_string()
      } else {
        message.text.clone()
      };
      group.sort_time = group.sort_time.max(message.time);
      group.snippet = Some(Snippet {
        sender_id: message.sender_id.clone(),
        sender_name: message.sender_name.clone(),
        text: text.split_whitespace().collect::<Vec<_>>().join(" "),
        time: message.time,
      });
      if message.sender_id == me_id {
        group.unread = false;
      } else if !open {
        group.unread = true;
      }
      self.emit(Event::GroupUpsert(group.clone()));
    }
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
    let Some(config) = config else {
      return self.emit(Event::Mcp(McpStatus::Off));
    };
    let port = config.port;
    match mcp::serve(self.clone(), config).await {
      Ok(server) => {
        *running = Some(server);
        self.emit(Event::Mcp(McpStatus::Running { port }));
      },
      Err(error) => self.emit(Event::Mcp(McpStatus::Failed(error.to_string()))),
    }
  }

  pub(crate) fn me(&self) -> Option<User> {
    self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).me.clone()
  }

  /// Every conversation with the text it can be found by: its name and, for a one-to-one chat, the other
  /// side's email.
  pub(crate) fn directory(&self) -> Vec<(Group, String)> {
    let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    state
      .groups
      .values()
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

  /// Fetches one page of a conversation without touching what the app shows.
  pub(crate) async fn topics(&self, group: &GroupId, page: Page) -> Result<TopicPage> {
    let response = self.list_topics(group, page).await?;
    let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut topics: Vec<(Micros, Vec<Arc<Message>>)> = response
      .topics
      .iter()
      .map(|topic| {
        let messages = topic
          .replies
          .iter()
          .filter_map(|message| convert::message(message, &state.users));
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

  /// Sends a message and waits for the server to take it. The app shows it like one typed there.
  pub(crate) async fn send_now(
    self: &Arc<Self>,
    group: &GroupId,
    topic_id: Option<&str>,
    text: &str,
    files: Vec<PathBuf>,
  ) -> Result<()> {
    if self.readonly {
      return Err(anyhow!("Working runs in read-only mode"));
    }
    let attached = files.iter().map(|path| local_attachment(path)).collect();
    let pending = self
      .add_pending(group, topic_id, text, &[], None, attached)
      .ok_or_else(|| anyhow!("the account is not loaded yet"))?;
    let result = async {
      let uploads = self.upload_files(group, files).await?;
      self.send_message(group, topic_id, text, &[], None, uploads, &pending).await
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
            // Events were missed while offline: reload the list and the conversation on screen.
            self.queue_world_refresh();
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
    for body in bodies {
      if let Some(posted) = body.message_posted.as_ref().and_then(|posted| posted.message.as_ref()) {
        let converted = convert::message(
          posted,
          &self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).users,
        );
        if let Some(message) = converted {
          let known = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .groups
            .contains_key(&message.group);
          self.upsert_message(Arc::new(message), posted.local_id.as_deref());
          if !known {
            self.queue_world_refresh();
          }
        }
      }
      if let Some(deleted) = &body.message_deleted {
        self.on_deleted(deleted);
      }
      if let Some(reaction) = &body.message_reaction {
        self.on_reaction(reaction);
      }
      // Read state moved, here or on another device.
      let viewed = body
        .group_viewed
        .as_ref()
        .map(|viewed| (viewed.group_id.as_ref(), viewed.view_time, false));
      let read_state = body.group_read_state_updated_event.as_ref().map(|updated| {
        (
          updated.group_id.as_ref(),
          updated.most_recent_read_time,
          updated.mark_as_unread_time.unwrap_or(0) > 0,
        )
      });
      for (group, read_time, marked_unread) in viewed.into_iter().chain(read_state) {
        let Some(group) = group.and_then(convert::group_id).or_else(|| event_group.clone()) else {
          continue;
        };
        let me = self.me_id();
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
          self.update_group(&group, |group| group.unread = true);
        }
      }
      if let Some(typing) = &body.typing_state_changed_event {
        self.on_typing(typing);
      }
      if let Some(updated) = &body.user_status_updated_event
        && let Some(status) = &updated.user_status
      {
        let id = status.user_id.as_ref().and_then(|id| id.id.as_deref()).unwrap_or("");
        if id.is_empty() || id == self.me_id() {
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
    let user_name = state.users.get(&user_id).map(|user| user.name.clone()).unwrap_or_default();
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

/// The time zone of this Mac, as Google Chat names zones.
pub fn device_time_zone() -> String {
  iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into())
}

// Presence and do-not-disturb schedules.
impl Engine {
  async fn presence_loop(self: Arc<Self>) {
    loop {
      tokio::time::sleep(HEARTBEAT).await;
      let presence = *self.presence.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      if let Some(presence) = presence {
        self.heartbeat(presence).await;
      }
    }
  }

  async fn heartbeat(&self, presence: Presence) {
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
    let _ = self.http.api::<_, proto::HeartbeatResponse>("heartbeat", &request).await;
  }

  /// The schedules sit behind two of the web app's page RPCs. Both speak the same shape:
  /// `[_, match time zone, time zone, [[_, days, start minute, end minute, enabled], ...]]`.
  async fn load_dnd(&self) -> Result<()> {
    let answer = self.http.page_rpc("bqAG7c", &serde_json::json!([device_time_zone()])).await?;
    let settings = &answer[0];
    let flag = |value: &serde_json::Value| value.as_bool().unwrap_or(value.as_i64() == Some(1));
    let schedules = settings[3]
      .as_array()
      .map(Vec::as_slice)
      .unwrap_or_default()
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
    loop {
      self.refresh_mine().await;
      let people: Vec<String> = {
        let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let direct = state.groups.values().filter(|group| group.kind == GroupKind::Dm);
        direct
          .flat_map(|group| group.member_ids.iter().cloned())
          .collect::<HashSet<_>>()
          .into_iter()
          .collect()
      };
      // Until the conversations are known there is nobody to ask about.
      if people.is_empty() {
        tokio::time::sleep(Duration::from_secs(3)).await;
        continue;
      }
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
      if let Ok(answer) = self
        .http
        .api::<_, proto::GetUserPresenceResponse>("get_user_presence", &request)
        .await
      {
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
      }
      tokio::time::sleep(STATUSES_EVERY).await;
    }
  }

  async fn refresh_mine(&self) {
    let request = proto::GetSelfUserStatusRequest {
      request_header: Some(request_header()),
      ..Default::default()
    };
    if let Ok(answer) = self
      .http
      .api::<_, proto::GetSelfUserStatusResponse>("get_self_user_status", &request)
      .await
      && let Some(status) = answer.user_status
    {
      self.emit(Event::Mine(mine_from(&status)));
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
    let members: Vec<proto::UserId> = ids
      .iter()
      .map(|id| proto::UserId {
        id: Some(id.clone()),
        ..Default::default()
      })
      .collect();
    let invitees: Vec<proto::InviteeInfo> = members
      .iter()
      .cloned()
      .map(|user_id| proto::InviteeInfo {
        user_id: Some(user_id),
        ..Default::default()
      })
      .collect();
    let request = proto::CreateDmRequest {
      request_header: Some(request_header()),
      members,
      invitees,
      retention_settings: Some(proto::RetentionSettings {
        state: Some(proto::retention_settings::RetentionState::Permanent as i32),
        ..Default::default()
      }),
      local_id: Some(format!("working%{}", rand::random::<u64>())),
      force_empty_dm_visible: Some(true),
      ..Default::default()
    };
    let answer: proto::CreateDmResponse = self.http.api("create_dm", &request).await?;
    let group_id = answer
      .dm
      .as_ref()
      .and_then(|dm| dm.group_id.as_ref())
      .and_then(convert::group_id);
    let Some(group_id) = group_id else {
      bail!("Google Chat did not return the new conversation")
    };
    self.fetch_users(ids.clone()).await.ok();
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

fn people_list(users: &[User], me: &str) -> Vec<User> {
  let mut people: Vec<User> = users
    .iter()
    .filter(|user| !user.is_bot && !user.deleted && user.id != me && !user.name.is_empty())
    .cloned()
    .collect();
  people.sort_by_key(|a| a.name.to_lowercase());
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
    pinned: false,
    muted: false,
    notify: Notify::All,
    snippet: None,
    placement: Placement::ByKind,
  }
}

/// The request header of the web app's JSON API calls, with the client capabilities it declares.
const WEB_REQUEST_HEADER: &str = r#"[0,3,1,"en",[null,null,null,null,2,2,null,2,2,2,2,null,null,null,null,2,2,2,2,2,2,2,2,2,2,2,2,2,2,null,null,2,2,null,null,2,2,2,null,null,null,null,2,2,2,2,null,2,null,null,2,null,2,2,2,2,null,2,null,2,2,2,2,2,2,2,2,2,2,2,null,2,2,null,2,2,null,2]]"#;

/// The web app's world section of starred messages: up to 100 entries filtered by the starred label.
const STARRED_SECTION: &str = r#"[100,null,null,null,null,null,null,null,null,[[1,1],[1]],[4],null,null,null,null,[[[3,[null,null,null,null,[2,[null,null,null,null,null,null,null,"label"],[null,null,null,null,null,null,null,"100G"]]]]]]]"#;

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
    sender_name: sender.map(|user| user.name.clone()).unwrap_or_default(),
    sender_avatar: sender.and_then(|user| user.avatar_url.clone()),
    sender_id,
    text: message[9].as_str().unwrap_or_default().to_string(),
    time: message[2].as_str().and_then(|time| time.parse().ok())?,
  })
}

#[cfg(test)]
mod tests {
  use super::{click_card_request, file_name, meeting_link, people_list, starred_hit};
  use crate::convert::Users;
  use crate::model::GroupId;
  use crate::model::{CardAction, User};
  use crate::proto;
  use std::path::Path;

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
}
