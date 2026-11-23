//! An open conversation: header, bottom-anchored virtualized message list, composer.

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gchat_core::{Command, Core, Group, GroupKind, Message, Micros, ReadReceipt, SlashCommand, User};
use gpui_kit::component::input::{Input, InputEvent, InputState, Paste, RopeExt as _, Textarea, TextareaState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{FocusTrapElement as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::completion::{self, Member, Menu, Trigger};
use crate::conv_list::{buffer_name, hint};
use crate::emoji_font;
use crate::format_logic::Format;
use crate::images::{self, Images};
use crate::message::{self, Act, Actor, MsgCtx, Opener, Readers, Row};
use crate::settings::{Layout, MessageStyle, RewriteTool, Settings, SidebarSide, ThreadView};
use crate::store::{History, Store};
use crate::text_panel::TextPanelHandle;
use crate::theme::{
  ACCENT, ACCENT_BG, BORDER, FIELD, MUTED, PANEL, PLACEHOLDER, POPOVER, ROW_HOVER, ROW_SELECTED, TEXT, mono, rgb,
};
use crate::ui::{Icon, group_avatar, icon};

const TYPING_IDLE: Duration = Duration::from_secs(5);
const TYPING_EXPIRY: Duration = Duration::from_secs(8);
/// IRC: the one-line title bar, the nick list's width, and one nick's line.
const IRC_BAR_HEIGHT: f32 = 32.0;
const NICK_LIST_WIDTH: f32 = 150.0;
const NICK_ROW_HEIGHT: f32 = 22.0;
/// The least and the most the thread pop-up measures as the window grows.
const THREAD_POPUP: (Size<f32>, Size<f32>) =
  (Size { width: 640.0, height: 420.0 }, Size { width: 1040.0, height: 1200.0 });
/// The room a modal leaves to each edge of the window.
const MODAL_MARGIN: f32 = 16.0;

actions!(working, [FormatBold, FormatItalic, FormatStrikethrough, FormatBulletedList]);

/// One button of the formatting bar.
struct FormatButton {
  format: Format,
  glyph: Icon,
  id: &'static str,
  /// The name its tooltip shows, before the key when it has one.
  name: &'static str,
  /// The key, as a binding (`secondary-` is Cmd on macOS and Ctrl elsewhere); the bulleted list's is spelled with
  /// the 8 the user presses rather than the `*` it is bound as.
  keys: Option<&'static str>,
}

/// The formatting bar's buttons, in groups with a divider between them: styles, the list, and code.
const FORMAT_BAR: [&[FormatButton]; 3] = [
  &[
    FormatButton {
      format: Format::Bold,
      glyph: Icon::Bold,
      id: "format-bold",
      name: "Bold",
      keys: Some("secondary-b"),
    },
    FormatButton {
      format: Format::Italic,
      glyph: Icon::Italic,
      id: "format-italic",
      name: "Italic",
      keys: Some("secondary-i"),
    },
    FormatButton {
      format: Format::Strikethrough,
      glyph: Icon::Strikethrough,
      id: "format-strikethrough",
      name: "Strikethrough",
      keys: Some("secondary-shift-x"),
    },
  ],
  &[FormatButton {
    format: Format::Bullets,
    glyph: Icon::List,
    id: "format-bullets",
    name: "Bulleted list",
    keys: Some("secondary-shift-8"),
  }],
  &[
    FormatButton {
      format: Format::Code,
      glyph: Icon::Code,
      id: "format-code",
      name: "Code",
      keys: None,
    },
    FormatButton {
      format: Format::CodeBlock,
      glyph: Icon::SquareCode,
      id: "format-code-block",
      name: "Code block",
      keys: None,
    },
  ],
];

/// Paging state and the messages below the fold.
struct LoadState {
  loading: Cell<bool>,
  reached: Cell<bool>,
  /// The last load added nothing: do not retry until the user scrolls away and back.
  stalled: Cell<bool>,
  /// False while the list is a window around a search hit: newer pages load at the bottom.
  at_end: Cell<bool>,
  loading_newer: Cell<bool>,
  /// Messages of someone else that came at the end while the list was scrolled up from it, which the pill over the
  /// bottom of the list counts until the list is at its end again.
  unseen: Cell<usize>,
}

/// The open thread panel: its rows (starting message first, then replies) are rebuilt with the message list.
struct Thread {
  topic: String,
  /// The rows, drawn as a cached view of their own.
  view: Entity<MessageList>,
  composer: Entity<TextareaState>,
  /// Files that go out with the next reply.
  files: Vec<Attached>,
  _sub: Subscription,
}

/// A file that goes out with the next message, with a small picture of it once decoded when it is one.
struct Attached {
  path: PathBuf,
  image: bool,
  thumb: Option<Arc<RenderImage>>,
}

/// The longer side, in pixels, of an attached picture's small picture beside its name.
const CHIP_THUMB: u32 = 128;

/// The message whose text is in a composer to be changed, and which composer that is.
struct Editing {
  message_id: String,
  in_thread: bool,
}

struct Find {
  input: Entity<InputState>,
  query: String,
  hits: Vec<(String, Range<usize>)>,
  /// The same hits by message, as the rows draw them.
  by_message: Rc<HashMap<String, Vec<Range<usize>>>>,
  index: usize,
  _sub: Subscription,
}

/// What the rows of both lists show besides their messages: the toolbar, the outline of a search hit, the find bar's
/// hits. The conversation hands a new one to its lists whenever it changes (`ConversationView::sync_marks`).
#[derive(Clone, Default)]
struct Marks {
  hovered: Option<String>,
  focus: Option<String>,
  /// The find bar's hits by message, and the one it is on.
  find: Option<(Rc<HashMap<String, Vec<Range<usize>>>>, Option<(String, Range<usize>)>)>,
}

/// What both message lists draw with, shared with the conversation.
#[derive(Clone)]
struct ListParts {
  images: Images,
  core: Core,
  actor: Actor,
  open_person: Opener,
  on_tools: Rc<Cell<bool>>,
  me_avatar: Option<String>,
}

/// A message list as a view of its own, which the conversation draws cached: typing in a composer, the `@` menu and
/// the typing line redraw the conversation without laying out these rows again. Whatever changes what the rows show
/// (rows, receipts, marks, images, settings) notifies this view, and it redraws each minute for the times its rows
/// show (`now`, `5 min`).
struct MessageList {
  rows: Vec<Row>,
  list: ListState,
  /// The IRC style's sender column for those rows (`message::irc_nick_width`).
  nick_width: Cell<(usize, f32)>,
  /// Who has read up to each message.
  readers: Readers,
  /// The New line: the message it sits above, the first the user had not read when the conversation opened, and
  /// whether a message past it mentions them.
  new_line: Option<(SharedString, bool)>,
  marks: Marks,
  parts: ListParts,
  /// The main list's thread footers open the thread; the thread panel has none.
  opener: Option<Opener>,
  /// "Reply in thread" is offered: the main list of a space.
  threads: bool,
  /// The main list of a conversation with a thread open: under the thread pop-up it shows no toolbar.
  under_thread: bool,
  /// The message layout the list was measured with.
  density: MessageStyle,
  _subs: [Subscription; 2],
  _clock: Task<()>,
}

impl MessageList {
  fn new(rows: Vec<Row>, parts: ListParts, opener: Option<Opener>, threads: bool, cx: &mut Context<Self>) -> Self {
    let subs = [
      cx.observe(parts.images.tick(), |_, _, cx| cx.notify()),
      // Rows change height with the message layout: measure the list again.
      cx.observe_global::<Settings>(|this, cx| {
        let density = cx.global::<Settings>().message_style;
        if density != this.density {
          this.density = density;
          this.list.reset(this.rows.len());
        }
        cx.notify();
      }),
    ];
    Self {
      list: ListState::new(rows.len(), ListAlignment::Bottom, px(600.)),
      rows,
      nick_width: Cell::default(),
      readers: Readers::default(),
      new_line: None,
      marks: Marks::default(),
      parts,
      opener,
      threads,
      under_thread: false,
      density: cx.global::<Settings>().message_style,
      _subs: subs,
      _clock: crate::home::minute_clock(cx),
    }
  }

  /// Replaces the rows, telling the list only about the changed middle so heights and scroll stay put. Rows that stay
  /// keep what they already worked out (`Row::adopt`).
  fn set_rows(&mut self, mut new: Vec<Row>, prepended: usize, receipts: &[ReadReceipt], cx: &mut Context<Self>) {
    // The message under the New line starts a run: its sender shows, whoever wrote the one above it.
    if let Some((id, _)) = &self.new_line
      && let Some(row) = new.iter_mut().find(|row| row.id == *id)
    {
      row.head = true;
    }
    let old = &mut self.rows;
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
    for (new, old) in new.iter_mut().zip(old.iter_mut()).take(prefix) {
      new.adopt(old);
    }
    for (new, old) in new.iter_mut().rev().zip(old.iter_mut().rev()).take(suffix) {
      new.adopt(old);
    }
    let top = self.list.logical_scroll_top();
    let anchored = top.item_ix < old_len;
    if prefix != old_len || prefix != new_len {
      self.list.splice(prefix..old_len - suffix, new_len - prefix - suffix);
    }
    *old = new;
    if prepended > 0 && anchored && new_len > old_len {
      self.list.scroll_to(ListOffset {
        item_ix: top.item_ix + (new_len - old_len),
        offset_in_item: top.offset_in_item,
      });
    }
    self.readers = Readers::place(&self.rows, receipts);
    cx.notify();
  }

  fn set_readers(&mut self, receipts: &[ReadReceipt], cx: &mut Context<Self>) {
    self.readers = Readers::place(&self.rows, receipts);
    cx.notify();
  }

  fn set_marks(&mut self, marks: Marks, cx: &mut Context<Self>) {
    self.marks = marks;
    cx.notify();
  }

  /// Puts the New line above a message, or takes it away. The rows it leaves and comes to are measured again, their
  /// heights having changed, and the row it comes to starts a run.
  fn set_new_line(&mut self, line: Option<(SharedString, bool)>, cx: &mut Context<Self>) {
    if line == self.new_line {
      return;
    }
    let old = std::mem::replace(&mut self.new_line, line);
    let under = self.new_line.as_ref().map(|(id, _)| id.clone());
    for id in old.iter().map(|(id, _)| id).chain(&under) {
      if let Some(ix) = self.rows.iter().position(|row| row.id == *id) {
        self.list.splice(ix..ix + 1, 1);
      }
    }
    if let Some(row) = under.and_then(|id| self.rows.iter_mut().find(|row| row.id == id)) {
      row.head = true;
    }
    cx.notify();
  }

  /// The row the New line sits above.
  fn new_line_row(&self) -> Option<usize> {
    let (id, _) = self.new_line.as_ref()?;
    self.rows.iter().position(|row| row.id == *id)
  }

  /// Whether the list shows its first row: the first row is in view, or the whole list fits.
  fn at_top(&self) -> bool {
    self.list.logical_scroll_top().item_ix == 0 || self.list.max_offset_for_scrollbar().y <= px(0.)
  }

  /// Whether the list shows the end of its last row. A list left at its end keeps no scroll position of its own;
  /// scrolled, it is at its end while the last row ends inside the viewport. `ListState::is_scrolled_to_end` cannot
  /// say for a long conversation, whose rows above the ones drawn have no height yet.
  fn at_end(&self) -> bool {
    let count = self.list.item_count();
    let Some(last) = count.checked_sub(1) else {
      return true;
    };
    if self.list.logical_scroll_top().item_ix >= count {
      return true;
    }
    let viewport = self.list.viewport_bounds();
    self
      .list
      .bounds_for_item(last)
      .is_some_and(|row| row.bottom() <= viewport.bottom() + px(1.))
  }

  /// Moves the list's scroll position, and redraws it there.
  fn scroll(&mut self, cx: &mut Context<Self>, to: Option<ListOffset>) {
    match to {
      Some(offset) => self.list.scroll_to(offset),
      None => self.list.scroll_to_end(),
    }
    cx.notify();
  }

  fn render_row(&self, ix: usize, window: &Window, cx: &App) -> AnyElement {
    let Some(row) = self.rows.get(ix) else {
      return div().into_any_element();
    };
    let id = row.msg.id.as_str();
    let (hits, find_hit) = match &self.marks.find {
      Some((by_message, current)) => (
        by_message.get(id).map_or(&[][..], Vec::as_slice),
        current.as_ref().filter(|(hit, _)| hit == id).map(|(_, range)| range.clone()),
      ),
      None => (&[][..], None),
    };
    let under_popup = self.under_thread && cx.global::<Settings>().thread_view == ThreadView::Modal;
    let ctx = MsgCtx {
      images: &self.parts.images,
      core: &self.parts.core,
      me_avatar: self.parts.me_avatar.as_deref(),
      open_thread: self.opener.as_ref(),
      focus: self.marks.focus.as_deref(),
      find: hits,
      find_hit,
      act: &self.parts.actor,
      open_person: &self.parts.open_person,
      threads: self.threads,
      // Under the thread pop-up the list shows no toolbar: the pop-up's own row has the same message.
      hovered: self.marks.hovered.as_deref().filter(|_| !under_popup),
      on_tools: &self.parts.on_tools,
      readers: self.readers.of(id),
      nick_width: message::irc_nick_width(&self.rows, &self.nick_width),
      new_line: self
        .new_line
        .as_ref()
        .filter(|(line, _)| *line == row.id)
        .map(|(_, mention)| *mention),
      scale: window.scale_factor(),
    };
    message::render_row(row, &ctx, cx)
  }
}

impl Render for MessageList {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    list(
      self.list.clone(),
      cx.processor(|this, ix: usize, window, cx| this.render_row(ix, window, cx)),
    )
    .size_full()
  }
}

/// The list inside `view`, scrolled to `to` (`None`: the end) and redrawn there.
fn scroll_list(view: &Entity<MessageList>, to: Option<ListOffset>, cx: &mut App) {
  view.update(cx, |list, cx| list.scroll(cx, to));
}

pub struct ConversationView {
  store: WeakEntity<Store>,
  core: Core,
  images: Images,
  group: Group,
  me_id: String,
  me_avatar: Option<String>,
  messages: Arc<Vec<Arc<Message>>>,
  /// The last message of the list `take_messages` handed back, until `set_history` brings the next list.
  taken_last: Option<Arc<Message>>,
  /// The main list, drawn as a cached view of its own.
  main: Entity<MessageList>,
  /// Who has read the conversation, as the core last said: none while Google Chat has read receipts off here.
  receipts: Arc<Vec<ReadReceipt>>,
  thread: Option<Thread>,
  /// Opens the chat with the person a name or a mention names.
  open_person: Opener,
  actor: Actor,
  /// The message the next one sent from the main composer quotes.
  quoting: Option<Arc<Message>>,
  editing: Option<Editing>,
  /// Files that go out with the next message from the main composer.
  files: Vec<Attached>,
  /// The message under the pointer, which shows its toolbar.
  hovered: Option<String>,
  /// The pointer is over a message list.
  in_list: bool,
  on_tools: Rc<Cell<bool>>,
  /// The thread panel vanished while its composer had focus: give focus back to the main composer.
  refocus: bool,
  /// The press on the thread pop-up's backdrop also closed the text panel: that press leaves the thread open.
  backdrop_kept: bool,
  /// Keeps Tab inside the thread pop-up, whose composer has the keyboard once it opens.
  thread_focus: FocusHandle,
  loaded: bool,
  composer: Entity<TextareaState>,
  load: LoadState,
  typing_sent: bool,
  typing_task: Option<Task<()>>,
  typing: Vec<(String, String, Instant)>,
  _subs: Vec<Subscription>,
  /// Drops typing people who went quiet; runs only while someone is typing (`expiring`).
  _expiry: Option<Task<()>>,
  expiring: bool,
  _top_check: Option<Task<()>>,
  /// The message a search hit jumped to, outlined for a moment.
  focus: Option<String>,
  _focus_clear: Option<Task<()>>,
  /// The user asked for the latest messages: scroll to the end when they arrive.
  to_latest: bool,
  /// The conversation's read time as it was when it opened unread: what the New line of each list is placed by, and
  /// what thread replies count as new from (`thread_seen`). None when it opened read, and once the user writes in it.
  read_at_open: Option<Micros>,
  /// When the conversation opened or the user last sent a message in it, whichever is later: what thread replies
  /// count as new from once `read_at_open` is gone.
  caught_up: Micros,
  /// The last time each thread's panel was on screen, by topic: its replies up to then are read.
  threads_seen: HashMap<String, Micros>,
  /// The main list's New line is still to be placed: the messages it is placed among have not all come.
  placing_new_line: bool,
  rewrite_task: Option<Task<()>>,
  /// Which composer is being rewritten: `Some(false)` main, `Some(true)` thread.
  rewriting: Option<bool>,
  rewrite_error: Option<String>,
  _rewrite_error_clear: Option<Task<()>>,
  find: Option<Find>,
  /// Who `@` offers, once `Command::LoadMembers` answered.
  members: Option<Vec<Member>>,
  /// The members A to Z, for IRC's nick list.
  nicks: Vec<User>,
  /// What `/` offers, once `Command::LoadSlashCommands` answered.
  commands: Option<Arc<Vec<SlashCommand>>>,
  /// Members and commands are asked for on the first `@` or `/`, once; IRC asks for a space's members on opening.
  asked_members: bool,
  asked_commands: bool,
  /// The `@` or `/` menu over one of the composers.
  menu: Option<Menu>,
  /// The chat is still being created (`Event::ChatStarting`): the core does not know it yet, so nothing is sent
  /// for it until `started`.
  starting: bool,
  /// Send was asked for while the chat was starting: the composer keeps the message, and it goes out once it started.
  send_held: bool,
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
    let me_id = me.as_ref().map_or_default(|u| u.id.clone());
    let me_avatar = me.and_then(|u| u.avatar_url);
    let read_at_open = group.unread.then_some(group.last_read_time);
    let caught_up = crate::calendar_layout::now_micros();
    // No thread has been on screen yet: every thread's replies count from the conversation's own time.
    let rows = message::build_rows(&messages, &me_id, |_| Some(read_at_open.unwrap_or(caught_up)));
    let load = LoadState {
      loading: Cell::new(false),
      reached: Cell::new(reached),
      stalled: Cell::new(false),
      at_end: Cell::new(true),
      loading_newer: Cell::new(false),
      unseen: Cell::new(0),
    };

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
    let on_tools = Rc::new(Cell::new(false));
    let parts = ListParts {
      images: images.clone(),
      core: core.clone(),
      actor: actor.clone(),
      open_person: open_person.clone(),
      on_tools: on_tools.clone(),
      me_avatar: me_avatar.clone(),
    };
    let threads = group.kind == GroupKind::Space;
    let main = cx.new(|cx| MessageList::new(rows, parts, Some(opener), threads, cx));
    {
      let weak = cx.weak_entity();
      // The event's range is the one the scroll started from, and the list's state is busy while this runs: once the
      // scroll is over, see where the list landed.
      main.read(cx).list.set_scroll_handler(move |_, _, cx| {
        let weak = weak.clone();
        cx.defer(move |cx| {
          weak.update(cx, |this, cx| this.check_scroll(cx)).ok();
        });
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

    let mut view = Self {
      store,
      core,
      images,
      group,
      me_id,
      me_avatar,
      messages,
      taken_last: None,
      main,
      receipts: Arc::default(),
      thread: None,
      open_person,
      actor,
      quoting: None,
      editing: None,
      files: Vec::new(),
      hovered: None,
      in_list: false,
      on_tools,
      refocus: false,
      backdrop_kept: false,
      thread_focus: cx.focus_handle(),
      loaded,
      composer,
      load,
      typing_sent: false,
      typing_task: None,
      typing: Vec::new(),
      _subs: subs,
      _expiry: None,
      expiring: false,
      _top_check: None,
      focus: None,
      _focus_clear: None,
      to_latest: false,
      read_at_open,
      caught_up,
      threads_seen: HashMap::new(),
      placing_new_line: read_at_open.is_some(),
      rewrite_task: None,
      rewriting: None,
      rewrite_error: None,
      _rewrite_error_clear: None,
      find: None,
      members: None,
      nicks: Vec::new(),
      commands: None,
      asked_members: false,
      asked_commands: false,
      menu: None,
      starting: false,
      send_held: false,
    };
    // Picking IRC lists a space's members.
    view._subs.push(cx.observe_global::<Settings>(|this, cx| {
      this.want_members(cx);
      cx.notify();
    }));
    // What the view draws from outside it: the header's and the `@` menu's pictures, and the header's presence dot.
    view._subs.push(cx.observe(view.images.tick(), |_, _, cx| cx.notify()));
    view._subs.push(cx.observe_global::<crate::ui::People>(|_, cx| cx.notify()));
    view.want_members(cx);
    view.place_new_line(false, true, cx);
    view.schedule_top_check(cx);
    view
  }

  pub fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.composer.update(cx, |state, cx| state.focus(window, cx));
  }

  pub fn set_group(&mut self, group: Group, cx: &mut Context<Self>) {
    let threads = group.kind == GroupKind::Space;
    self.group = group;
    self.main.update(cx, |list, cx| {
      list.threads = threads;
      cx.notify();
    });
    cx.notify();
  }

  /// Marks the view as one of a chat that is still being created.
  pub const fn hold(&mut self) {
    self.starting = true;
  }

  /// The chat the view was showing while it was created now exists, as `group`: what was held back goes out.
  pub fn started(&mut self, group: Group, window: &mut Window, cx: &mut Context<Self>) {
    self.starting = false;
    // Whatever was asked while it was starting was never sent.
    self.typing_sent = false;
    self.asked_members = false;
    self.asked_commands = false;
    self.set_group(group, cx);
    if std::mem::take(&mut self.send_held) {
      self.send(window, cx);
    }
  }

  /// Sends a command about this conversation, unless the chat is still being created.
  fn command(&self, command: Command) {
    if !self.starting {
      self.core.send(command);
    }
  }

  pub fn set_me(&mut self, me: User, cx: &mut Context<Self>) {
    self.me_avatar = me.avatar_url;
    self.me_id = me.id;
    let avatar = self.me_avatar.clone();
    for view in self.lists() {
      let avatar = avatar.clone();
      view.update(cx, |list, _| list.parts.me_avatar = avatar);
    }
    self.rebuild(0, cx);
    self.place_new_line(false, true, cx);
    cx.notify();
  }

  /// The main list, then the thread panel's when a thread is open.
  fn lists(&self) -> impl Iterator<Item = Entity<MessageList>> + use<> {
    std::iter::once(self.main.clone()).chain(self.thread.as_ref().map(|thread| thread.view.clone()))
  }

  /// Places the main list's New line, once: above the first message the user had not read when the conversation
  /// opened (`message::new_line`), where the list then goes with `scroll`. Until the user is known and the messages
  /// have come there is nothing to place it by. `settled` says the messages are the core's latest: with none of them
  /// past the read time, the line of a conversation marked unread by hand goes above the newest message of someone
  /// else, and with none of those the placing ends without a line. Placed, the line stays where it is, whatever comes.
  fn place_new_line(&mut self, settled: bool, scroll: bool, cx: &mut Context<Self>) {
    if !self.placing_new_line || !self.loaded || self.me_id.is_empty() {
      return;
    }
    let line = self
      .read_at_open
      .and_then(|read| message::new_line(&self.messages, &self.main.read(cx).rows, &self.me_id, read, settled));
    self.placing_new_line = line.is_none() && !settled;
    if line.is_none() {
      return;
    }
    self.main.update(cx, |list, cx| {
      list.set_new_line(line, cx);
      if scroll && let Some(ix) = list.new_line_row() {
        list.scroll(cx, Some(ListOffset { item_ix: ix, offset_in_item: px(0.) }));
      }
    });
  }

  /// Takes the New line off both lists: the user wrote, so everything before is read.
  fn clear_new_line(&mut self, cx: &mut Context<Self>) {
    self.read_at_open = None;
    self.placing_new_line = false;
    for view in self.lists() {
      view.update(cx, |list, cx| list.set_new_line(None, cx));
    }
  }

  /// Hands what the rows show besides their messages (`Marks`) to both lists.
  fn sync_marks(&self, cx: &mut App) {
    let marks = Marks {
      hovered: self.hovered.clone(),
      focus: self.focus.clone(),
      find: self
        .find
        .as_ref()
        .filter(|find| !find.query.is_empty())
        .map(|find| (find.by_message.clone(), find.hits.get(find.index).cloned())),
    };
    for view in self.lists() {
      let marks = marks.clone();
      view.update(cx, |list, cx| list.set_marks(marks, cx));
    }
  }

  /// Takes who has read the conversation, and whether Google Chat has read receipts on in it: with them off, nobody's
  /// avatar shows. Each avatar sits on the last message its person has read, in the main list and in the thread panel,
  /// over the rows rather than in them, so people reading on never measure the lists again.
  pub fn set_readers(&mut self, enabled: bool, receipts: Arc<Vec<ReadReceipt>>, cx: &mut Context<Self>) {
    self.receipts = if enabled { receipts } else { Arc::default() };
    for view in self.lists() {
      let receipts = self.receipts.clone();
      view.update(cx, |list, cx| list.set_readers(&receipts, cx));
    }
    cx.notify();
  }

  /// Hands the message list back and leaves an empty one, so the store can change the list without copying it before
  /// it calls `set_history` with the result. The rows stay as they are until then.
  pub fn take_messages(&mut self) -> Arc<Vec<Arc<Message>>> {
    self.taken_last = self.messages.last().cloned().or_else(|| self.taken_last.take());
    std::mem::take(&mut self.messages)
  }

  /// Takes the full message list. Returns true when a message from someone else arrived at the end. Those that arrive
  /// while the main list is scrolled up from its end are counted for its pill (`LoadState::unseen`).
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
    // The last message before this list: after `take_messages`, the one it kept.
    let previous = self.messages.last().cloned().or_else(|| self.taken_last.take());
    self.taken_last = None;
    let incoming = match (messages.last(), &previous) {
      (Some(new), Some(old)) => reached_end && new.id != old.id && !new.pending && new.sender_id != self.me_id,
      _ => false,
    };
    // Every message of someone else after the last one on screen counts, whichever message ends the list. Asked of the
    // rows still on screen, before they are replaced.
    if reached_end
      && self.load.at_end.get()
      && let Some(old) = &previous
      && !self.main.read(cx).at_end()
    {
      let arrived = message::arrived(&messages, &old.id, &self.me_id);
      self.load.unseen.set(self.load.unseen.get().saturating_add(arrived));
    }
    self.messages = messages;
    self.loaded = true;
    self.rebuild(prepended, cx);

    if self.load.loading.get() {
      self.load.stalled.set(prepended == 0 && !reached_start);
      self.load.loading.set(false);
    }
    self.load.reached.set(reached_start);
    self.load.at_end.set(reached_end);
    self.load.loading_newer.set(false);
    self.place_new_line(reached_end, focus.is_none(), cx);
    if let Some(id) = focus {
      self.focus_message(id, window, cx);
    } else if reached_end && self.to_latest {
      self.to_latest = false;
      scroll_list(&self.main, None, cx);
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
    let ix = self.main.read(cx).rows.iter().position(|row| row.msg.id == top_level);
    if let Some(ix) = ix {
      // The hit's own row: one row above can be a tall image that pushes the hit below the fold.
      scroll_list(&self.main, Some(ListOffset { item_ix: ix, offset_in_item: px(0.) }), cx);
    }
    if message.is_thread_reply() {
      self.open_thread(&top_level, window, cx);
    }
    self.focus = Some(id);
    self.sync_marks(cx);
    if self.find.is_some() {
      self._focus_clear = None;
      return;
    }
    self._focus_clear = Some(cx.spawn(async move |this, cx| {
      cx.background_executor().timer(Duration::from_secs(3)).await;
      let _ = this.update(cx, |this, cx| {
        this.focus = None;
        this.sync_marks(cx);
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
      by_message: Rc::default(),
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
    self.sync_marks(cx);
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
    self.sync_marks(cx);
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
    self.sync_marks(cx);
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
    let mut by_message: HashMap<String, Vec<Range<usize>>> = HashMap::new();
    for (id, range) in &hits {
      by_message.entry(id.clone()).or_default().push(range.clone());
    }
    if let Some(find) = &mut self.find {
      find.hits = hits;
      find.by_message = Rc::new(by_message);
      find.index = index;
    }
    if let Some(id) = jump {
      if self.focus.as_deref() != Some(id.as_str()) {
        self.focus_message(id, window, cx);
      }
    } else {
      self.focus = None;
    }
    self.sync_marks(cx);
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
    self.command(Command::Open(self.group.id.clone()));
    cx.notify();
  }

  /// Takes the main list to its end, past the messages that came below the fold, and the pill that counts them goes.
  fn show_unseen(&mut self, cx: &mut Context<Self>) {
    self.load.unseen.set(0);
    scroll_list(&self.main, None, cx);
    cx.notify();
  }

  /// After a scroll of the main list, from where it landed: away from the top, a load that added nothing may be tried
  /// again; at the top, older messages are asked for (`check_top`); at the end of a window around a search hit, newer
  /// ones; at the list's end, the messages counted below the fold are seen and the pill goes. Redraws only for the pill.
  fn check_scroll(&self, cx: &mut Context<Self>) {
    let (at_top, at_end) = {
      let main = self.main.read(cx);
      (main.at_top(), main.at_end())
    };
    let load = &self.load;
    if !at_top {
      load.stalled.set(false);
    }
    self.check_top(cx);
    if at_end && !load.at_end.get() && !load.loading_newer.get() {
      load.loading_newer.set(true);
      self.command(Command::LoadNewer(self.group.id.clone()));
    }
    if at_end && load.unseen.get() > 0 {
      load.unseen.set(0);
      cx.notify();
    }
  }

  /// The time from which a thread's replies count as new on its footer (`message::build_rows`): the later of the
  /// conversation's own (`read_at_open` while there is one, else `caught_up`) and the last time the thread's panel
  /// was on screen. None for the thread whose panel is open, whose replies are all on screen.
  fn thread_seen(&self, topic: &str) -> Option<Micros> {
    if self.thread.as_ref().is_some_and(|thread| thread.topic == topic) {
      return None;
    }
    let base = self.read_at_open.unwrap_or(self.caught_up);
    Some(self.threads_seen.get(topic).map_or(base, |&seen| seen.max(base)))
  }

  /// Notes that a thread's panel leaves the screen: its replies up to now are read.
  fn left_thread(&mut self, topic: String) {
    self.threads_seen.insert(topic, crate::calendar_layout::now_micros());
  }

  /// Rebuilds the main list rows and the thread panel rows from the current messages.
  fn rebuild(&mut self, prepended: usize, cx: &mut Context<Self>) {
    self.rebuild_main(prepended, cx);
    let Some(thread) = &self.thread else {
      return;
    };
    let receipts = self.receipts.clone();
    if let Some(rows) = message::build_thread_rows(&self.messages, &thread.topic, &self.me_id) {
      thread.view.update(cx, |list, cx| list.set_rows(rows, 0, &receipts, cx));
    } else if let Some(thread) = self.thread.take() {
      // The starting message is gone.
      self.left_thread(thread.topic);
      self.refocus = true;
      self.set_under_thread(cx);
    }
  }

  /// Rebuilds the main list rows, whose thread footers count new replies (`thread_seen`): again whenever a thread
  /// panel opens or closes and whenever the user sends.
  fn rebuild_main(&mut self, prepended: usize, cx: &mut Context<Self>) {
    let rows = message::build_rows(&self.messages, &self.me_id, |topic| self.thread_seen(topic));
    let receipts = self.receipts.clone();
    self.main.update(cx, |list, cx| list.set_rows(rows, prepended, &receipts, cx));
  }

  /// Tells the main list whether a thread is open, which under the pop-up hides its toolbar.
  fn set_under_thread(&self, cx: &mut App) {
    let open = self.thread.is_some();
    self.main.update(cx, |list, cx| {
      if list.under_thread != open {
        list.under_thread = open;
        cx.notify();
      }
    });
  }

  /// After a layout pass: when the list shows its first row, ask for older messages.
  fn schedule_top_check(&mut self, cx: &mut Context<Self>) {
    self._top_check = Some(cx.spawn(async move |this, cx| {
      cx.background_executor().timer(Duration::from_millis(150)).await;
      let _ = this.update(cx, |this, cx| this.check_top(cx));
    }));
  }

  /// Asks for older messages while the main list shows its first row (`MessageList::at_top`), unless the start is
  /// reached, a load is on its way, or the last one added nothing (`LoadState::stalled`).
  fn check_top(&self, cx: &App) {
    let load = &self.load;
    if load.reached.get() || load.loading.get() || load.stalled.get() {
      return;
    }
    let main = self.main.read(cx);
    if main.list.viewport_bounds().size.height <= px(0.) {
      return;
    }
    if main.at_top() {
      load.loading.set(true);
      self.command(Command::LoadOlder(self.group.id.clone()));
    }
  }

  // Typing.

  pub fn set_typing(&mut self, user_id: String, name: String, typing: bool, cx: &mut Context<Self>) {
    self.typing.retain(|(id, ..)| id != &user_id);
    if typing {
      self.typing.push((user_id, name, Instant::now()));
    }
    // Who went quiet without saying so is dropped each second, while anyone is typing.
    if !self.typing.is_empty() && !self.expiring {
      self.expiring = true;
      self._expiry = Some(cx.spawn(async move |this, cx| {
        loop {
          cx.background_executor().timer(Duration::from_secs(1)).await;
          if !matches!(this.update(cx, Self::prune_typing), Ok(true)) {
            break;
          }
        }
      }));
    }
    cx.notify();
  }

  /// Drops who stopped typing a while ago. Returns whether anyone is still typing; when nobody is, the timer stops.
  fn prune_typing(&mut self, cx: &mut Context<Self>) -> bool {
    let before = self.typing.len();
    self.typing.retain(|(_, _, at)| at.elapsed() < TYPING_EXPIRY);
    if self.typing.len() != before {
      cx.notify();
    }
    self.expiring = !self.typing.is_empty();
    self.expiring
  }

  fn on_typed(&mut self, cx: &mut Context<Self>) {
    if !self.typing_sent {
      self.typing_sent = true;
      self.command(Command::SetTyping {
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
      self.command(Command::SetTyping {
        group: self.group.id.clone(),
        typing: false,
      });
    }
  }

  fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.rewriting == Some(false) {
      return;
    }
    if self.starting {
      self.send_held = true;
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
      self.command(Command::Edit {
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
      self.command(Command::Send {
        group,
        topic_id: None,
        text,
        quote,
        files: release(files, window, cx),
        chips,
      });
      self.load.unseen.set(0);
      self.sent(cx);
      scroll_list(&self.main, None, cx);
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
        this.sync_marks(cx);
      }
    });
  }

  /// A button of a message's hover toolbar.
  fn on_act(&mut self, act: Act, message: &Arc<Message>, window: &mut Window, cx: &mut Context<Self>) {
    match act {
      Act::Hover => {
        if self.hovered.as_deref() != Some(message.id.as_str()) {
          self.hovered = Some(message.id.clone());
          self.sync_marks(cx);
        }
        return;
      },
      // Once the move is over: another message's row has taken the toolbar by then, or the pointer is on this one's.
      Act::Unhover => {
        let id = message.id.clone();
        return cx.defer_in(window, move |this, _, cx| {
          if !this.on_tools.get() && this.hovered.as_deref() == Some(id.as_str()) {
            this.hovered = None;
            this.sync_marks(cx);
          }
        });
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

  /// The header's arrow: the same step back as Cmd+[ (Ctrl+[ off macOS).
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
    let parts = ListParts {
      images: self.images.clone(),
      core: self.core.clone(),
      actor: self.actor.clone(),
      open_person: self.open_person.clone(),
      on_tools: self.on_tools.clone(),
      me_avatar: self.me_avatar.clone(),
    };
    let receipts = self.receipts.clone();
    let view = cx.new(|cx| {
      let mut list = MessageList::new(rows, parts, None, false, cx);
      list.readers = Readers::place(&list.rows, &receipts);
      // The thread's own New line, placed as the main list's is, and the panel opens on it.
      let line = self
        .read_at_open
        .and_then(|read| message::new_line(&self.messages, &list.rows, &self.me_id, read, false));
      list.set_new_line(line, cx);
      if let Some(ix) = list.new_line_row() {
        list.list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) });
      }
      list
    });
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
    if let Some(old) = self.thread.take() {
      self.left_thread(old.topic);
    }
    self.thread = Some(Thread {
      topic: topic.to_string(),
      view,
      composer,
      files: Vec::new(),
      _sub: sub,
    });
    self.set_under_thread(cx);
    self.rebuild_main(0, cx);
    self.sync_marks(cx);
    cx.notify();
  }

  /// Closes the thread panel. Returns false when none was open.
  pub fn close_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    let Some(thread) = self.thread.take() else {
      return false;
    };
    release(thread.files, window, cx);
    self.left_thread(thread.topic);
    self.set_under_thread(cx);
    self.rebuild_main(0, cx);
    self.editing.take_if(|editing| editing.in_thread);
    self.composer.update(cx, |state, cx| state.focus(window, cx));
    cx.notify();
    true
  }

  /// Closes the thread when it is open as a pop-up, which Escape closes before anything else of the conversation.
  /// Returns false when no pop-up was open.
  pub fn close_thread_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    cx.global::<Settings>().thread_view == ThreadView::Modal && self.close_thread(window, cx)
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
    let sent = !editing;
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
        files: release(files, window, cx),
        chips,
      });
      scroll_list(&thread.view, None, cx);
    }
    thread.composer.update(cx, |state, cx| state.set_value("", window, cx));
    if sent {
      self.sent(cx);
    }
    cx.notify();
  }

  /// The user sent a message: writing is having read, so the New line goes and thread replies count as new only past
  /// now.
  fn sent(&mut self, cx: &mut Context<Self>) {
    self.clear_new_line(cx);
    self.caught_up = crate::calendar_layout::now_micros();
    self.rebuild_main(0, cx);
  }

  // Attachments.

  /// Adds files (dropped on the conversation, or picked) to the message being written.
  fn attach(&mut self, in_thread: bool, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
    let (files, composer) = match (&mut self.thread, in_thread) {
      (Some(thread), true) => (&mut thread.files, &thread.composer),
      (_, false) => (&mut self.files, &self.composer),
      (None, true) => return,
    };
    let mut pictures = Vec::new();
    for path in paths {
      // A folder cannot be sent.
      if path.is_file() && !files.iter().any(|file| file.path == *path) {
        let image = is_image(path);
        if image {
          pictures.push(path.clone());
        }
        files.push(Attached { path: path.clone(), image, thumb: None });
      }
    }
    composer.update(cx, |state, cx| state.focus(window, cx));
    for path in pictures {
      Self::load_thumb(in_thread, path, cx);
    }
    cx.notify();
  }

  /// Decodes a small picture of an attached image off the UI thread, for its chip.
  fn load_thumb(in_thread: bool, path: PathBuf, cx: &mut Context<Self>) {
    let read = path.clone();
    let decoding = cx
      .background_executor()
      .spawn(async move { std::fs::read(&read).ok().and_then(|bytes| images::decode(&bytes, CHIP_THUMB)) });
    cx.spawn(async move |this, cx| {
      let Some(thumb) = decoding.await else {
        return;
      };
      this
        .update(cx, |this, cx| {
          let files = match (&mut this.thread, in_thread) {
            (Some(thread), true) => &mut thread.files,
            (_, false) => &mut this.files,
            (None, true) => return,
          };
          // Still attached: the file may have been taken off or sent meanwhile.
          if let Some(file) = files.iter_mut().find(|file| file.path == path && file.thumb.is_none()) {
            file.thumb = Some(thumb);
            cx.notify();
          }
        })
        .ok();
    })
    .detach();
  }

  /// Which composer has the keyboard: `Some(true)` the thread's, `Some(false)` the main one, `None` neither.
  fn focused_composer(&self, window: &Window, cx: &App) -> Option<bool> {
    match &self.thread {
      Some(thread) if thread.composer.focus_handle(cx).is_focused(window) => Some(true),
      _ if self.composer.focus_handle(cx).is_focused(window) => Some(false),
      _ => None,
    }
  }

  /// Paste in a composer: files and pictures on the clipboard are attached; text is left to the composer.
  fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
    let Some(in_thread) = self.focused_composer(window, cx) else {
      return;
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

  fn detach(&mut self, in_thread: bool, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
    let files = match (&mut self.thread, in_thread) {
      (Some(thread), true) => &mut thread.files,
      (_, false) => &mut self.files,
      (None, true) => return,
    };
    if ix < files.len() {
      release(vec![files.remove(ix)], window, cx);
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
    let chips = files.iter().enumerate().map(|(ix, file)| {
      let name = file.path.file_name().map_or_default(|name| name.to_string_lossy().into_owned());
      let glyph = if file.image {
        // The backdrop is for pictures with see-through parts, and stands in until the picture is decoded.
        div()
          .size(px(28.))
          .flex_shrink_0()
          .rounded(px(6.))
          .bg(rgb(PLACEHOLDER))
          .children(
            file
              .thumb
              .clone()
              .map(|thumb| img(thumb).size_full().rounded(px(6.)).object_fit(ObjectFit::Cover)),
          )
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
            .on_click(cx.listener(move |this, _, window, cx| this.detach(in_thread, ix, window, cx)))
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
    let topic = match (&self.thread, in_thread) {
      (Some(thread), true) => Some(&thread.topic),
      _ => None,
    };
    let context = crate::rewrite::earlier(
      self
        .messages
        .iter()
        .rev()
        .filter(|message| !message.system && topic.is_none_or(|topic| message.topic_id == *topic))
        .map(|message| {
          let from = if message.sender_id == self.me_id {
            crate::rewrite::ME
          } else {
            message.sender_name.as_str()
          };
          (from, message.text.as_str())
        }),
    );
    let field = composer.clone();
    self.rewrite_task = Some(crate::rewrite::spawn(
      composer,
      crate::rewrite::Kind::Chat,
      context,
      window,
      cx,
      move |this, result, window, cx| {
        this.rewrite_task = None;
        this.rewriting = None;
        if let Ok(text) = &result {
          field.update(cx, |state, cx| {
            for (range, token) in completion::reattach(text, &tokens) {
              state.replace_range_with_token(range, token, window, cx).ok();
            }
          });
        }
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
      },
    ));
    cx.notify();
  }

  /// A composer's formatting bar: a row of buttons above its text, in groups with a thin divider between them. A button
  /// formats the selection as its key does.
  fn format_bar(&self, in_thread: bool, cx: &mut Context<Self>) -> Div {
    let mut bar = h_flex().w_full().px_3().pt_2().gap_1().items_center();
    for (ix, group) in FORMAT_BAR.iter().enumerate() {
      if ix > 0 {
        bar = bar.child(div().w(px(1.)).h(px(16.)).mx_1().flex_shrink_0().bg(rgb(BORDER)));
      }
      for button in *group {
        let format = button.format;
        bar = bar.child(
          div()
            .id((button.id, usize::from(in_thread)))
            .size(px(28.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(BORDER)))
            .tooltip({
              let tip = SharedString::from(match button.keys {
                Some(keys) => format!("{} ({})", button.name, crate::keys::text(keys)),
                None => button.name.to_string(),
              });
              move |window, cx| Tooltip::new(tip.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, window, cx| this.apply_format(format, in_thread, window, cx)))
            .child(icon(button.glyph, 16., MUTED)),
        );
      }
    }
    bar
  }

  /// Formats the selection of a composer, as the formatting bar's buttons and keys do. The markers go in as range
  /// edits, never by setting the whole text, so the chips of the draft stay and undo keeps its history.
  fn apply_format(&mut self, format: Format, in_thread: bool, window: &mut Window, cx: &mut Context<Self>) {
    let Some(composer) = self.composer_of(in_thread).cloned() else {
      return;
    };
    composer.update(cx, |state, cx| {
      if state.is_editable() {
        let edit = format.apply(&state.value(), state.selected_range());
        // From the last change to the first, so the offsets of the others stay as they are.
        for change in edit.changes.iter().rev() {
          state.set_selected_range(change.range.clone(), cx);
          state.replace(change.text.clone(), window, cx);
        }
        state.set_selected_range(edit.selection, cx);
      }
      // A click on a button must leave the keyboard with the text.
      state.focus(window, cx);
    });
  }

  /// A formatting key: it formats the composer that has the keyboard. In any other text field of the conversation,
  /// such as the find bar, it is left for others to take.
  fn format_key(&mut self, format: Format, window: &mut Window, cx: &mut Context<Self>) {
    match self.focused_composer(window, cx) {
      Some(in_thread) => self.apply_format(format, in_thread, window, cx),
      None => cx.propagate(),
    }
  }

  /// The toggle at a composer's right: shows or hides the formatting bar of every composer, and leaves the keyboard
  /// with the text.
  fn toggle_format_bar(&mut self, in_thread: bool, window: &mut Window, cx: &mut Context<Self>) {
    Settings::update(cx, |settings| settings.show_format_bar.toggle());
    if let Some(composer) = self.composer_of(in_thread) {
      composer.update(cx, |state, cx| state.focus(window, cx));
    }
  }

  /// A composer's field: the formatting bar when it is shown, then the row with the button that attaches files, the
  /// text, the toggle of the bar, and rewrite when it is on.
  fn composer_field(&self, composer: &Entity<TextareaState>, in_thread: bool, cx: &mut Context<Self>) -> Div {
    let settings = cx.global::<Settings>();
    let rewrite = settings.rewrite != RewriteTool::Off;
    let always = settings.rewrite_always;
    let bar = settings.show_format_bar;
    let busy = self.rewriting == Some(in_thread);
    let row = h_flex()
      .w_full()
      .pl_2()
      .pr_4()
      .py_2()
      .gap_2()
      .items_end()
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
      .child(
        div()
          .id(("format-toggle", usize::from(in_thread)))
          .size(px(28.))
          .mb(px(3.))
          .flex_shrink_0()
          .flex()
          .items_center()
          .justify_center()
          .rounded_full()
          .cursor_pointer()
          .hover(|s| s.bg(rgb(BORDER)))
          .when(bar, |button| button.bg(rgb(ACCENT_BG)))
          .tooltip(hint(if bar {
            "Hide formatting options"
          } else {
            "Show formatting options"
          }))
          .on_click(cx.listener(move |this, _, window, cx| this.toggle_format_bar(in_thread, window, cx)))
          .child(icon(Icon::Baseline, 21., if bar { ACCENT } else { MUTED })),
      )
      .when(rewrite, |row| {
        row.child(crate::rewrite::button(
          ("rewrite", usize::from(in_thread)),
          ("rewrite-pulse", usize::from(in_thread)),
          busy,
          always,
          cx,
          move |this, _, window, cx| this.rewrite(in_thread, false, window, cx),
        ))
      });
    let field = v_flex()
      .w_full()
      .rounded(px(24.))
      .bg(rgb(if busy { ACCENT_BG } else { FIELD }))
      .when(bar, |field| field.child(self.format_bar(in_thread, cx)))
      .child(row);
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
    let members: Vec<Member> = users.iter().cloned().map(Member::new).collect();
    let mut nicks: Vec<&Member> = members.iter().collect();
    nicks.sort_by(|a, b| a.key().cmp(b.key()));
    self.nicks = nicks.into_iter().map(|member| member.user.clone()).collect();
    self.members = Some(members);
    self.refresh_menus(cx);
    cx.notify();
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
        self.command(Command::LoadMembers(self.group.id.clone()));
      },
      Trigger::Command if !self.asked_commands => {
        self.asked_commands = true;
        self.command(Command::LoadSlashCommands(self.group.id.clone()));
      },
      _ => {},
    }
    let members = self.members.as_deref().unwrap_or_default();
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

  /// The thread's header, rows and composer: the side panel and the pop-up show the same ones.
  fn render_thread_panel(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
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
      .child(thread.view.clone().cached(StyleRefinement::default().size_full()));
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
        .id("thread")
        // Files dropped on the thread go out with the next reply.
        .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(ROW_HOVER)))
        .on_drop(
          cx.listener(|this, dropped: &ExternalPaths, window, cx| this.attach(true, dropped.paths(), window, cx)),
        )
        .child(header)
        .child(body)
        .child(composer),
    )
  }

  /// The thread as a column beside the conversation, which shares the width with it.
  fn thread_side(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
    let panel = self.render_thread_panel(cx)?;
    Some(
      panel
        .w(relative(0.45))
        .h_full()
        .flex_shrink_0()
        .border_l_1()
        .border_color(rgb(BORDER))
        .into_any_element(),
    )
  }

  /// The thread over the dimmed window, centered as the window's other modals are and sized with the window (`viewport`
  /// is the window's). The window root draws it, under its menus and pickers; `None` unless a thread is open and
  /// `Settings::thread_view` is the pop-up. A press on the backdrop closes it, unless that press was closing the text
  /// panel opened from inside it; presses inside it stay there.
  pub fn thread_popup(&self, viewport: Size<Pixels>, cx: &mut Context<Self>) -> Option<AnyElement> {
    if cx.global::<Settings>().thread_view != ThreadView::Modal {
      return None;
    }
    let panel = self.render_thread_panel(cx)?;
    let (width, height) = modal_size(viewport, THREAD_POPUP, (0.55, 0.85));
    Some(
      self
        .keyed(div(), cx)
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x00000073))
        .occlude()
        // The capture phase runs before the text panel's press outside closes it.
        .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
          this.backdrop_kept = cx.global::<TextPanelHandle>().0.read(cx).is_open();
        }))
        .on_mouse_down(
          MouseButton::Left,
          cx.listener(|this, _, window, cx| {
            if !std::mem::take(&mut this.backdrop_kept) {
              this.close_thread(window, cx);
            }
          }),
        )
        .child(
          panel
            .w(width)
            .h(height)
            .rounded(px(14.))
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(BORDER))
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
        )
        .focus_trap("thread-popup", &self.thread_focus)
        .into_any_element(),
    )
  }

  /// The keys a conversation's composers share, on `root`: the conversation itself and the thread pop-up, which the
  /// window root draws outside it.
  fn keyed(&self, root: Div, cx: &mut Context<Self>) -> Div {
    root
      // Before the composer sees the paste, so files never land in it as text.
      .capture_action(cx.listener(Self::paste))
      .on_action(cx.listener(|this, _: &FormatBold, window, cx| this.format_key(Format::Bold, window, cx)))
      .on_action(cx.listener(|this, _: &FormatItalic, window, cx| this.format_key(Format::Italic, window, cx)))
      .on_action(cx.listener(|this, _: &FormatStrikethrough, window, cx| {
        this.format_key(Format::Strikethrough, window, cx);
      }))
      .on_action(cx.listener(|this, _: &FormatBulletedList, window, cx| this.format_key(Format::Bullets, window, cx)))
  }
}

