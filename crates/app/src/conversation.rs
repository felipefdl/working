//! An open conversation: header, bottom-anchored virtualized message list, composer.

use std::cell::Cell;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gchat_core::{Command, Core, Group, GroupKind, Message, SlashCommand, User};
use gpui_kit::component::input::{Input, InputEvent, InputState, Paste, RopeExt as _, Textarea, TextareaState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::completion::{self, Menu, Trigger};
use crate::images::Images;
use crate::message::{self, Act, Actor, MsgCtx, Opener, Row};
use crate::settings::{Density, RewriteTool, Settings};
use crate::store::{History, Store};
use crate::theme::{ACCENT, ACCENT_BG, BORDER, FIELD, MUTED, PLACEHOLDER, POPOVER, ROW_HOVER, ROW_SELECTED, TEXT, rgb};
use crate::ui::{Icon, group_avatar, icon};

const TYPING_IDLE: Duration = Duration::from_secs(5);
const TYPING_EXPIRY: Duration = Duration::from_secs(8);

/// Older-history paging state, shared with the list's scroll handler.
struct LoadState {
  loading: Cell<bool>,
  reached: Cell<bool>,
  /// The last load added nothing: do not retry until the user scrolls away and back.
  stalled: Cell<bool>,
  /// False while the list is a window around a search hit: newer pages load at the bottom.
  at_end: Cell<bool>,
  loading_newer: Cell<bool>,
  /// Row count, for the scroll handler to know when the last row is in view.
  rows: Cell<usize>,
}

/// The open thread panel: its rows (starting message first, then replies) are rebuilt with the message list.
struct Thread {
  topic: String,
  rows: Vec<Row>,
  list: ListState,
  composer: Entity<TextareaState>,
  /// Files that go out with the next reply.
  files: Vec<PathBuf>,
  _sub: Subscription,
}

/// The message whose text is in a composer to be changed, and which composer that is.
struct Editing {
  message_id: String,
  in_thread: bool,
}

struct Find {
  input: Entity<InputState>,
  query: String,
  hits: Vec<(String, Range<usize>)>,
  index: usize,
  _sub: Subscription,
}

pub struct ConversationView {
  store: WeakEntity<Store>,
  core: Core,
  images: Images,
  group: Group,
  me_id: String,
  me_avatar: Option<String>,
  messages: Arc<Vec<Arc<Message>>>,
  rows: Vec<Row>,
  /// The message layout the lists were measured with.
  density: Density,
  thread: Option<Thread>,
  opener: Opener,
  /// Opens the chat with the person a name or a mention names.
  open_person: Opener,
  actor: Actor,
  /// The message the next one sent from the main composer quotes.
  quoting: Option<Arc<Message>>,
  editing: Option<Editing>,
  /// Files that go out with the next message from the main composer.
  files: Vec<PathBuf>,
  /// The message under the pointer, which shows its toolbar.
  hovered: Option<String>,
  /// The pointer is over a message list.
  in_list: bool,
  on_tools: Rc<Cell<bool>>,
  /// The thread panel vanished while its composer had focus: give focus back to the main composer.
  refocus: bool,
  loaded: bool,
  list: ListState,
  composer: Entity<TextareaState>,
  load: Rc<LoadState>,
  typing_sent: bool,
  typing_task: Option<Task<()>>,
  typing: Vec<(String, String, Instant)>,
  _subs: Vec<Subscription>,
  _expiry: Task<()>,
  _top_check: Option<Task<()>>,
  /// The message a search hit jumped to, outlined for a moment.
  focus: Option<String>,
  _focus_clear: Option<Task<()>>,
  /// The user asked for the latest messages: scroll to the end when they arrive.
  to_latest: bool,
  rewrite_task: Option<Task<()>>,
  /// Which composer is being rewritten: `Some(false)` main, `Some(true)` thread.
  rewriting: Option<bool>,
  rewrite_error: Option<String>,
  _rewrite_error_clear: Option<Task<()>>,
  find: Option<Find>,
  /// Who `@` offers, once `Command::LoadMembers` answered.
  members: Option<Arc<Vec<User>>>,
  /// What `/` offers, once `Command::LoadSlashCommands` answered.
  commands: Option<Arc<Vec<SlashCommand>>>,
  /// Members and commands are asked for on the first `@` or `/`, once.
  asked_members: bool,
  asked_commands: bool,
  /// The `@` or `/` menu over one of the composers.
  menu: Option<Menu>,
}

impl ConversationView {
  #[allow(
    clippy::too_many_arguments,
    reason = "GPUI view construction takes independent handles"
  )]
  pub fn new(
    store: WeakEntity<Store>,
    core: Core,
    images: Images,
    group: Group,
    me: Option<User>,
    history: Option<History>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let (messages, reached, loaded) = match history {
      Some(h) => (h.messages, h.reached_start, true),
      None => (Arc::new(Vec::new()), true, false),
    };
    let me_id = me.as_ref().map(|u| u.id.clone()).unwrap_or_default();
    let me_avatar = me.and_then(|u| u.avatar_url);
    let rows = message::build_rows(&messages, &me_id);
    let list = ListState::new(rows.len(), ListAlignment::Bottom, px(600.));
    let load = Rc::new(LoadState {
      loading: Cell::new(false),
      reached: Cell::new(reached),
      stalled: Cell::new(false),
      at_end: Cell::new(true),
      loading_newer: Cell::new(false),
      rows: Cell::new(rows.len()),
    });
    {
      let (load, core, id) = (load.clone(), core.clone(), group.id.clone());
      list.set_scroll_handler(move |event, _, _| {
        if event.visible_range.start == 0 {
          if !load.reached.get() && !load.loading.get() && !load.stalled.get() {
            load.loading.set(true);
            core.send(Command::LoadOlder(id.clone()));
          }
        } else {
          load.stalled.set(false);
        }
        if event.visible_range.end >= load.rows.get() && !load.at_end.get() && !load.loading_newer.get() {
          load.loading_newer.set(true);
          core.send(Command::LoadNewer(id.clone()));
        }
      });
    }

    let composer = cx.new(|cx| {
      TextareaState::new(window, cx)
        .auto_grow(1, 8)
        .submit_on_enter(true)
        .placeholder("Message")
    });
    let subs = vec![
      cx.subscribe_in(&composer, window, |this, _, event: &InputEvent, window, cx| match event {
        InputEvent::Change => {
          this.on_typed(cx);
          this.refresh_menu(false, cx);
        },
        InputEvent::PressEnter { shift: false, .. } => this.submit(false, window, cx),
        _ => {},
      }),
    ];
    composer.update(cx, |state, cx| state.focus(window, cx));

    let expiry = cx.spawn(async move |this, cx| {
      loop {
        cx.background_executor().timer(Duration::from_secs(1)).await;
        if this.update(cx, Self::prune_typing).is_err() {
          break;
        }
      }
    });
    let open_person: Opener = {
      let store = store.clone();
      Rc::new(move |user_id, window, cx| {
        store.update(cx, |store, cx| store.open_person(user_id, window, cx)).ok();
      })
    };
    let opener: Opener = {
      let weak = cx.weak_entity();
      Rc::new(move |topic, window, cx| {
        weak.update(cx, |this, cx| this.open_thread(topic, window, cx)).ok();
      })
    };
    let actor: Actor = {
      let weak = cx.weak_entity();
      Rc::new(move |act, message, window, cx| {
        weak.update(cx, |this, cx| this.on_act(act, message, window, cx)).ok();
      })
    };
    let mut view = Self {
      store,
      core,
      images,
      group,
      me_id,
      me_avatar,
      messages,
      rows,
      density: cx.global::<Settings>().density,
      thread: None,
      opener,
      open_person,
      actor,
      quoting: None,
      editing: None,
      files: Vec::new(),
      hovered: None,
      in_list: false,
      on_tools: Rc::new(Cell::new(false)),
      refocus: false,
      loaded,
      list,
      composer,
      load,
      typing_sent: false,
      typing_task: None,
      typing: Vec::new(),
      _subs: subs,
      _expiry: expiry,
      _top_check: None,
      focus: None,
      _focus_clear: None,
      to_latest: false,
      rewrite_task: None,
      rewriting: None,
      rewrite_error: None,
      _rewrite_error_clear: None,
      find: None,
      members: None,
      commands: None,
      asked_members: false,
      asked_commands: false,
      menu: None,
    };
    // Rows change height with the message layout: measure the lists again.
    view._subs.push(cx.observe_global::<Settings>(|this, cx| {
      let density = cx.global::<Settings>().density;
      if density != this.density {
        this.density = density;
        this.list.reset(this.rows.len());
        if let Some(thread) = &this.thread {
          thread.list.reset(thread.rows.len());
        }
      }
      cx.notify();
    }));
    view.schedule_top_check(cx);
    view
  }

  pub fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.composer.update(cx, |state, cx| state.focus(window, cx));
  }

  pub fn set_group(&mut self, group: Group, cx: &mut Context<Self>) {
    self.group = group;
    cx.notify();
  }

  pub fn set_me(&mut self, me: User, cx: &mut Context<Self>) {
    self.me_avatar = me.avatar_url;
    self.me_id = me.id;
    self.rebuild(0);
    cx.notify();
  }

  /// Takes the full message list. Returns true when a message from someone else arrived at the end.
  pub fn set_history(
    &mut self,
    messages: Arc<Vec<Arc<Message>>>,
    reached_start: bool,
    reached_end: bool,
    prepended: usize,
    focus: Option<String>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> bool {
    let incoming = match (messages.last(), self.messages.last()) {
      (Some(new), Some(old)) => reached_end && new.id != old.id && !new.pending && new.sender_id != self.me_id,
      _ => false,
    };
    self.messages = messages;
    self.loaded = true;
    self.rebuild(prepended);

    if self.load.loading.get() {
      self.load.stalled.set(prepended == 0 && !reached_start);
      self.load.loading.set(false);
    }
    self.load.reached.set(reached_start);
    self.load.at_end.set(reached_end);
    self.load.loading_newer.set(false);
    if let Some(id) = focus {
      self.focus_message(id, window, cx);
    } else if reached_end && self.to_latest {
      self.to_latest = false;
      self.list.scroll_to_end();
    }
    if self.find.is_some() {
      self.refresh_find(window, cx);
    }
    self.schedule_top_check(cx);
    cx.notify();
    incoming
  }

  /// Scrolls to a message and outlines it for a few seconds. A thread reply opens its thread.
  fn focus_message(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
    let Some(message) = self.messages.iter().find(|message| message.id == id).cloned() else {
      return;
    };
    let top_level = if message.is_thread_reply() {
      message.topic_id.clone()
    } else {
      id.clone()
    };
    if let Some(ix) = self.rows.iter().position(|row| row.msg.id == top_level) {
      // The hit's own row: one row above can be a tall image that pushes the hit below the fold.
      self.list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) });
    }
    if message.is_thread_reply() {
      self.open_thread(&top_level, window, cx);
    }
    self.focus = Some(id);
    if self.find.is_some() {
      self._focus_clear = None;
      return;
    }
    self._focus_clear = Some(cx.spawn(async move |this, cx| {
      cx.background_executor().timer(Duration::from_secs(3)).await;
      let _ = this.update(cx, |this, cx| {
        this.focus = None;
        cx.notify();
      });
    }));
  }

  pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if let Some(find) = &self.find {
      let input = find.input.clone();
      input.update(cx, |state, cx| state.focus(window, cx));
      return;
    }
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find"));
    let sub = cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| match event {
      InputEvent::Change => {
        let query = input.read(cx).value().to_string();
        this.set_find_query(query, window, cx);
      },
      InputEvent::PressEnter { shift, .. } => {
        if *shift {
          this.find_prev(window, cx);
        } else {
          this.find_next(window, cx);
        }
      },
      _ => {},
    });
    input.update(cx, |state, cx| state.focus(window, cx));
    self.find = Some(Find {
      input,
      query: String::new(),
      hits: Vec::new(),
      index: 0,
      _sub: sub,
    });
    cx.notify();
  }

  pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    if self.find.take().is_none() {
      return false;
    }
    self.focus = None;
    self._focus_clear = None;
    self.composer.update(cx, |state, cx| state.focus(window, cx));
    cx.notify();
    true
  }

  pub fn find_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(find) = &mut self.find else { return };
    if find.hits.is_empty() {
      return;
    }
    find.index = (find.index + 1) % find.hits.len();
    let id = find.hits[find.index].0.clone();
    self.focus_message(id, window, cx);
    cx.notify();
  }

  pub fn find_prev(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(find) = &mut self.find else { return };
    if find.hits.is_empty() {
      return;
    }
    find.index = if find.index == 0 {
      find.hits.len() - 1
    } else {
      find.index - 1
    };
    let id = find.hits[find.index].0.clone();
    self.focus_message(id, window, cx);
    cx.notify();
  }

  fn set_find_query(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
    let Some(find) = &mut self.find else { return };
    find.query = query;
    self.refresh_find(window, cx);
  }

  fn refresh_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(find) = &self.find else { return };
    let query = find.query.clone();
    let current = find.hits.get(find.index).cloned();
    let mut hits = Vec::new();
    if !query.is_empty() {
      for message in self.messages.iter() {
        for range in message::find_ranges(&message.text, &query) {
          hits.push((message.id.clone(), range));
        }
      }
    }
    let index = current
      .and_then(|(id, range)| hits.iter().position(|hit| hit.0 == id && hit.1 == range))
      .unwrap_or(0);
    let jump = hits.get(index).map(|(id, _)| id.clone());
    if let Some(find) = &mut self.find {
      find.hits = hits;
      find.index = index;
    }
    if let Some(id) = jump {
      if self.focus.as_deref() != Some(id.as_str()) {
        self.focus_message(id, window, cx);
      }
    } else {
      self.focus = None;
    }
    cx.notify();
  }

  fn find_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
    let find = self.find.as_ref()?;
    let count = if find.query.is_empty() {
      String::new()
    } else if find.hits.is_empty() {
      "No matches".into()
    } else {
      format!("{} of {}", find.index + 1, find.hits.len())
    };
    Some(
      div()
        .absolute()
        .top(px(8.))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
          h_flex()
            .w_full()
            .max_w(px(480.))
            .h(px(36.))
            .px_3()
            .gap_2()
            .items_center()
            .rounded_full()
            .bg(rgb(POPOVER))
            .border_1()
            .border_color(rgb(BORDER))
            .child(icon(Icon::Search, 15., MUTED))
            .child(
              div()
                .flex_1()
                .min_w_0()
                .child(Input::new(&find.input).appearance(false).bordered(false)),
            )
            .child(div().flex_shrink_0().text_size(px(12.)).text_color(rgb(MUTED)).child(count))
            .child(
              div()
                .id("find-prev")
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(ROW_HOVER)))
                .on_click(cx.listener(|this, _, window, cx| this.find_prev(window, cx)))
                .child(icon(Icon::ChevronUp, 16., TEXT)),
            )
            .child(
              div()
                .id("find-next")
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(ROW_HOVER)))
                .on_click(cx.listener(|this, _, window, cx| this.find_next(window, cx)))
                .child(icon(Icon::ChevronDown, 16., TEXT)),
            )
            .child(
              div()
                .id("find-close")
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(ROW_HOVER)))
                .on_click(cx.listener(|this, _, window, cx| {
                  this.close_find(window, cx);
                }))
                .child(icon(Icon::X, 16., MUTED)),
            ),
        )
        .into_any_element(),
    )
  }

  /// Leaves the window around a search hit for the latest messages.
  fn jump_latest(&mut self, cx: &mut Context<Self>) {
    self.to_latest = true;
    self.core.send(Command::Open(self.group.id.clone()));
    cx.notify();
  }

  /// Rebuilds the main list rows and the thread panel rows from the current messages.
  fn rebuild(&mut self, prepended: usize) {
    let rows = message::build_rows(&self.messages, &self.me_id);
    apply_rows(&self.list, &mut self.rows, rows, prepended);
    self.load.rows.set(self.rows.len());
    let Some(thread) = &mut self.thread else {
      return;
    };
    if let Some(rows) = message::build_thread_rows(&self.messages, &thread.topic, &self.me_id) {
      apply_rows(&thread.list, &mut thread.rows, rows, 0)
    } else {
      // The starting message is gone.
      self.thread = None;
      self.refocus = true;
    }
  }

  /// After a layout pass: when the whole history fits or the list sits at the top, ask for older messages.
  fn schedule_top_check(&mut self, cx: &mut Context<Self>) {
    self._top_check = Some(cx.spawn(async move |this, cx| {
      cx.background_executor().timer(Duration::from_millis(150)).await;
      let _ = this.update(cx, |this, _| this.check_top());
    }));
  }

  fn check_top(&mut self) {
    let load = &self.load;
    if load.reached.get() || load.loading.get() || load.stalled.get() {
      return;
    }
    if self.list.viewport_bounds().size.height <= px(0.) {
      return;
    }
    let top = self.list.logical_scroll_top();
    let fits = self.list.max_offset_for_scrollbar().y <= px(0.);
    let at_top = top.item_ix == 0 && top.offset_in_item <= px(1.);
    if fits || at_top {
      load.loading.set(true);
      self.core.send(Command::LoadOlder(self.group.id.clone()));
    }
  }

  // Typing.

  pub fn set_typing(&mut self, user_id: String, name: String, typing: bool, cx: &mut Context<Self>) {
    self.typing.retain(|(id, ..)| id != &user_id);
    if typing {
      self.typing.push((user_id, name, Instant::now()));
    }
    cx.notify();
  }

  fn prune_typing(&mut self, cx: &mut Context<Self>) {
    let before = self.typing.len();
    self.typing.retain(|(_, _, at)| at.elapsed() < TYPING_EXPIRY);
    if self.typing.len() != before {
      cx.notify();
    }
  }

  fn on_typed(&mut self, cx: &mut Context<Self>) {
    if !self.typing_sent {
      self.typing_sent = true;
      self.core.send(Command::SetTyping {
        group: self.group.id.clone(),
        typing: true,
      });
    }
    self.typing_task = Some(cx.spawn(async move |this, cx| {
      cx.background_executor().timer(TYPING_IDLE).await;
      let _ = this.update(cx, |this, _| this.stop_typing());
    }));
  }

  fn stop_typing(&mut self) {
    self.typing_task = None;
    if self.typing_sent {
      self.typing_sent = false;
      self.core.send(Command::SetTyping {
        group: self.group.id.clone(),
        typing: false,
      });
    }
  }

  fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.rewriting == Some(false) {
      return;
    }
    let (text, chips) = completion::message(&self.composer.read(cx).content());
    let editing = self.editing.as_ref().is_some_and(|editing| !editing.in_thread);
    // Files alone make a message; a change to an existing one needs text.
    if text.is_empty() && (editing || self.files.is_empty()) {
      return;
    }
    let group = self.group.id.clone();
    if let Some(editing) = self.editing.take_if(|editing| !editing.in_thread) {
      self.core.send(Command::Edit {
        group,
        message_id: editing.message_id,
        text,
      });
    } else {
      // A new message belongs at the end: leave a window around a search hit first.
      if !self.load.at_end.get() {
        self.jump_latest(cx);
      }
      let quote = self.quoting.take().map(|message| message.id.clone());
      let files = std::mem::take(&mut self.files);
      self.core.send(Command::Send {
        group,
        topic_id: None,
        text,
        quote,
        files,
        chips,
      });
      self.list.scroll_to_end();
    }
    self.stop_typing();
    self.composer.update(cx, |state, cx| state.set_value("", window, cx));
    cx.notify();
  }

  fn list_hovered(&mut self, hovered: bool, window: &mut Window, cx: &mut Context<Self>) {
    self.in_list = hovered;
    if !hovered {
      self.pointer_left(window, cx);
    }
  }

  /// The pointer left a list or a toolbar. Once the move is over, the toolbar goes away unless the pointer
  /// ended up on one of them: a toolbar covers its list, so entering it reads as leaving the list.
  fn pointer_left(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    cx.defer_in(window, |this, _, cx| {
      if !this.in_list && !this.on_tools.get() && this.hovered.take().is_some() {
        cx.notify();
      }
    });
  }

  /// A button of a message's hover toolbar.
  fn on_act(&mut self, act: Act, message: &Arc<Message>, window: &mut Window, cx: &mut Context<Self>) {
    match act {
      Act::Hover => {
        if self.hovered.as_deref() != Some(message.id.as_str()) {
          self.hovered = Some(message.id.clone());
          cx.notify();
        }
        return;
      },
      Act::Leave => return self.pointer_left(window, cx),
      Act::Thread => self.open_thread(&message.id, window, cx),
      Act::Quote => {
        if self.editing.take_if(|editing| !editing.in_thread).is_some() {
          self.composer.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.quoting = Some(message.clone());
        self.composer.update(cx, |state, cx| state.focus(window, cx));
      },
      Act::Edit => {
        // A reply is only on screen in the thread panel, and is edited in that panel's composer.
        let panel = self
          .thread
          .as_ref()
          .filter(|thread| message.is_thread_reply() && thread.topic == message.topic_id);
        let in_thread = panel.is_some();
        let composer = panel.map_or_else(|| self.composer.clone(), |thread| thread.composer.clone());
        if !in_thread {
          self.quoting = None;
        }
        self.editing = Some(Editing {
          message_id: message.id.clone(),
          in_thread,
        });
        let text = message.text.clone();
        composer.update(cx, |state, cx| {
          state.set_value(text, window, cx);
          // A multi-line value puts the caret at the start; editing goes on from the end.
          let end = state.text().offset_to_position(state.text().len());
          state.set_cursor_position(end, window, cx);
        });
      },
    }
    cx.notify();
  }

  /// Drops a pending quote or edit. Returns false when there was none.
  pub fn cancel_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    let quoting = self.quoting.take().is_some();
    let Some(editing) = self.editing.take() else {
      cx.notify();
      return quoting;
    };
    let composer = match (&self.thread, editing.in_thread) {
      (Some(thread), true) => thread.composer.clone(),
      _ => self.composer.clone(),
    };
    composer.update(cx, |state, cx| state.set_value("", window, cx));
    cx.notify();
    true
  }

  /// The line above a composer that says what it is about to do: change a message, or quote one.
  fn draft_banner(&self, in_thread: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
    let editing = self.editing.as_ref().is_some_and(|editing| editing.in_thread == in_thread);
    let (glyph, label) = match (&self.quoting, editing) {
      (_, true) => (Icon::Pencil, "Editing message".to_string()),
      (Some(quoted), false) if !in_thread => {
        let text: String = quoted.text.chars().take(120).map(|c| if c == '\n' { ' ' } else { c }).collect();
        (Icon::Reply, format!("Replying to {}: {text}", quoted.sender_name))
      },
      _ => return None,
    };
    Some(
      h_flex()
        .w_full()
        .max_w(px(780.))
        .px_3()
        .pb_1()
        .gap_2()
        .items_center()
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .child(icon(glyph, 13., MUTED))
        .child(div().flex_1().min_w_0().truncate().child(label))
        .child(
          div()
            .id(("cancel-draft", usize::from(in_thread)))
            .size(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(ROW_HOVER)))
            .on_click(cx.listener(|this, _, window, cx| {
              this.cancel_draft(window, cx);
            }))
            .child(icon(Icon::X, 13., MUTED)),
        )
        .into_any_element(),
    )
  }

  /// The header's arrow: the same step back as Cmd+[.
  fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.store.update(cx, |store, cx| store.go_back(window, cx)).ok();
  }

  // Thread panel.

  fn open_thread(&mut self, topic: &str, window: &mut Window, cx: &mut Context<Self>) {
    if let Some(thread) = &self.thread
      && thread.topic == topic
    {
      thread.composer.update(cx, |state, cx| state.focus(window, cx));
      return;
    }
    let Some(rows) = message::build_thread_rows(&self.messages, topic, &self.me_id) else {
      return;
    };
    let list = ListState::new(rows.len(), ListAlignment::Bottom, px(600.));
    let composer = cx.new(|cx| {
      TextareaState::new(window, cx)
        .auto_grow(1, 8)
        .submit_on_enter(true)
        .placeholder("Reply")
    });
    let sub = cx.subscribe_in(&composer, window, |this, _, event: &InputEvent, window, cx| match event {
      InputEvent::Change => this.refresh_menu(true, cx),
      InputEvent::PressEnter { shift: false, .. } => this.submit(true, window, cx),
      _ => {},
    });
    composer.update(cx, |state, cx| state.focus(window, cx));
    self.thread = Some(Thread {
      topic: topic.to_string(),
      rows,
      list,
      composer,
      files: Vec::new(),
      _sub: sub,
    });
    cx.notify();
  }

  /// Closes the thread panel. Returns false when none was open.
  pub fn close_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    if self.thread.take().is_none() {
      return false;
    }
    self.editing.take_if(|editing| editing.in_thread);
    self.composer.update(cx, |state, cx| state.focus(window, cx));
    cx.notify();
    true
  }

  fn send_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.rewriting == Some(true) {
      return;
    }
    let Some(thread) = &mut self.thread else {
      return;
    };
    let (text, chips) = completion::message(&thread.composer.read(cx).content());
    let editing = self.editing.as_ref().is_some_and(|editing| editing.in_thread);
    if text.is_empty() && (editing || thread.files.is_empty()) {
      return;
    }
    let group = self.group.id.clone();
    if let Some(editing) = self.editing.take_if(|editing| editing.in_thread) {
      self.core.send(Command::Edit {
        group,
        message_id: editing.message_id,
        text,
      });
    } else {
      let files = std::mem::take(&mut thread.files);
      self.core.send(Command::Send {
        group,
        topic_id: Some(thread.topic.clone()),
        text,
        quote: None,
        files,
        chips,
      });
      thread.list.scroll_to_end();
    }
    thread.composer.update(cx, |state, cx| state.set_value("", window, cx));
    cx.notify();
  }

  // Attachments.

  /// Adds files (dropped on the conversation, or picked) to the message being written.
  fn attach(&mut self, in_thread: bool, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
    let (files, composer) = match (&mut self.thread, in_thread) {
      (Some(thread), true) => (&mut thread.files, &thread.composer),
      (_, false) => (&mut self.files, &self.composer),
      (None, true) => return,
    };
    for path in paths {
      // A folder cannot be sent.
      if path.is_file() && !files.contains(path) {
        files.push(path.clone());
      }
    }
    composer.update(cx, |state, cx| state.focus(window, cx));
    cx.notify();
  }

  /// Cmd+V in a composer: files and pictures on the clipboard are attached; text is left to the composer.
  fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
    let in_thread = match &self.thread {
      Some(thread) if thread.composer.focus_handle(cx).is_focused(window) => true,
      _ if self.composer.focus_handle(cx).is_focused(window) => false,
      _ => return,
    };
    let Some(item) = cx.read_from_clipboard() else {
      return;
    };
    let mut paths = Vec::new();
    for entry in item.entries() {
      match entry {
        ClipboardEntry::ExternalPaths(copied) => paths.extend(copied.paths().iter().cloned()),
        ClipboardEntry::Image(image) => paths.extend(save_pasted(image)),
        ClipboardEntry::String(_) => {},
      }
    }
    if paths.is_empty() {
      return;
    }
    cx.stop_propagation();
    self.attach(in_thread, &paths, window, cx);
  }

  fn detach(&mut self, in_thread: bool, ix: usize, cx: &mut Context<Self>) {
    let files = match (&mut self.thread, in_thread) {
      (Some(thread), true) => &mut thread.files,
      (_, false) => &mut self.files,
      (None, true) => return,
    };
    if ix < files.len() {
      files.remove(ix);
    }
    cx.notify();
  }

  /// Asks which files to attach, in the system's file dialog.
  fn pick_files(&mut self, in_thread: bool, window: &mut Window, cx: &mut Context<Self>) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
      files: true,
      directories: false,
      multiple: true,
      prompt: Some("Attach".into()),
    });
    cx.spawn_in(window, async move |this, cx| {
      if let Ok(Ok(Some(paths))) = picked.await {
        this
          .update_in(cx, |this, window, cx| this.attach(in_thread, &paths, window, cx))
          .ok();
      }
    })
    .detach();
  }

  /// The files waiting to go out with the next message, each with a button to take it off.
  fn attached(&self, in_thread: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
    let files = if in_thread {
      &self.thread.as_ref()?.files
    } else {
      &self.files
    };
    if files.is_empty() {
      return None;
    }
    let chips = files.iter().enumerate().map(|(ix, path)| {
      let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
      let glyph = if is_image(path) {
        // The backdrop is for pictures with see-through parts.
        img(path.clone())
          .size(px(28.))
          .flex_shrink_0()
          .rounded(px(6.))
          .bg(rgb(PLACEHOLDER))
          .object_fit(ObjectFit::Cover)
          .into_any_element()
      } else {
        icon(Icon::File, 16., MUTED).into_any_element()
      };
      h_flex()
        .max_w(px(260.))
        .h(px(40.))
        .gap_2()
        .pl_2()
        .pr_1()
        .items_center()
        .rounded(px(10.))
        .border_1()
        .border_color(rgb(BORDER))
        .child(glyph)
        .child(div().min_w_0().text_size(px(13.)).text_color(rgb(TEXT)).truncate().child(name))
        .child(
          div()
            .id((if in_thread { "detach-reply" } else { "detach" }, ix))
            .size(px(22.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(ROW_HOVER)))
            .on_click(cx.listener(move |this, _, _, cx| this.detach(in_thread, ix, cx)))
            .child(icon(Icon::X, 13., MUTED)),
        )
    });
    Some(
      h_flex()
        .w_full()
        .max_w(px(780.))
        .flex_wrap()
        .gap_2()
        .px_1()
        .pb_2()
        .children(chips)
        .into_any_element(),
    )
  }

  pub fn rewrite_hotkey(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.rewrite(self.thread.is_some(), false, window, cx);
  }

  /// Enter in a composer: sends, or with "Always rewrite" on, rewrites first and sends the result.
  fn submit(&mut self, in_thread: bool, window: &mut Window, cx: &mut Context<Self>) {
    let settings = cx.global::<Settings>();
    if settings.rewrite_always && settings.rewrite != RewriteTool::Off {
      self.rewrite(in_thread, true, window, cx);
    } else {
      self.send_from(in_thread, window, cx);
    }
  }

  fn send_from(&mut self, in_thread: bool, window: &mut Window, cx: &mut Context<Self>) {
    if in_thread {
      self.send_thread(window, cx);
    } else {
      self.send(window, cx);
    }
  }

  /// Rewrites the draft in place. With `then_send`, a successful rewrite goes out right away; a failed one stays
  /// in the composer, unsent, with the error under it.
  fn rewrite(&mut self, in_thread: bool, then_send: bool, window: &mut Window, cx: &mut Context<Self>) {
    if self.rewrite_task.is_some() {
      return;
    }
    let settings = cx.global::<Settings>();
    let tool = settings.rewrite;
    if tool == RewriteTool::Off {
      return;
    }
    let extra = settings.rewrite_instruction.clone();
    let composer = match (&self.thread, in_thread) {
      (Some(thread), true) => thread.composer.clone(),
      _ => self.composer.clone(),
    };
    let draft = composer.read(cx).value().to_string();
    let tokens = composer.read(cx).tokens().to_vec();
    if draft.trim().is_empty() {
      // Files alone have nothing to rewrite.
      if then_send {
        self.send_from(in_thread, window, cx);
      }
      return;
    }
    self.rewrite_error = None;
    self.rewriting = Some(in_thread);
    composer.update(cx, |state, cx| state.set_disabled(true, cx));
    let background = cx.background_executor().clone();
    self.rewrite_task = Some(cx.spawn_in(window, async move |this, cx| {
      let result = background.spawn(async move { crate::rewrite::run(tool, &extra, &draft) }).await;
      cx.update(|window, cx| {
        this.update(cx, |this, cx| {
          this.rewrite_task = None;
          this.rewriting = None;
          composer.update(cx, |state, cx| {
            state.set_disabled(false, cx);
            if let Ok(text) = &result {
              state.replace_all(text.clone(), window, cx);
              for (range, token) in completion::reattach(text, &tokens) {
                state.replace_range_with_token(range, token, window, cx).ok();
              }
            }
            state.focus(window, cx);
          });
          if then_send && result.is_ok() {
            this.send_from(in_thread, window, cx);
          }
          if let Err(error) = result {
            let error = if then_send {
              format!("{error} Not sent.")
            } else {
              error
            };
            this.rewrite_error = Some(error);
            this._rewrite_error_clear = Some(cx.spawn(async move |this, cx| {
              cx.background_executor().timer(Duration::from_secs(4)).await;
              let _ = this.update(cx, |this, cx| {
                this.rewrite_error = None;
                cx.notify();
              });
            }));
          }
          cx.notify();
        })
      })
      .ok();
    }));
    cx.notify();
  }

  /// A composer's field: the button that attaches files, then the text, then rewrite when it is on.
  fn composer_field(&self, composer: &Entity<TextareaState>, in_thread: bool, cx: &mut Context<Self>) -> Div {
    let settings = cx.global::<Settings>();
    let rewrite = settings.rewrite != RewriteTool::Off;
    let always = settings.rewrite_always;
    let busy = self.rewriting == Some(in_thread);
    let sparkle = icon(Icon::Sparkles, 18., if busy || always { ACCENT } else { MUTED });
    let sparkle = if busy {
      sparkle
        .with_animation(
          ("rewrite-pulse", usize::from(in_thread)),
          Animation::new(Duration::from_millis(900)).repeat(),
          |this, delta| this.opacity(0.65f32.mul_add((delta * std::f32::consts::PI).sin().abs(), 0.35)),
        )
        .into_any_element()
    } else {
      sparkle.into_any_element()
    };
    let field = h_flex()
      .w_full()
      .pl_2()
      .pr_4()
      .py_2()
      .gap_2()
      .items_end()
      .rounded(px(24.))
      .bg(rgb(if busy { ACCENT_BG } else { FIELD }))
      .child(
        div()
          .id(("attach", usize::from(in_thread)))
          .size(px(28.))
          .mb(px(3.))
          .flex_shrink_0()
          .flex()
          .items_center()
          .justify_center()
          .rounded_full()
          .cursor_pointer()
          .hover(|s| s.bg(rgb(BORDER)))
          .on_click(cx.listener(move |this, _, window, cx| this.pick_files(in_thread, window, cx)))
          .child(icon(Icon::Plus, 18., MUTED)),
      )
      .child(
        div()
          .flex_1()
          .min_w_0()
          .child(Textarea::new(composer).appearance(false).bordered(false).disabled(busy)),
      )
      .when(rewrite, |row| {
        row.child(
          div()
            .id(("rewrite", usize::from(in_thread)))
            .size(px(28.))
            .mb(px(3.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(BORDER)))
            .when(always, |button| button.bg(rgb(ACCENT_BG)))
            .tooltip(move |window, cx| {
              Tooltip::new(if always {
                "Rewrite. Always rewrite is on: right-click to turn it off"
              } else {
                "Rewrite. Right-click to always rewrite before sending"
              })
              .build(window, cx)
            })
            .on_click(cx.listener(move |this, _, window, cx| this.rewrite(in_thread, false, window, cx)))
            .on_mouse_down(MouseButton::Right, |_, _, cx| {
              cx.stop_propagation();
              Settings::update(cx, |settings| settings.rewrite_always = !settings.rewrite_always);
            })
            .child(sparkle),
        )
      });
    let menu = self.menu.as_ref().filter(|menu| menu.in_thread == in_thread).map(|menu| {
      let view = cx.weak_entity();
      let pick: Rc<dyn Fn(usize, &mut Window, &mut App)> = Rc::new(move |ix, window, cx| {
        view.update(cx, |this, cx| this.pick(ix, window, cx)).ok();
      });
      deferred(
        div()
          .absolute()
          .bottom_full()
          .left_0()
          .right_0()
          .mb_2()
          .child(menu.render(&self.images, pick, cx)),
      )
      .with_priority(1)
    });
    div().relative().w_full().max_w(px(780.)).child(field).children(menu)
  }

  // Mentions and slash commands.

  /// Members and commands of this conversation arrived: show them if a menu is waiting on them.
  pub fn set_members(&mut self, users: Arc<Vec<User>>, cx: &mut Context<Self>) {
    self.members = Some(users);
    self.refresh_menus(cx);
  }

  pub fn set_slash_commands(&mut self, commands: Arc<Vec<SlashCommand>>, cx: &mut Context<Self>) {
    self.commands = Some(commands);
    self.refresh_menus(cx);
  }

  fn refresh_menus(&mut self, cx: &mut Context<Self>) {
    let in_thread = self.menu.as_ref().map_or(self.thread.is_some(), |menu| menu.in_thread);
    self.refresh_menu(in_thread, cx);
  }

  fn composer_of(&self, in_thread: bool) -> Option<&Entity<TextareaState>> {
    if in_thread {
      self.thread.as_ref().map(|thread| &thread.composer)
    } else {
      Some(&self.composer)
    }
  }

  /// Opens, updates, or closes a composer's menu for the text before its cursor. A change to a message has no
  /// menu: an edit keeps only the text.
  fn refresh_menu(&mut self, in_thread: bool, cx: &mut Context<Self>) {
    let editing = self.editing.as_ref().is_some_and(|editing| editing.in_thread == in_thread);
    let found = self.composer_of(in_thread).filter(|_| !editing).and_then(|composer| {
      let state = composer.read(cx);
      let text = state.value();
      let (trigger, range) = completion::find(&text, state.cursor(), state.tokens())?;
      let query = text.get(range.start + 1..range.end)?.to_string();
      Some((trigger, range, query))
    });
    let Some((trigger, range, query)) = found else {
      if self.menu.take().is_some() {
        cx.notify();
      }
      return;
    };
    match trigger {
      Trigger::Mention if !self.asked_members => {
        self.asked_members = true;
        self.core.send(Command::LoadMembers(self.group.id.clone()));
      },
      Trigger::Command if !self.asked_commands => {
        self.asked_commands = true;
        self.core.send(Command::LoadSlashCommands(self.group.id.clone()));
      },
      _ => {},
    }
    let members = self.members.as_deref().map_or(&[][..], Vec::as_slice);
    let commands = self.commands.as_deref().map_or(&[][..], Vec::as_slice);
    let all = self.group.kind == GroupKind::Space;
    let items = completion::items(trigger, &query, members, all, commands);
    if items.is_empty() {
      self.menu = None;
    } else {
      match &mut self.menu {
        Some(menu) if menu.in_thread == in_thread => {
          if menu.range.start != range.start {
            menu.selected = 0;
          }
          menu.selected = menu.selected.min(items.len() - 1);
          menu.range = range;
          menu.items = items;
        },
        _ => {
          let view = cx.weak_entity();
          let keys = cx.intercept_keystrokes(move |event, window, cx| {
            view.update(cx, |this, cx| this.menu_key(event, window, cx)).ok();
          });
          self.menu = Some(Menu {
            in_thread,
            range,
            items,
            selected: 0,
            _keys: keys,
          });
        },
      }
    }
    cx.notify();
  }

  /// The menu's keys, taken before the composer sees them: arrows move, Return or Tab picks, Escape closes.
  fn menu_key(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
    let Some(in_thread) = self.menu.as_ref().map(|menu| menu.in_thread) else {
      return;
    };
    let focused = self
      .composer_of(in_thread)
      .is_some_and(|composer| composer.read(cx).focus_handle(cx).is_focused(window));
    let keystroke = &event.keystroke;
    let Some(menu) = self.menu.as_mut().filter(|_| focused && !keystroke.modifiers.modified()) else {
      return;
    };
    match keystroke.key.as_str() {
      "up" => menu.selected = menu.selected.checked_sub(1).unwrap_or(menu.items.len() - 1),
      "down" => menu.selected = (menu.selected + 1) % menu.items.len(),
      "enter" | "tab" => {
        let selected = menu.selected;
        self.pick(selected, window, cx);
      },
      "escape" => self.menu = None,
      _ => return,
    }
    cx.stop_propagation();
    cx.notify();
  }

  /// Puts the row's chip in place of the trigger and query, followed by a space.
  fn pick(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
    let Some(menu) = self.menu.take() else {
      return;
    };
    let (Some(item), Some(composer)) = (menu.items.get(ix), self.composer_of(menu.in_thread).cloned()) else {
      return;
    };
    let token = item.token();
    composer.update(cx, |state, cx| {
      if state.replace_range_with_token(menu.range, token, window, cx).is_ok() {
        state.insert(" ", window, cx);
      }
      state.focus(window, cx);
    });
    cx.notify();
  }

  fn composer_status(&self, in_thread: bool, cx: &App) -> Option<String> {
    if self.rewriting == Some(in_thread) {
      let name = cx.global::<Settings>().rewrite.label();
      return Some(format!("Rewriting with {name}..."));
    }
    if in_thread {
      return None;
    }
    self.rewrite_error.clone().or_else(|| self.typing_text())
  }

  fn typing_text(&self) -> Option<String> {
    match self.typing.as_slice() {
      [] => None,
      [(_, a, _)] => Some(format!("{a} is typing…")),
      [(_, a, _), (_, b, _)] => Some(format!("{a} and {b} are typing…")),
      _ => Some("Several people are typing…".into()),
    }
  }

  fn find_for(&self, id: &str) -> (Option<&str>, Option<Range<usize>>) {
    let Some(find) = &self.find else {
      return (None, None);
    };
    let query = (!find.query.is_empty()).then_some(find.query.as_str());
    let hit = find
      .hits
      .get(find.index)
      .and_then(|(hit_id, range)| (hit_id == id).then(|| range.clone()));
    (query, hit)
  }

  fn render_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
    let Some(id) = self.rows.get(ix).map(|row| row.msg.id.clone()) else {
      return div().into_any_element();
    };
    let (find, find_hit) = self.find_for(&id);
    let Some(row) = self.rows.get(ix) else {
      return div().into_any_element();
    };
    let ctx = MsgCtx {
      images: &self.images,
      core: &self.core,
      me_avatar: self.me_avatar.as_deref(),
      open_thread: Some(&self.opener),
      focus: self.focus.as_deref(),
      find,
      find_hit,
      act: &self.actor,
      open_person: &self.open_person,
      threads: self.group.kind == GroupKind::Space,
      hovered: self.hovered.as_deref(),
      on_tools: &self.on_tools,
    };
    message::render_row(row, &ctx, cx)
  }

  fn render_thread_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
    let Some(id) = self.thread.as_ref().and_then(|t| t.rows.get(ix).map(|row| row.msg.id.clone())) else {
      return div().into_any_element();
    };
    let (find, find_hit) = self.find_for(&id);
    let Some(row) = self.thread.as_ref().and_then(|t| t.rows.get(ix)) else {
      return div().into_any_element();
    };
    let ctx = MsgCtx {
      images: &self.images,
      core: &self.core,
      me_avatar: self.me_avatar.as_deref(),
      open_thread: None,
      focus: self.focus.as_deref(),
      find,
      find_hit,
      act: &self.actor,
      open_person: &self.open_person,
      threads: false,
      hovered: self.hovered.as_deref(),
      on_tools: &self.on_tools,
    };
    message::render_row(row, &ctx, cx)
  }

  fn render_thread_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
    let thread = self.thread.as_ref()?;
    let header = h_flex()
      .h(px(56.))
      .flex_shrink_0()
      .px_4()
      .items_center()
      .justify_between()
      .border_b_1()
      .border_color(rgb(BORDER))
      .child(
        div()
          .text_size(px(18.))
          .font_weight(FontWeight::MEDIUM)
          .text_color(rgb(TEXT))
          .child("Thread"),
      )
      .child(
        div()
          .id("thread-close")
          .size(px(32.))
          .flex()
          .items_center()
          .justify_center()
          .rounded_full()
          .cursor_pointer()
          .hover(|s| s.bg(rgb(ROW_HOVER)))
          .on_click(cx.listener(|this, _, window, cx| {
            this.close_thread(window, cx);
          }))
          .child(icon(Icon::X, 18., TEXT)),
      );
    let body = div()
      .id("thread-body")
      .on_hover(cx.listener(|this, hovered: &bool, window, cx| this.list_hovered(*hovered, window, cx)))
      .flex_1()
      .min_h_0()
      .w_full()
      .child(
        list(
          thread.list.clone(),
          cx.processor(|this, ix: usize, _, cx| this.render_thread_row(ix, cx)),
        )
        .size_full(),
      );
    let composer = v_flex()
      .w_full()
      .flex_shrink_0()
      .px_4()
      .pt_2()
      .pb_4()
      .child(
        div()
          .w_full()
          .h(px(20.))
          .px_3()
          .text_size(px(12.))
          .text_color(rgb(if self.rewriting == Some(true) {
            ACCENT
          } else {
            MUTED
          }))
          .children(self.composer_status(true, cx)),
      )
      .children(self.draft_banner(true, cx))
      .children(self.attached(true, cx))
      .child(self.composer_field(&thread.composer, true, cx));
    Some(
      v_flex()
        .w(relative(0.45))
        .h_full()
        .flex_shrink_0()
        .border_l_1()
        .border_color(rgb(BORDER))
        // Files dropped on the thread go out with the next reply.
        .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(ROW_HOVER)))
        .on_drop(
          cx.listener(|this, dropped: &ExternalPaths, window, cx| this.attach(true, dropped.paths(), window, cx)),
        )
        .child(header)
        .child(body)
        .child(composer)
        .into_any_element(),
    )
  }
}