/// The size of a modal that grows with the window: `share` of the viewport each way, no smaller than `least` nor larger
/// than `most`, and always clear of the window's edges.
pub fn modal_size(
  viewport: Size<Pixels>,
  (least, most): (Size<f32>, Size<f32>),
  share: (f32, f32),
) -> (Pixels, Pixels) {
  let fit = |room: Pixels, share: f32, least: f32, most: f32| {
    (room * share).clamp(px(least), px(most)).min(room - px(MODAL_MARGIN * 2.))
  };
  (
    fit(viewport.width, share.0, least.width, most.width),
    fit(viewport.height, share.1, least.height, most.height),
  )
}

impl Drop for ConversationView {
  fn drop(&mut self) {
    self.stop_typing();
  }
}

impl ConversationView {
  /// Creates a Google Meet meeting and posts its link here.
  fn start_meet(&mut self, cx: &mut Context<Self>) {
    self.command(Command::StartMeet(self.group.id.clone()));
    scroll_list(&self.main, None, cx);
    cx.notify();
  }

  /// The IRC layout lists a space's members beside it from the start, so it asks for them as the space opens or
  /// the layout is picked, rather than on the first `@`.
  fn want_members(&mut self, cx: &App) {
    if !self.asked_members && self.group.kind == GroupKind::Space && cx.global::<Settings>().layout == Layout::Irc {
      self.asked_members = true;
      self.command(Command::LoadMembers(self.group.id.clone()));
    }
  }

  /// The call button of the header.
  fn meet_button(&self, height: f32, glyph: f32, cx: &mut Context<Self>) -> Stateful<Div> {
    div()
      .id("meet")
      .h(px(height))
      .px_4()
      .flex()
      .items_center()
      .justify_center()
      .rounded_full()
      .bg(rgb(FIELD))
      .cursor_pointer()
      .hover(|s| s.bg(rgb(ROW_SELECTED)))
      .on_click(cx.listener(|this, _, _, cx| this.start_meet(cx)))
      .child(icon(Icon::Phone, glyph, TEXT))
  }

  /// The header: Back where the layout has no list beside the conversation, the avatar and name, then the call
  /// button.
  fn header(&self, cx: &mut Context<Self>) -> Div {
    h_flex()
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
      .child(self.meet_button(36., 17., cx))
  }

  /// IRC's title bar: one line of the buffer name in the monospaced font, then the call button.
  fn irc_header(&self, cx: &mut Context<Self>) -> Div {
    h_flex()
      .h(px(IRC_BAR_HEIGHT))
      .flex_shrink_0()
      .px_3()
      .gap_2()
      .items_center()
      .border_b_1()
      .border_color(rgb(BORDER))
      .child(
        div()
          .flex_1()
          .min_w_0()
          .truncate()
          .font_family(mono())
          .text_size(px(13.))
          .font_weight(FontWeight::BOLD)
          .text_color(rgb(TEXT))
          .child(emoji_font::text(buffer_name(&self.group))),
      )
      .child(self.meet_button(26., 14., cx))
  }