impl Drop for ConversationView {
  fn drop(&mut self) {
    self.stop_typing();
  }
}

impl ConversationView {
  /// Creates a Google Meet meeting and posts its link here.
  fn start_meet(&mut self, cx: &mut Context<Self>) {
    self.core.send(Command::StartMeet(self.group.id.clone()));
    self.list.scroll_to_end();
    cx.notify();
  }
}

impl Render for ConversationView {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    if self.refocus {
      self.refocus = false;
      let composer = self.composer.clone();
      window.defer(cx, move |window, cx| composer.update(cx, |state, cx| state.focus(window, cx)));
    }
    let header = h_flex()
      .h(px(56.))
      .flex_shrink_0()
      .px_3()
      .gap_3()
      .items_center()
      .border_b_1()
      .border_color(rgb(BORDER))
      // Messages and Rail keep the list beside the conversation: nothing to go back to.
      .when(!cx.global::<Settings>().layout.keeps_list(), |header| {
        header.child(
          div()
            .id("back")
            .size(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(ROW_HOVER)))
            .on_click(cx.listener(|this, _, window, cx| this.go_back(window, cx)))
            .child(icon(Icon::ArrowLeft, 18., TEXT)),
        )
      })
      .child(group_avatar(&self.images, cx, &self.group, 32.))
      .child(
        div()
          .min_w_0()
          .truncate()
          .text_size(px(18.))
          .font_weight(FontWeight::MEDIUM)
          .text_color(rgb(TEXT))
          .child(self.group.name.clone()),
      )
      .child(div().flex_1())
      .child(
        div()
          .id("meet")
          .h(px(36.))
          .px_4()
          .flex()
          .items_center()
          .justify_center()
          .rounded_full()
          .bg(rgb(FIELD))
          .cursor_pointer()
          .hover(|s| s.bg(rgb(ROW_SELECTED)))
          .on_click(cx.listener(|this, _, _, cx| this.start_meet(cx)))
          .child(icon(Icon::Phone, 17., TEXT)),
      );

    let body = if self.rows.is_empty() {
      div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(MUTED))
        .child(if self.loaded {
          "No messages yet"
        } else {
          "Loading messages…"
        })
        .into_any_element()
    } else {
      div()
        .id("body")
        .on_hover(cx.listener(|this, hovered: &bool, window, cx| this.list_hovered(*hovered, window, cx)))
        .flex_1()
        .min_h_0()
        .w_full()
        .child(
          list(
            self.list.clone(),
            cx.processor(|this, ix: usize, _, cx| this.render_row(ix, cx)),
          )
          .size_full(),
        )
        .into_any_element()
    };

    let composer = v_flex()
      .w_full()
      .flex_shrink_0()
      .items_center()
      .px_4()
      .pb_4()
      .child(
        div()
          .w_full()
          .max_w(px(780.))
          .h(px(20.))
          .px_3()
          .text_size(px(12.))
          .text_color(rgb(if self.rewriting == Some(false) {
            ACCENT
          } else {
            MUTED
          }))
          .children(self.composer_status(false, cx)),
      )
      .children(self.draft_banner(false, cx))
      .children(self.attached(false, cx))
      .child(self.composer_field(&self.composer, false, cx));

    let latest = (!self.load.at_end.get()).then(|| {
      div().w_full().flex().justify_center().pb_1().child(
        div()
          .id("jump-latest")
          .px_3()
          .py_1()
          .rounded_full()
          .bg(rgb(ACCENT_BG))
          .text_size(px(12.))
          .text_color(rgb(TEXT))
          .cursor_pointer()
          .on_click(cx.listener(|this, _, _, cx| this.jump_latest(cx)))
          .child("Jump to latest"),
      )
    });
    let main = v_flex()
      .relative()
      .flex_1()
      .min_w_0()
      .h_full()
      // Files dropped anywhere on the conversation go out with the next message.
      .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(ROW_HOVER)))
      .on_drop(cx.listener(|this, dropped: &ExternalPaths, window, cx| this.attach(false, dropped.paths(), window, cx)))
      .child(header)
      .child(body)
      .children(latest)
      .child(composer)
      .children(self.find_bar(cx));
    h_flex()
      .size_full()
      // Before the composer sees the paste, so files never land in it as text.
      .capture_action(cx.listener(Self::paste))
      .child(main)
      .children(self.render_thread_panel(cx))
  }
}