  /// IRC: the space's members A to Z, one monospaced line each, at a fixed width so nothing moves when they arrive.
  /// A click opens the chat with that person, as a sender's name does.
  fn nick_list(&self, cx: &mut Context<Self>) -> AnyElement {
    let right = cx.global::<Settings>().sidebar_side == SidebarSide::Left;
    let count = self.nicks.len();
    div()
      .w(px(NICK_LIST_WIDTH))
      .h_full()
      .flex_shrink_0()
      .py_1()
      .when(right, gpui_kit::Styled::border_l_1)
      .when(!right, gpui_kit::Styled::border_r_1)
      .border_color(rgb(BORDER))
      .font_family(mono())
      .text_size(px(13.))
      .child(
        uniform_list(
          "nicks",
          count,
          cx.processor(|this, range: Range<usize>, _, _| this.nick_rows(range)),
        )
        .size_full(),
      )
      .into_any_element()
  }

  fn nick_rows(&self, range: Range<usize>) -> Vec<AnyElement> {
    range
      .filter_map(|ix| self.nicks.get(ix).map(|user| (ix, user)))
      .map(|(ix, user)| {
        let (id, open) = (user.id.clone(), self.open_person.clone());
        div()
          .id(("nick", ix))
          .h(px(NICK_ROW_HEIGHT))
          .w_full()
          .px_3()
          .flex()
          .items_center()
          .cursor_pointer()
          .hover(|s| s.bg(rgb(ROW_HOVER)))
          .on_click(move |_, window, cx| open(&id, window, cx))
          .child(
            div()
              .min_w_0()
              .truncate()
              .text_color(rgb(MUTED))
              .child(emoji_font::text(user.name.clone())),
          )
          .into_any_element()
      })
      .collect()
  }
}

impl Render for ConversationView {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    if self.refocus {
      self.refocus = false;
      let composer = self.composer.clone();
      window.defer(cx, move |window, cx| composer.update(cx, |state, cx| state.focus(window, cx)));
    }
    let irc = cx.global::<Settings>().layout == Layout::Irc;
    let header = if irc {
      self.irc_header(cx)
    } else {
      self.header(cx)
    };

    // Over the bottom of the message list, so it moves neither the list nor the composer as it comes and goes. The
    // window around a search hit comes first.
    let unseen = self.load.unseen.get();
    let latest = if !self.load.at_end.get() {
      Some(latest_pill(
        "Jump to latest".into(),
        cx.listener(|this, _, _, cx| this.jump_latest(cx)),
      ))
    } else if unseen > 0 {
      let label = if unseen == 1 {
        SharedString::from("1 new message")
      } else {
        format!("{unseen} new messages").into()
      };
      Some(latest_pill(label, cx.listener(|this, _, _, cx| this.show_unseen(cx))))
    } else {
      None
    };
    let body = if self.main.read(cx).rows.is_empty() {
      div()
        .relative()
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
        .children(latest)
        .into_any_element()
    } else {
      div()
        .id("body")
        .on_hover(cx.listener(|this, hovered: &bool, window, cx| this.list_hovered(*hovered, window, cx)))
        .relative()
        .flex_1()
        .min_h_0()
        .w_full()
        .child(self.main.clone().cached(StyleRefinement::default().size_full()))
        .children(latest)
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
      .child(composer)
      .children(self.find_bar(cx));
    // IRC: a space's nick list sits on the side away from the buffer list.
    let nicks = (irc && self.group.kind == GroupKind::Space).then(|| self.nick_list(cx));
    let nicks_left = cx.global::<Settings>().sidebar_side == SidebarSide::Right;
    let (left, right) = if nicks_left {
      (nicks, None)
    } else {
      (None, nicks)
    };
    let side = match cx.global::<Settings>().thread_view {
      ThreadView::Side => self.thread_side(cx),
      // The window root draws the pop-up, which reserves no width here.
      ThreadView::Modal => None,
    };
    self
      .keyed(h_flex(), cx)
      .relative()
      .size_full()
      .children(left)
      .child(main)
      .children(side)
      .children(right)
  }
}