/// A file the app can show a small picture of beside its name.
fn is_image(path: &Path) -> bool {
  let extension = path.extension().map(|extension| extension.to_string_lossy().to_lowercase());
  matches!(extension.as_deref(), Some("png" | "jpg" | "jpeg" | "gif" | "webp"))
}

/// Where pictures pasted from the clipboard are kept until they are sent. Emptied when the app starts.
pub fn pasted_dir() -> PathBuf {
  std::env::temp_dir().join("working-pasted")
}

/// Writes a picture from the clipboard to a file, so it attaches like any other. Formats Google Chat does not
/// show inline become PNG.
fn save_pasted(picture: &Image) -> Option<PathBuf> {
  let (bytes, extension) = match picture.format {
    ImageFormat::Png => (picture.bytes.clone(), "png"),
    ImageFormat::Jpeg => (picture.bytes.clone(), "jpg"),
    ImageFormat::Webp => (picture.bytes.clone(), "webp"),
    ImageFormat::Gif => (picture.bytes.clone(), "gif"),
    _ => {
      let mut png = std::io::Cursor::new(Vec::new());
      image::load_from_memory(&picture.bytes)
        .ok()?
        .write_to(&mut png, image::ImageFormat::Png)
        .ok()?;
      (png.into_inner(), "png")
    },
  };
  let dir = pasted_dir();
  std::fs::create_dir_all(&dir).ok()?;
  let path = dir.join(format!(
    "Pasted image {}.{extension}",
    chrono::Local::now().format("%Y-%m-%d at %H.%M.%S%.3f")
  ));
  std::fs::write(&path, bytes).ok()?;
  Some(path)
}