/// The pill at the bottom center of the message list that takes it to its newest messages, over the list. Only the
/// pill takes the pointer: the list scrolls and hovers beside it.
fn latest_pill(label: SharedString, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> AnyElement {
  div()
    .absolute()
    .bottom(px(8.))
    .left_0()
    .right_0()
    .flex()
    .justify_center()
    .child(
      div()
        .id("jump-latest")
        .occlude()
        .px_3()
        .py_1()
        .rounded_full()
        .bg(rgb(ACCENT_BG))
        .border_1()
        .border_color(rgb(BORDER))
        .shadow_lg()
        .text_size(px(12.))
        .text_color(rgb(TEXT))
        .cursor_pointer()
        .on_click(on_click)
        .child(label),
    )
    .into_any_element()
}

/// A file the app can show a small picture of beside its name.
fn is_image(path: &Path) -> bool {
  let extension = path.extension().map(|extension| extension.to_string_lossy().to_lowercase());
  matches!(extension.as_deref(), Some("png" | "jpg" | "jpeg" | "gif" | "webp"))
}

/// Where pictures pasted from the clipboard are kept until they are sent: in the scratch folder, which is emptied
/// when the app starts.
fn pasted_dir() -> PathBuf {
  gchat_core::scratch_dir().join("pasted")
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
  let path = pasted_dir().join(format!(
    "Pasted image {}.{extension}",
    jiff::ZonedDateTime::now().strftime("%Y-%m-%d at %H.%M.%S%.3f")
  ));
  gchat_core::write_private(&path, &bytes).ok()?;
  Some(path)
}

/// The paths of `files`, whose small pictures leave the windows' sprite atlases (`window` is the one being updated).
fn release(files: Vec<Attached>, window: &mut Window, cx: &mut App) -> Vec<PathBuf> {
  files
    .into_iter()
    .map(|file| {
      if let Some(thumb) = file.thumb {
        cx.drop_image(thumb, Some(&mut *window));
      }
      file.path
    })
    .collect()
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