/// Replaces `old` with `new`, telling the list only about the changed middle so heights and scroll stay put.
fn apply_rows(list: &ListState, old: &mut Vec<Row>, new: Vec<Row>, prepended: usize) {
  let (old_len, new_len) = (old.len(), new.len());
  let mut prefix = 0;
  while prefix < old_len.min(new_len) && old[prefix].same(&new[prefix]) {
    prefix += 1;
  }
  let mut suffix = 0;
  while suffix < old_len - prefix
    && suffix < new_len - prefix
    && old[old_len - 1 - suffix].same(&new[new_len - 1 - suffix])
  {
    suffix += 1;
  }
  let top = list.logical_scroll_top();
  let anchored = top.item_ix < old_len;
  if prefix != old_len || prefix != new_len {
    list.splice(prefix..old_len - suffix, new_len - prefix - suffix);
  }
  *old = new;
  if prepended > 0 && anchored && new_len > old_len {
    list.scroll_to(ListOffset {
      item_ix: top.item_ix + (new_len - old_len),
      offset_in_item: top.offset_in_item,
    });
  }
}

#[cfg(test)]
mod tests {
  use super::is_image;
  use std::path::Path;

  #[test]
  fn pasted_images_by_extension() {
    assert!(is_image(Path::new("shot.PNG")));
    assert!(is_image(Path::new("a.jpeg")));
    assert!(!is_image(Path::new("notes.txt")));
    assert!(!is_image(Path::new("noext")));
  }
}
