//! Rendering of one conversation row: header, bubbles with formatted text, attachments, cards, reactions.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gchat_core::{
  Attachment, AttachmentKind, Card, Command, Core, GroupId, Message, Micros, ReadReceipt, Span, SpanKind,
};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::emoji_font;
use crate::emoji_picker;
use crate::images::Images;
use crate::menu::{self, MenuItem};
use crate::receipt_logic;
use crate::settings::{MessageStyle, Settings};
use crate::text_panel;
use crate::theme::{
  self, ACCENT, ACCENT_BG, BG, BORDER, BUBBLE, BUBBLE_OWN, CODE_BG, FIELD, MARK, MUTED, PANEL, PLACEHOLDER, POPOVER,
  RED, ROW_HOVER, TEXT, Theme, Tone, WHITE, rgb,
};
use crate::ui::{Icon, avatar, icon};
use crate::util;
use crate::viewer;

const FIVE_MINUTES: i64 = 5 * 60 * 1_000_000;
const ONE_HOUR: i64 = 60 * 60 * 1_000_000;

/// The parts of a row with an element id: each is `(row.id, PART_*)`, plus the index of the item for the ranges.
const PART_AVATAR: usize = 0;
const PART_NAME: usize = 1;
const PART_BUBBLE: usize = 2;
const PART_ROW: usize = 3;
const PART_TOOLS: usize = 4;
const PART_READERS: usize = 5;
const PART_THREAD: usize = 6;
const PART_REACT: usize = 7;
const PART_EDIT: usize = 8;
const PART_THREAD_TOOL: usize = 9;
const PART_QUOTE: usize = 10;
const PART_MORE: usize = 11;
const PART_QUICK: usize = 16;
const PART_ATTACHMENT: usize = 1 << 10;
const PART_REACTION: usize = 2 << 10;
const PART_TEXT: usize = 3 << 10;

/// One avatar of whoever has read up to a message: the picture, and the ring in the page's color around it that keeps
/// it apart from its neighbors and from the part of the message it sits on.
const READER_FACE: f32 = 16.;
const READER_RING: f32 = 18.;
/// How far each avatar after the nearest reaches over the one before it, rings included.
const READER_OVERLAP: f32 = 6.;
/// How far the nearest avatar reaches over the corner it sits on, ring included.
const READER_TUCK: f32 = 4.;
/// The room kept between the avatars and the row's edge when they have to move over the part they sit on.
const READER_EDGE_GAP: f32 = 2.;
/// The padding at both ends of a message row: the avatars may reach into it, all but `READER_EDGE_GAP`.
const ROW_PAD: f32 = 16.;

/// Reply summary of a thread: shown as a footer under its starting message in the main list and as the
/// `N replies` divider in the thread panel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ThreadInfo {
  pub count: usize,
  pub last: Micros,
}

/// Opens the thread panel for a topic id.
pub type Opener = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// One list row: a message plus how it is presented.
#[derive(Clone)]
pub struct Row {
  pub msg: Arc<Message>,
  /// The message's id, the start of the element ids of its parts.
  pub id: SharedString,
  /// Date separator shown above this message (first message of a day).
  pub date: Option<SharedString>,
  /// Starts a run: shows avatar, name and time.
  pub head: bool,
  pub own: bool,
  /// Set on a message that started a thread with replies.
  pub thread: Option<ThreadInfo>,
  /// The text as it is drawn: made on the first draw, kept while the theme and the fonts stay.
  prepared: RefCell<Option<Rc<Prepared>>>,
  /// The header's time once it is a clock time, which no longer changes.
  clock: OnceCell<SharedString>,
  /// The IRC style's time (`irc_clock`), which is always a clock time and so never changes. Apart from `clock` because
  /// the two are written differently on a 24-hour clock (`09:05` here, `9:05` there).
  irc_clock: OnceCell<SharedString>,
}

impl Row {
  fn new(msg: &Arc<Message>, date: Option<SharedString>, head: bool, own: bool, thread: Option<ThreadInfo>) -> Self {
    Self {
      msg: msg.clone(),
      id: SharedString::from(msg.id.as_str()),
      date,
      head,
      own,
      thread,
      prepared: RefCell::default(),
      clock: OnceCell::new(),
      irc_clock: OnceCell::new(),
    }
  }

  pub fn same(&self, other: &Self) -> bool {
    self.head == other.head
      && self.own == other.own
      && self.thread == other.thread
      && self.date == other.date
      && (Arc::ptr_eq(&self.msg, &other.msg) || *self.msg == *other.msg)
  }

  /// Takes over what `old`, a row `same` as this one, already worked out, so it is not worked out again.
  pub fn adopt(&mut self, old: &mut Self) {
    *self.prepared.get_mut() = old.prepared.get_mut().take();
    if let Some(clock) = old.clock.take() {
      let _ = self.clock.set(clock);
    }
    if let Some(clock) = old.irc_clock.take() {
      let _ = self.irc_clock.set(clock);
    }
  }

  /// The text as it is drawn, worked out on the first call and again after the theme or the fonts change.
  fn prepared(&self) -> Rc<Prepared> {
    let mut slot = self.prepared.borrow_mut();
    if let Some(prepared) = slot.as_ref().filter(|prepared| prepared.fresh()) {
      return prepared.clone();
    }
    let prepared = Rc::new(Prepared::new(&self.msg));
    *slot = Some(prepared.clone());
    prepared
  }

  /// The time in the header (`util::message_time`): `now` and minutes while recent, kept once it is a clock time. The
  /// IRC style shows `irc_time` instead.
  fn time(&self) -> SharedString {
    if let Some(clock) = self.clock.get() {
      return clock.clone();
    }
    // Asked before the text is made, so a time that reads as minutes is never kept.
    let old = chrono::Local::now().timestamp_micros() - self.msg.time >= ONE_HOUR;
    let time = SharedString::from(util::message_time(self.msg.time));
    if old {
      let _ = self.clock.set(time.clone());
    }
    time
  }

  /// The IRC style's time column: always the clock time (`irc_clock`), never `now` or minutes.
  fn irc_time(&self) -> SharedString {
    self
      .irc_clock
      .get_or_init(|| irc_clock(self.msg.time, crate::clock::is_24_hour()).into())
      .clone()
  }
}

fn starts_run(prev: Option<&Row>, msg: &Message, dated: bool) -> bool {
  match prev {
    Some(prev) => {
      dated
        || msg.system
        || prev.msg.system
        || prev.thread.is_some()
        || prev.msg.sender_id != msg.sender_id
        || msg.time - prev.msg.time > FIVE_MINUTES
    },
    None => true,
  }
}

fn thread_infos(messages: &[Arc<Message>]) -> HashMap<&str, ThreadInfo> {
  let mut infos: HashMap<&str, ThreadInfo> = HashMap::new();
  for msg in messages.iter().filter(|m| m.is_thread_reply()) {
    let info = infos.entry(msg.topic_id.as_str()).or_insert(ThreadInfo { count: 0, last: 0 });
    info.count += 1;
    info.last = info.last.max(msg.time);
  }
  infos
}

/// Rows of the main list: top-level messages only, each thread starter carrying its reply summary.
pub fn build_rows(messages: &[Arc<Message>], me_id: &str) -> Vec<Row> {
  let infos = thread_infos(messages);
  let mut rows: Vec<Row> = Vec::with_capacity(messages.len());
  let mut prev_day = i32::MIN;
  // The local day of the last message looked at and its span: the messages of one day take no time-zone conversion.
  let mut day: Option<(i32, Range<Micros>)> = None;
  for msg in messages.iter().filter(|m| !m.is_thread_reply()) {
    let key = match &day {
      Some((key, span)) if span.contains(&msg.time) => *key,
      _ => {
        let (key, span) = util::day_span(msg.time);
        day = Some((key, span));
        key
      },
    };
    let date = (key != prev_day).then(|| SharedString::from(util::date_label(msg.time)));
    let head = starts_run(rows.last(), msg, date.is_some());
    prev_day = key;
    let thread = infos.get(msg.id.as_str()).copied();
    let own = !me_id.is_empty() && msg.sender_id == me_id;
    rows.push(Row::new(msg, date, head, own, thread));
  }
  rows
}

/// Rows of the thread panel: the starting message, then its replies oldest first. None when the starter is gone.
pub fn build_thread_rows(messages: &[Arc<Message>], topic: &str, me_id: &str) -> Option<Vec<Row>> {
  let start = messages.iter().find(|m| m.id == topic)?;
  let infos = thread_infos(messages);
  let own = |msg: &Message| !me_id.is_empty() && msg.sender_id == me_id;
  let mut rows = vec![Row::new(start, None, true, own(start), infos.get(topic).copied())];
  for msg in messages.iter().filter(|m| m.is_thread_reply() && m.topic_id == topic) {
    let head = starts_run(rows.last(), msg, false);
    rows.push(Row::new(msg, None, head, own(msg), None));
  }
  Some(rows)
}

/// Who has read up to each message of a list of rows, by message id: their avatars sit on its corner.
#[derive(Default)]
pub struct Readers(HashMap<SharedString, Rc<[ReadReceipt]>>);

impl Readers {
  /// Puts each of `receipts` on the newest message of `rows` sent at or before their read time. A line of the
  /// conversation itself, a message still going out, and one that failed are nobody's last read message.
  pub fn place(rows: &[Row], receipts: &[ReadReceipt]) -> Self {
    if receipts.is_empty() {
      return Self::default();
    }
    let candidates: Vec<&Row> = rows
      .iter()
      .filter(|row| !row.msg.system && !row.msg.pending && !row.msg.failed)
      .collect();
    let times: Vec<Micros> = candidates.iter().map(|row| row.msg.time).collect();
    let read_times: Vec<Micros> = receipts.iter().map(|receipt| receipt.read_time).collect();
    let by_message = receipt_logic::place(&times, &read_times)
      .into_iter()
      .filter_map(|(at, readers)| {
        let id = candidates.get(at)?.id.clone();
        let readers: Rc<[ReadReceipt]> =
          readers.into_iter().filter_map(|reader| receipts.get(reader).cloned()).collect();
        Some((id, readers))
      })
      .collect();
    Self(by_message)
  }

  /// Who has read up to the message `id`, earliest reader first; `None` when nobody has.
  pub fn of(&self, id: &str) -> Option<&Rc<[ReadReceipt]>> {
    self.0.get(id)
  }
}

pub struct MsgCtx<'a> {
  pub images: &'a Images,
  pub core: &'a Core,
  pub me_avatar: Option<&'a str>,
  /// Present in the main list: thread starters get a clickable footer. Absent in the thread panel, where
  /// the starter gets the `N replies` divider instead.
  pub open_thread: Option<&'a Opener>,
  /// The message a search hit jumped to: its row gets a tint.
  pub focus: Option<&'a str>,
  /// Where the find bar's query is in this message's text: empty while the find bar is closed or finds nothing here.
  pub find: &'a [Range<usize>],
  /// The current find hit in this message, if any.
  pub find_hit: Option<Range<usize>>,
  /// Runs what a message's toolbar asks for.
  pub act: &'a Actor,
  /// Opens the chat with a user id: clicking a sender's name or avatar, or a mention.
  pub open_person: &'a Opener,
  /// "Reply in thread" is offered: the main list of a space.
  pub threads: bool,
  /// The message under the pointer: the one that shows its toolbar.
  pub hovered: Option<&'a str>,
  /// Set while the pointer is on a toolbar, which can reach over the row above its message.
  pub on_tools: &'a Rc<Cell<bool>>,
  /// Who has read up to this message: their avatars sit on its bottom corner, out of the row's layout. `None` in a
  /// conversation with read receipts off.
  pub readers: Option<&'a Rc<[ReadReceipt]>>,
  /// IRC style: the width of the sender column, as wide as the longest name among the rows (`irc_nick_width`).
  pub nick_width: f32,
  /// The window's pixels per point: thumbnails are decoded for the size they are drawn at.
  pub scale: f32,
}

/// What a message's toolbar asks the conversation to do.
#[derive(Clone, Copy)]
pub enum Act {
  /// Put the message's text in the composer to change it.
  Edit,
  /// Quote the message in the next message sent.
  Quote,
  /// Open the thread panel on the message.
  Thread,
  /// The pointer came onto the message: it is the one with the toolbar now.
  Hover,
  /// The pointer left a toolbar: the conversation checks whether any message is still pointed at.
  Leave,
}

pub type Actor = Rc<dyn Fn(Act, &Arc<Message>, &mut Window, &mut App)>;

pub fn render_row(row: &Row, ctx: &MsgCtx, cx: &App) -> AnyElement {
  let msg = &row.msg;
  if cx.global::<Settings>().message_style == MessageStyle::Irc {
    return irc_row(row, ctx, cx);
  }
  let focused = ctx.focus == Some(msg.id.as_str());
  let mut out = v_flex().w_full().items_center().when(focused, |row| row.bg(rgb(ACCENT_BG)));
  if let Some(date) = &row.date {
    out = out.child(
      div().py_3().child(
        div()
          .px_3()
          .py_1()
          .rounded_full()
          .bg(rgb(FIELD))
          .text_size(px(11.))
          .text_color(rgb(MUTED))
          .child(date.clone()),
      ),
    );
  }
  let prepared = row.prepared();
  if msg.system {
    return out
      .child(
        div()
          .px_4()
          .py_1()
          .max_w(px(640.))
          .text_size(px(12.))
          .text_color(rgb(MUTED))
          .text_center()
          .child(plain_text(row, &prepared, ctx)),
      )
      .into_any_element();
  }

  // Comfortable layout: the user's own messages sit on the right, with only their time above.
  let aside = row.own && cx.global::<Settings>().message_style == MessageStyle::Comfortable;
  let name = if row.own {
    "You".to_string()
  } else {
    msg.sender_name.clone()
  };
  let avatar_url = if row.own {
    ctx.me_avatar.or(msg.sender_avatar.as_deref())
  } else {
    msg.sender_avatar.as_deref()
  };
  // Someone else's avatar and name open the chat with them.
  let person = (!row.own).then(|| (msg.clone(), ctx.open_person.clone()));
  let avatar_col = (!aside).then(|| {
    div().w(px(36.)).flex_shrink_0().when(row.head, |d| {
      let picture =
        div()
          .id((row.id.clone(), PART_AVATAR))
          .child(avatar(ctx.images, cx, avatar_url, None, &msg.sender_name, 32.));
      d.child(match person.clone() {
        Some((sender, open)) => picture
          .cursor_pointer()
          .on_click(move |_, window, cx| open(&sender.sender_id, window, cx)),
        None => picture,
      })
    })
  });

  // As wide as the row lets it be, so the part the readers' avatars sit on knows the room beside it (`read_mark`). Its
  // children keep their own widths: each is no wider than it needs, as in a column as wide as the widest of them.
  let mut content = v_flex()
    .relative()
    .flex_1()
    .when(aside, gpui_kit::Styled::items_end)
    .when(!aside, gpui_kit::Styled::items_start)
    .gap_1()
    .min_w_0()
    .when(msg.pending, |d| d.opacity(0.55));
  // What the message shows, top to bottom, and where in it the text bubble, the last attachment and the last card are.
  let mut body: Vec<AnyElement> = Vec::new();
  let mut bubble_at = None;
  if row.head {
    let time = div().text_size(px(11.)).text_color(rgb(MUTED)).child(row.time());
    let header = if aside {
      time
    } else {
      h_flex()
        .gap_2()
        .items_baseline()
        .child({
          let label = div()
            .id((row.id.clone(), PART_NAME))
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(TEXT))
            .child(name);
          match person {
            Some((sender, open)) => label
              .cursor_pointer()
              .hover(gpui_kit::Styled::underline)
              .on_click(move |_, window, cx| open(&sender.sender_id, window, cx)),
            None => label,
          }
        })
        .child(time)
    };
    body.push(header.into_any_element());
  }

  let bubble_bg = if row.own { BUBBLE_OWN } else { BUBBLE };
  let has_text = !msg.text.trim().is_empty();
  let has_bubble = has_text || msg.quote.is_some();
  if has_bubble {
    // A click on the bubble, the quote in it included, opens its text in the panel where it can be selected. A link or
    // a mention in the text stops the click (`Segment::draw`), as does the readers' overlay on its corner.
    let mut bubble = v_flex()
      .id((row.id.clone(), PART_BUBBLE))
      .max_w(px(600.))
      .gap_1()
      .px_3p5()
      .py_2()
      .rounded(px(16.))
      .bg(rgb(bubble_bg))
      .text_size(px(14.))
      .text_color(rgb(TEXT))
      .when(has_text, |bubble| {
        let msg = msg.clone();
        bubble.on_click(move |event, window, cx| text_panel::show(event.position(), &msg.text, window, cx))
      });
    if let (Some(quote), Some(clipped)) = (&msg.quote, &prepared.quote) {
      bubble = bubble.child(
        v_flex()
          .gap_0p5()
          .pl_2()
          .border_l_2()
          .border_color(rgb(MUTED))
          .text_size(px(12.5))
          .text_color(rgb(MUTED))
          .child(div().font_weight(FontWeight::SEMIBOLD).child(quote.sender_name.clone()))
          .child(div().child(clipped.clone())),
      );
    }
    if has_text {
      bubble = bubble.child(rich_text(row, &prepared, ctx));
      if msg.edited {
        bubble = bubble.child(div().text_size(px(11.)).text_color(rgb(MUTED)).child("(edited)"));
      }
    }
    bubble_at = Some(body.len());
    body.push(bubble.into_any_element());
  }

  let (attachment_at, card_at) = push_extras(&mut body, row, bubble_bg, ctx, cx);
  content = place_readers(content, body, [bubble_at, attachment_at, card_at], aside, row, ctx, cx);

  // Floats over the top edge of the first bubble, as in Google Chat, so it can reach over the row above.
  // Only the message under the pointer has one, and it swallows the pointer: the row beneath it does not
  // count as pointed at, so two toolbars never show together. It always sits on its bubble, which on a row
  // that starts a run puts it over the sender's name while the pointer is there.
  if let Some(tools) = floating_tools(row, ctx) {
    // Where the first bubble starts: below the name or time line of a row that starts a run.
    let bubble_top = match (row.head, aside) {
      (true, false) => 24.,
      (true, true) => 19.,
      (false, _) => 0.,
    };
    content = content.child(
      tools
        .top(px(bubble_top - 26.))
        .when(aside, |tools| tools.right(px(12.)))
        .when(!aside, |tools| tools.left(px(12.))),
    );
  }
  let (act, entered) = (ctx.act.clone(), msg.clone());
  let out = out.child(
    h_flex()
      .id((row.id.clone(), PART_ROW))
      .w_full()
      .max_w(px(780.))
      .px(px(ROW_PAD))
      .pt(if row.head { px(10.) } else { px(2.) })
      .gap_2()
      .items_start()
      .on_hover(move |hovered, window, cx| {
        if *hovered {
          act(Act::Hover, &entered, window, cx);
        }
      })
      .children(avatar_col)
      .child(content),
  );
  match (row.thread, ctx.open_thread) {
    (Some(info), None) => out.child(thread_divider(info)).into_any_element(),
    _ => out.into_any_element(),
  }
}

/// What a message shows under its text, in `body` after what is already there: attachments, the starred mark, cards,
/// reactions, the failed note, and the thread footer. Answers where in `body` the last attachment and the last card are.
fn push_extras(
  body: &mut Vec<AnyElement>,
  row: &Row,
  bubble_bg: Tone,
  ctx: &MsgCtx,
  cx: &App,
) -> (Option<usize>, Option<usize>) {
  let msg = &row.msg;
  let (mut attachment_at, mut card_at) = (None, None);
  for i in 0..msg.attachments.len() {
    attachment_at = Some(body.len());
    body.push(attachment_view(
      msg,
      i,
      (row.id.clone(), PART_ATTACHMENT + i).into(),
      bubble_bg,
      ctx,
      cx,
    ));
  }
  if msg.starred {
    body.push(
      h_flex()
        .gap_1()
        .items_center()
        .text_size(px(11.))
        .text_color(rgb(MUTED))
        .child(icon(Icon::Star, 11., MUTED))
        .child("Starred")
        .into_any_element(),
    );
  }
  for (i, card) in msg.cards.iter().enumerate() {
    card_at = Some(body.len());
    body.push(card_view(card, msg, ctx, cx, i));
  }
  if !msg.reactions.is_empty() {
    body.push(reactions_view(msg, &row.id, ctx.core));
  }
  if msg.failed {
    body.push(
      div()
        .text_size(px(12.))
        .text_color(rgb(RED))
        .child("Not sent")
        .into_any_element(),
    );
  }
  if let (Some(info), Some(open)) = (row.thread, ctx.open_thread) {
    body.push(thread_footer(row, info, open.clone()));
  }
  (attachment_at, card_at)
}

/// `body` into `content`, with the avatars of whoever has read up to the message on the first of `marks` there is (the
/// text, else the last attachment, else the last card), never below the reactions, the cards, or the thread footer; a
/// message with none of those has them on the last thing it shows.
fn place_readers(
  mut content: Div,
  body: Vec<AnyElement>,
  marks: [Option<usize>; 3],
  aside: bool,
  row: &Row,
  ctx: &MsgCtx,
  cx: &App,
) -> Div {
  let mark = marks.into_iter().flatten().next().or_else(|| body.len().checked_sub(1));
  let mut readers = readers_group(aside, row, ctx, cx);
  for (ix, part) in body.into_iter().enumerate() {
    content = content.child(if mark == Some(ix) {
      read_mark(part, readers.take(), aside).into_any_element()
    } else {
      part
    });
  }
  content
}

/// The reactions and toolbar that float over the message under the pointer, unplaced: `None` on any other message and
/// on one still going out or failed. It swallows the pointer, and leaving it lets the conversation check what is
/// pointed at.
fn floating_tools(row: &Row, ctx: &MsgCtx) -> Option<Stateful<Div>> {
  let msg = &row.msg;
  if msg.pending || msg.failed || ctx.hovered != Some(msg.id.as_str()) {
    return None;
  }
  let (on_tools, act, left) = (ctx.on_tools.clone(), ctx.act.clone(), msg.clone());
  Some(
    h_flex()
      .id((row.id.clone(), PART_TOOLS))
      .absolute()
      .gap_1()
      .occlude()
      .on_hover(move |hovered, window, cx| {
        on_tools.set(*hovered);
        if !*hovered {
          act(Act::Leave, &left, window, cx);
        }
      })
      .child(quick_reactions(msg, &row.id, ctx.core))
      .child(toolbar(row, ctx)),
  )
}

/// The fewest and the most characters the IRC style's sender column makes room for: a longer name ends in an ellipsis.
const IRC_NICK_CHARS: (usize, usize) = (3, 14);
/// How wide one character of the monospaced font is at the IRC style's text size, a little over most fonts' 0.6 em.
const IRC_CHAR: f32 = 8.;

/// The IRC style's time, as WeeChat writes it: the clock time, never `now` or minutes, with the hour zero-padded so
/// every time has the same length. `01:50 PM`, or `13:50` on a 24-hour clock when `h24`.
fn irc_clock(time: Micros, h24: bool) -> String {
  let at = util::local(time);
  if h24 {
    at.format("%H:%M").to_string()
  } else {
    at.format("%I:%M %p").to_string()
  }
}

/// Width of the IRC style's time column: the length of every time `irc_clock` writes (`12:59 PM`, or `23:59` when
/// `h24`) in the monospaced font.
fn irc_time_width(h24: bool) -> f32 {
  let chars: u8 = if h24 { 5 } else { 8 };
  f32::from(chars) * IRC_CHAR
}

/// The width of the IRC style's sender column for `rows`: room for the longest sender's name, so short names do not
/// leave a gap after the time. `cache` keeps the answer while the count of rows stays the same.
pub fn irc_nick_width(rows: &[Row], cache: &std::cell::Cell<(usize, f32)>) -> f32 {
  let (count, width) = cache.get();
  if count == rows.len() && width > 0. {
    return width;
  }
  let (least, most) = IRC_NICK_CHARS;
  let chars = rows
    .iter()
    .filter(|row| !row.msg.system)
    .map(|row| row.msg.sender_name.chars().count())
    .max()
    .unwrap_or(least)
    .clamp(least, most);
  #[expect(clippy::cast_precision_loss, reason = "at most 14 characters")]
  let width = chars as f32 * IRC_CHAR;
  cache.set((rows.len(), width));
  width
}

/// A row in the IRC style, as WeeChat draws a buffer: each message on its own lines of time, sender and text in the
/// monospaced font, the clock time right-aligned in a column as wide as the longest time (`irc_time_width`), the
/// senders right-aligned against a rule that runs down the list, and wrapped text hanging under the text column. No
/// avatars, no bubbles, no runs: every row names its sender and time. What the message shows after its text
/// (attachments, cards, reactions, the thread footer) stacks under the text in the same column.
fn irc_row(row: &Row, ctx: &MsgCtx, cx: &App) -> AnyElement {
  let msg = &row.msg;
  let focused = ctx.focus == Some(msg.id.as_str());
  let mut out = v_flex()
    .w_full()
    .font_family(crate::theme::mono())
    .text_size(px(13.))
    .line_height(px(18.))
    .when(focused, |row| row.bg(rgb(ACCENT_BG)));
  if let Some(date) = &row.date {
    out = out.child(
      h_flex()
        .w_full()
        .px(px(ROW_PAD))
        .py_1()
        .gap_2()
        .items_center()
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .child(div().w(px(24.)).h(px(1.)).flex_shrink_0().bg(rgb(BORDER)))
        .child(div().flex_shrink_0().child(date.clone()))
        .child(div().flex_1().h(px(1.)).bg(rgb(BORDER))),
    );
  }
  // A message that shows more than text (an attachment, a card, reactions) stands clear of the rows over and under
  // it. The space is inside its columns, so the rule between them still runs on unbroken.
  let blocks = !msg.attachments.is_empty() || !msg.cards.is_empty() || !msg.reactions.is_empty();
  let time = div()
    .w(px(irc_time_width(crate::clock::is_24_hour())))
    .flex_shrink_0()
    .whitespace_nowrap()
    .text_right()
    .when(blocks, gpui_kit::Styled::pt_1p5)
    .text_color(rgb(MUTED))
    .child(row.irc_time());
  let nick = div()
    .w(px(ctx.nick_width))
    .ml_1()
    .flex_shrink_0()
    .when(blocks, gpui_kit::Styled::pt_1p5)
    .text_right()
    .truncate();
  // The rule between the senders and the text, as tall as the row, so it runs on from one row to the next.
  let rule = div().w(px(1.)).flex_shrink_0().self_stretch().mx_2().bg(rgb(BORDER));
  let prepared = row.prepared();
  if msg.system {
    return out
      .child(
        h_flex()
          .w_full()
          .px(px(ROW_PAD))
          .items_start()
          .child(time)
          .child(nick.text_color(rgb(MUTED)).child("*"))
          .child(rule)
          .child(
            div()
              .flex_1()
              .min_w_0()
              .text_color(rgb(MUTED))
              .child(plain_text(row, &prepared, ctx)),
          ),
      )
      .into_any_element();
  }

  // The user's own messages carry their name, in the accent color; someone else's name opens the chat with them.
  let name = if row.own && msg.sender_name.is_empty() {
    "You".to_string()
  } else {
    msg.sender_name.clone()
  };
  let color = if row.own {
    rgb(ACCENT).into()
  } else {
    nick_color(&msg.sender_id)
  };
  let nick = nick.id((row.id.clone(), PART_NAME)).text_color(color).child(name);
  let nick = if row.own {
    nick
  } else {
    let (sender, open) = (msg.clone(), ctx.open_person.clone());
    nick
      .cursor_pointer()
      .hover(gpui_kit::Styled::underline)
      .on_click(move |_, window, cx| open(&sender.sender_id, window, cx))
  };

  let mut column = v_flex()
    .relative()
    .flex_1()
    .min_w_0()
    .items_start()
    .gap_0p5()
    .when(blocks, gpui_kit::Styled::py_1p5)
    .when(msg.pending, |d| d.opacity(0.55));
  let mut body: Vec<AnyElement> = Vec::new();
  let mut text_at = None;
  let has_text = !msg.text.trim().is_empty();
  if has_text || msg.quote.is_some() {
    // A click on the text, its quote included, opens it in the panel where it can be selected, as on a bubble.
    let mut text = v_flex()
      .id((row.id.clone(), PART_BUBBLE))
      .min_w_0()
      // No bubble for the readers' avatars to tuck into: the text keeps them a gap away (`read_mark`).
      .pr(px(READER_TUCK + 6.))
      .text_color(rgb(TEXT))
      .when(has_text, |text| {
        let msg = msg.clone();
        text.on_click(move |event, window, cx| text_panel::show(event.position(), &msg.text, window, cx))
      });
    if let Some(quote) = &prepared.irc_quote {
      text = text.child(div().max_w_full().truncate().text_color(rgb(MUTED)).child(quote.clone()));
    }
    if has_text {
      let rich = rich_text(row, &prepared, ctx);
      // The mark has a line of its own under the text, which keeps the text free to wrap at the column's edge.
      text = text.child(rich);
      if msg.edited {
        text = text.child(div().text_color(rgb(MUTED)).child("(edited)"));
      }
    }
    text_at = Some(body.len());
    body.push(text.into_any_element());
  }
  let (attachment_at, card_at) = push_extras(&mut body, row, BUBBLE, ctx, cx);
  column = place_readers(column, body, [text_at, attachment_at, card_at], false, row, ctx, cx);
  // Over the right end of the row above, clear of the sender and the start of the text.
  if let Some(tools) = floating_tools(row, ctx) {
    column = column.child(tools.top(px(-26.)).right(px(12.)));
  }

  let (act, entered) = (ctx.act.clone(), msg.clone());
  let out = out.child(
    h_flex()
      .id((row.id.clone(), PART_ROW))
      .w_full()
      .px(px(ROW_PAD))
      .items_start()
      .on_hover(move |hovered, window, cx| {
        if *hovered {
          act(Act::Hover, &entered, window, cx);
        }
      })
      .child(time)
      .child(nick)
      .child(rule)
      .child(column),
  );
  match (row.thread, ctx.open_thread) {
    (Some(info), None) => out.child(thread_divider(info)).into_any_element(),
    _ => out.into_any_element(),
  }
}

/// Where on the color wheel a sender's name sits in the IRC style, from 0 up to 1: the same for the same id on every
/// row and every run (FNV-1a over its bytes), so a person keeps their color.
fn nick_hue(id: &str) -> f32 {
  let hash = id
    .bytes()
    .fold(0x811c_9dc5_u32, |hash, byte| (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193));
  f32::from(u16::try_from(hash % 360).unwrap_or_default()) / 360.
}

/// A sender's name color in the IRC style: their hue at the accent's saturation and lightness, which the theme tunes
/// to read on its surfaces.
fn nick_color(id: &str) -> Hsla {
  Hsla {
    h: nick_hue(id),
    ..Hsla::from(rgb(ACCENT))
  }
}

/// The emoji offered by a message's "Add reaction" button.
const QUICK_EMOJI: [&str; 10] = ["👍", "❤️", "😂", "😮", "😢", "🙏", "🎉", "✅", "👀", "🚀"];

fn tool(id: ElementId, name: Icon) -> Stateful<Div> {
  div()
    .id(id)
    .size(px(26.))
    .flex()
    .items_center()
    .justify_center()
    .rounded_full()
    .cursor_pointer()
    .hover(|s| s.bg(rgb(ROW_HOVER)))
    .child(icon(name, 15., MUTED))
}

/// A tooltip with this text, over a button or a group of avatars. The buttons that open a menu go without: theirs
/// would stay over it.
fn hint(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
  let text: SharedString = text.into();
  move |window, cx| Tooltip::new(text.clone()).build(window, cx)
}

/// `part` of a message with the avatars of whoever has read up to the message on its bottom corner (`readers_group`).
///
/// A row as wide as the message's column: `part` first, in a column of its own where it is as wide as it would be in
/// the message's column (it never grows or shrinks for what follows), then a holder that takes the rest, the room
/// beside it. Over the holder lies a track from `READER_TUCK` inside the part to `READER_EDGE_GAP` short of the row's
/// edge (its `ROW_PAD` less the gap), and the avatars sit in it on the part's bottom edge: next to the part while they
/// fit, their auto margin taking the room that is left, else against the far end of the track, over the part's corner.
/// Mirrored beside a bubble on the right. Every row has this, read or not, and the track is out of the layout, so
/// people reading on never change a row.
fn read_mark(part: impl IntoElement, readers: Option<Stateful<Div>>, aside: bool) -> Div {
  let reach = ROW_PAD - READER_EDGE_GAP;
  let track = readers.map(|group| {
    let track = h_flex().absolute().bottom_0();
    if aside {
      track
        .left(px(-reach))
        .right(px(-READER_TUCK))
        .justify_start()
        .child(group.ml_auto())
    } else {
      track
        .left(px(-READER_TUCK))
        .right(px(-reach))
        .justify_end()
        .child(group.mr_auto())
    }
  });
  h_flex()
    .self_stretch()
    .items_start()
    .when(aside, gpui_kit::Styled::flex_row_reverse)
    .child(
      v_flex()
        .min_w_0()
        .when(aside, gpui_kit::Styled::items_end)
        .when(!aside, gpui_kit::Styled::items_start)
        .child(part),
    )
    .child(div().relative().flex_1().min_w_0().self_stretch().children(track))
}

/// The avatars of whoever has read up to a message, `None` when nobody has: at most `MAX_SHOWN`, overlapping, then `+N`
/// in muted text, with a tooltip that names them all. Laid out from the part's corner outward (`read_mark` places
/// them): left to right, or right to left beside a bubble on the right, with the count outermost.
fn readers_group(aside: bool, row: &Row, ctx: &MsgCtx, cx: &App) -> Option<Stateful<Div>> {
  let readers = ctx.readers.filter(|readers| !readers.is_empty())?;
  let (faces, folded) = receipt_logic::shown(&readers[..]);
  let pictures = faces.iter().enumerate().map(|(ix, reader)| {
    let ring = div()
      .size(px(READER_RING))
      .flex_shrink_0()
      .flex()
      .items_center()
      .justify_center()
      .rounded_full()
      .bg(rgb(BG))
      .child(avatar(
        ctx.images,
        cx,
        reader.avatar_url.as_deref(),
        None,
        &reader.name,
        READER_FACE,
      ));
    // Each avatar after the nearest reaches back over the one before it.
    match (ix, aside) {
      (0, _) => ring,
      (_, false) => ring.ml(px(-READER_OVERLAP)),
      (_, true) => ring.mr(px(-READER_OVERLAP)),
    }
  });
  let count = (folded > 0).then(|| {
    div()
      .flex_shrink_0()
      .text_size(px(11.))
      .text_color(rgb(MUTED))
      .child(format!("+{folded}"))
  });
  let outward = |row: Div| if aside { row.flex_row_reverse() } else { row };
  let named = readers.clone();
  Some(
    outward(h_flex())
      .id((row.id.clone(), PART_READERS))
      .flex_shrink_0()
      .gap(px(3.))
      .tooltip(move |window, cx| {
        let names: Vec<&str> = named.iter().map(|reader| reader.name.as_str()).collect();
        Tooltip::new(receipt_logic::read_by(&names)).build(window, cx)
      })
      // Over the bubble, a click on it is not a click on the bubble.
      .on_click(|_, _, cx| cx.stop_propagation())
      .child(outward(h_flex()).children(pictures))
      .children(count),
  )
}

/// The reactions one click away, left of the toolbar.
fn quick_reactions(msg: &Arc<Message>, id: &SharedString, core: &Core) -> Div {
  let mut pill = h_flex()
    .flex_shrink_0()
    .p(px(2.))
    .rounded_full()
    .bg(rgb(POPOVER))
    .border_1()
    .border_color(rgb(BORDER));
  for (ix, emoji) in QUICK_EMOJI.iter().take(3).enumerate() {
    let (core, msg) = (core.clone(), msg.clone());
    pill = pill.child(
      div()
        .id((id.clone(), PART_QUICK + ix))
        .size(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .text_size(px(15.))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(ROW_HOVER)))
        .on_click(move |_, _, _| {
          let add = !msg.reactions.iter().any(|reaction| reaction.mine && reaction.emoji == *emoji);
          react(&core, &msg.group, &msg.topic_id, &msg.id, emoji, add);
        })
        .child(emoji_font::text(*emoji)),
    );
  }
  pill
}

/// The buttons that float over a message under the pointer: react, edit, reply in thread, quote, more.
fn toolbar(row: &Row, ctx: &MsgCtx) -> Div {
  let msg = &row.msg;
  let id = |part: usize| ElementId::from((row.id.clone(), part));
  let act = |what: Act| {
    let (act, msg) = (ctx.act.clone(), msg.clone());
    move |_: &ClickEvent, window: &mut Window, cx: &mut App| act(what, &msg, window, cx)
  };
  let mut tools = h_flex()
    .flex_shrink_0()
    .p(px(2.))
    .rounded_full()
    .bg(rgb(POPOVER))
    .border_1()
    .border_color(rgb(BORDER));

  let (core, reacting) = (ctx.core.clone(), msg.clone());
  tools = tools.child(tool(id(PART_REACT), Icon::SmilePlus).on_mouse_down(
    MouseButton::Left,
    move |event, window, cx| {
      // Picking one the user already gave takes it back, as clicking its chip does.
      let toggle = {
        let (core, msg) = (core.clone(), reacting.clone());
        move |emoji: &str| {
          let add = !msg.reactions.iter().any(|reaction| reaction.mine && reaction.emoji == emoji);
          react(&core, &msg.group, &msg.topic_id, &msg.id, emoji, add);
        }
      };
      let mut choices: Vec<MenuItem> = QUICK_EMOJI
        .iter()
        .map(|emoji| {
          let toggle = toggle.clone();
          MenuItem::new(*emoji, move |_, _| toggle(emoji))
        })
        .collect();
      // Every other emoji: the full picker, with search.
      let at = event.position;
      choices.push(MenuItem::new("＋", move |window, cx| {
        let toggle = toggle.clone();
        emoji_picker::show(at, Rc::new(move |emoji, _, _| toggle(emoji)), window, cx);
      }));
      menu::show_strip(event.position, choices, window, cx);
    },
  ));
  if row.own && !msg.text.trim().is_empty() {
    tools = tools.child(tool(id(PART_EDIT), Icon::Pencil).tooltip(hint("Edit")).on_click(act(Act::Edit)));
  }
  if ctx.open_thread.is_some() {
    if ctx.threads {
      tools = tools.child(
        tool(id(PART_THREAD_TOOL), Icon::Thread)
          .tooltip(hint("Reply in thread"))
          .on_click(act(Act::Thread)),
      );
    }
    tools = tools.child(
      tool(id(PART_QUOTE), Icon::Reply)
        .tooltip(hint("Quote in reply"))
        .on_click(act(Act::Quote)),
    );
  }

  let (core, own, target) = (ctx.core.clone(), row.own, msg.clone());
  tools.child(
    tool(id(PART_MORE), Icon::More).on_mouse_down(MouseButton::Left, move |event, window, cx| {
      let mut items = Vec::new();
      if !target.text.trim().is_empty() {
        let msg = target.clone();
        items.push(MenuItem::new("Copy Text", move |_, cx| {
          cx.write_to_clipboard(ClipboardItem::new_string(msg.text.clone()));
        }));
      }
      if !target.pending {
        let (core, msg) = (core.clone(), target.clone());
        let label = if msg.starred { "Unstar" } else { "Star" };
        items.push(MenuItem::new(label, move |_, _| {
          core.send(Command::Star {
            group: msg.group.clone(),
            topic_id: msg.topic_id.clone(),
            message_id: msg.id.clone(),
            starred: !msg.starred,
          });
        }));
      }
      if own {
        let (core, msg, at) = (core.clone(), target.clone(), event.position);
        // Deleting is for everyone and cannot be undone: a second menu asks first.
        items.push(MenuItem::new("Delete...", move |window, cx| {
          let (core, msg) = (core.clone(), msg.clone());
          window.defer(cx, move |window, cx| {
            let delete = MenuItem::new("Delete for everyone", move |_, _| {
              core.send(Command::Delete {
                group: msg.group.clone(),
                message_id: msg.id.clone(),
              });
            });
            menu::show(at, vec![delete, MenuItem::new("Cancel", |_, _| {})], window, cx);
          });
        }));
      }
      menu::show(event.position, items, window, cx);
    }),
  )
}

fn reply_label(count: usize) -> String {
  if count == 1 {
    "1 reply".into()
  } else {
    format!("{count} replies")
  }
}

/// `N replies · time` under a thread starter, with the small corner connector; opens the thread panel.
fn thread_footer(row: &Row, info: ThreadInfo, open: Opener) -> AnyElement {
  let topic = row.id.clone();
  h_flex()
    .id((row.id.clone(), PART_THREAD))
    .mt_1()
    .gap_2()
    .items_end()
    .cursor_pointer()
    .text_size(px(12.))
    .text_color(rgb(MUTED))
    .hover(|s| s.text_color(rgb(ACCENT)))
    .on_click(move |_, window, cx| open(&topic, window, cx))
    .child(
      div()
        .w(px(12.))
        .h(px(10.))
        .flex_shrink_0()
        .border_l_1()
        .border_b_1()
        .border_color(rgb(BORDER))
        .rounded_bl(px(6.)),
    )
    .child(div().font_weight(FontWeight::MEDIUM).child(reply_label(info.count)))
    .child(div().child("·"))
    .child(div().child(util::rel_time(info.last)))
    .into_any_element()
}

/// The `N replies` rule between a thread's starting message and its replies.
fn thread_divider(info: ThreadInfo) -> AnyElement {
  h_flex()
    .w_full()
    .px_4()
    .py_3()
    .gap_3()
    .items_center()
    .text_size(px(12.))
    .text_color(rgb(MUTED))
    .child(reply_label(info.count))
    .child(div().flex_1().h(px(1.)).bg(rgb(BORDER)))
    .into_any_element()
}

fn clip(text: &str, max: usize) -> String {
  match text.char_indices().nth(max) {
    Some((end, _)) => format!("{}…", &text[..end]),
    None => text.to_string(),
  }
}

// Text.

const fn valid(text: &str, range: &Range<usize>) -> bool {
  range.start < range.end
    && range.end <= text.len()
    && text.is_char_boundary(range.start)
    && text.is_char_boundary(range.end)
}

/// Case-insensitive substring hits in `text`, byte ranges on the original.
pub fn find_ranges(text: &str, query: &str) -> Vec<Range<usize>> {
  if query.is_empty() {
    return Vec::new();
  }
  let lower = text.to_ascii_lowercase();
  let needle = query.to_ascii_lowercase();
  let mut hits = Vec::new();
  let mut from = 0;
  while from + needle.len() <= lower.len() {
    match lower[from..].find(&needle) {
      Some(rel) => {
        let start = from + rel;
        let end = start + needle.len();
        if text.is_char_boundary(start) && text.is_char_boundary(end) {
          hits.push(start..end);
        }
        from = start + needle.len().max(1);
      },
      None => break,
    }
  }
  hits
}

fn find_style(now: bool) -> HighlightStyle {
  HighlightStyle {
    background_color: Some(rgb(if now { ACCENT } else { MARK }).into()),
    color: Some(rgb(TEXT).into()),
    ..Default::default()
  }
}

/// The find bar's hits of a message (`hits`, offsets into its text) inside its segment `seg`, in the segment's own
/// offsets. `current` is the hit the find bar is on.
fn find_highlights(
  seg: &Range<usize>,
  hits: &[Range<usize>],
  current: Option<&Range<usize>>,
) -> Vec<(Range<usize>, HighlightStyle)> {
  hits
    .iter()
    .filter_map(|range| {
      let a = range.start.max(seg.start);
      let b = range.end.min(seg.end);
      (a < b).then(|| {
        let now = current.is_some_and(|hit| hit.start == range.start && hit.end == range.end);
        ((a - seg.start)..(b - seg.start), find_style(now))
      })
    })
    .collect()
}

fn compact_highlights(
  slice: &str,
  mut highlights: Vec<(Range<usize>, HighlightStyle)>,
) -> Vec<(Range<usize>, HighlightStyle)> {
  highlights.sort_by_key(|(range, _)| range.start);
  let mut end = 0;
  highlights.retain(|(range, _)| {
    let keep = range.start >= end && slice.is_char_boundary(range.start) && slice.is_char_boundary(range.end);
    if keep {
      end = range.end;
    }
    keep
  });
  highlights
}

/// A message's text as it is drawn, worked out once (`Row::prepared`): how it breaks into blocks, and each text
/// block's padded text, styles, fonts and links. The theme's colors and the monospace family are in it, so it is made
/// again when they change. The find bar's hits go on at draw time (`Segment::draw`).
struct Prepared {
  theme: &'static Theme,
  mono: SharedString,
  blocks: Vec<Block>,
  /// The quoted message's text, clipped, as a bubble shows it.
  quote: Option<SharedString>,
  /// The quote as the IRC style's one line: `> name: text` with its whitespace folded.
  irc_quote: Option<SharedString>,
}

impl Prepared {
  fn new(msg: &Message) -> Self {
    let text = msg.text.as_str();
    let blocks = if msg.system {
      // A line of the conversation itself: its whole text, unformatted.
      vec![Block::Text(Segment::new(text, 0..text.len(), &[], 0))]
    } else {
      blocks(text, &msg.spans)
    };
    let quote = msg.quote.as_ref().map(|quote| SharedString::from(clip(&quote.text, 240)));
    let irc_quote = msg.quote.as_ref().map(|quote| {
      let quoted = quote.text.split_whitespace().collect::<Vec<_>>().join(" ");
      SharedString::from(format!("> {}: {}", quote.sender_name, clip(&quoted, 240)))
    });
    Self {
      theme: theme::current(),
      mono: theme::mono(),
      blocks,
      quote,
      irc_quote,
    }
  }

  /// Made with the theme and the monospace family in use.
  fn fresh(&self) -> bool {
    std::ptr::eq(self.theme, theme::current()) && self.mono == theme::mono()
  }
}

/// One row of a message's text.
enum Block {
  /// Text that flows.
  Text(Segment),
  /// A list item, after its bullet.
  Item(Segment),
  /// A code block.
  Code(SharedString),
}

/// A message's text split into blocks: code blocks and list items each take their own row, and the text between them
/// flows as plain segments.
fn blocks(text: &str, spans: &[Span]) -> Vec<Block> {
  let mut marked: Vec<(Range<usize>, bool)> = spans
    .iter()
    .filter(|s| matches!(s.kind, SpanKind::CodeBlock | SpanKind::ListItem) && valid(text, &s.range))
    .map(|s| (s.range.clone(), s.kind == SpanKind::ListItem))
    .collect();
  marked.sort_by_key(|(range, _)| range.start);

  let mut out = Vec::new();
  let mut pos = 0;
  let mut n = 0;
  let push_text = |out: &mut Vec<Block>, range: Range<usize>, n: &mut usize| {
    let trimmed = text[range.clone()].trim_end_matches(['\n', ' ']);
    if trimmed.is_empty() {
      return;
    }
    *n += 1;
    out.push(Block::Text(Segment::new(
      text,
      range.start..range.start + trimmed.len(),
      spans,
      *n,
    )));
  };
  for (block, item) in marked {
    if block.start < pos {
      continue;
    }
    if block.start > pos {
      push_text(&mut out, pos..block.start, &mut n);
    }
    if item {
      let line = text[block.clone()].trim_end_matches(['\n', ' ']);
      n += 1;
      out.push(Block::Item(Segment::new(text, block.start..block.start + line.len(), spans, n)));
    } else {
      out.push(Block::Code(text[block.clone()].trim_matches('\n').to_string().into()));
    }
    pos = block.end;
    // The line break that ends a block is the row itself, not a blank line before the next text.
    if text[pos..].starts_with('\n') {
      pos += 1;
    }
  }
  if pos < text.len() {
    push_text(&mut out, pos..text.len(), &mut n);
  }
  out
}

/// The message's text, block under block.
fn rich_text(row: &Row, prepared: &Prepared, ctx: &MsgCtx) -> AnyElement {
  let mut column = v_flex().gap_1().items_start().min_w_0();
  for block in &prepared.blocks {
    column = match block {
      Block::Text(segment) => column.child(segment.draw(row, ctx)),
      Block::Item(segment) => column.child(
        h_flex()
          .w_full()
          .min_w_0()
          .pl_2()
          .gap_2()
          .items_start()
          .child(div().flex_shrink_0().child("•"))
          .child(div().flex_1().min_w_0().child(segment.draw(row, ctx))),
      ),
      Block::Code(code) => column.child(
        div()
          .px_3()
          .py_2()
          .rounded(px(8.))
          .bg(rgb(CODE_BG))
          .font_family(prepared.mono.clone())
          .text_size(px(12.5))
          .child(code.clone()),
      ),
    };
  }
  column.into_any_element()
}

/// A line of the conversation itself: its text, with nothing but the find bar's hits on it.
fn plain_text(row: &Row, prepared: &Prepared, ctx: &MsgCtx) -> AnyElement {
  match prepared.blocks.first() {
    Some(Block::Text(segment)) => segment.draw(row, ctx),
    _ => StyledText::new(SharedString::default()).into_any_element(),
  }
}

/// One stretch of a message's text drawn as one text element, worked out once.
struct Segment {
  /// Its number among the message's text elements, for its element id.
  n: usize,
  /// Where it is in the message's text.
  range: Range<usize>,
  /// Its text with the pads around mentions (`pad_at`).
  shown: SharedString,
  /// The styles of its spans and the monospace runs of its code, on its own text before the pads, for laying the find
  /// bar's hits in with them.
  styles: Vec<(Range<usize>, HighlightStyle)>,
  code: Vec<(Range<usize>, SharedString)>,
  marks: Vec<Mark>,
  /// The highlights and fonts of `shown` with no find hits on it.
  highlights: Vec<(Range<usize>, HighlightStyle)>,
  fonts: Vec<(Range<usize>, SharedString)>,
  /// Where in `shown` the links and mentions are, and what a click on each opens.
  links: Vec<Range<usize>>,
  targets: Rc<[Target]>,
}

impl Segment {
  fn new(text: &str, seg: Range<usize>, spans: &[Span], n: usize) -> Self {
    let slice = &text[seg.clone()];
    let clipped: Vec<(Range<usize>, &SpanKind)> = spans
      .iter()
      .filter(|s| !matches!(s.kind, SpanKind::CodeBlock) && valid(text, &s.range))
      .filter_map(|s| {
        let a = s.range.start.max(seg.start);
        let b = s.range.end.min(seg.end);
        (a < b).then(|| ((a - seg.start)..(b - seg.start), &s.kind))
      })
      .collect();

    let mut cuts = vec![0, slice.len()];
    for (range, _) in &clipped {
      cuts.push(range.start);
      cuts.push(range.end);
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mono = theme::mono();
    let mut styles: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut code: Vec<(Range<usize>, SharedString)> = Vec::new();
    for pair in cuts.windows(2) {
      let (a, b) = (pair[0], pair[1]);
      let mut style = HighlightStyle::default();
      let mut touched = false;
      let mut monospace = false;
      for (range, kind) in &clipped {
        if range.start > a || range.end < b {
          continue;
        }
        touched = true;
        match kind {
          SpanKind::Bold => style.font_weight = Some(FontWeight::BOLD),
          SpanKind::Italic => style.font_style = Some(FontStyle::Italic),
          SpanKind::Strike => {
            style.strikethrough = Some(StrikethroughStyle { thickness: px(1.), color: None });
          },
          SpanKind::Underline => {
            style.underline = Some(UnderlineStyle {
              thickness: px(1.),
              color: None,
              wavy: false,
            });
          },
          SpanKind::Code => {
            style.background_color = Some(rgb(CODE_BG).into());
            monospace = true;
          },
          SpanKind::Link(_) => {
            style.color = Some(rgb(ACCENT).into());
            style.underline = Some(UnderlineStyle {
              thickness: px(1.),
              color: None,
              wavy: false,
            });
          },
          SpanKind::Mention(_) | SpanKind::MentionAll => {
            style.color = Some(rgb(ACCENT).into());
            style.background_color = Some(rgb(ACCENT_BG).into());
          },
          SpanKind::CodeBlock | SpanKind::ListItem => {},
        }
      }
      if touched {
        styles.push((a..b, style));
      }
      if monospace {
        code.push((a..b, mono.clone()));
      }
    }

    // What a click opens: a link's address, or the chat with the person a mention names.
    let mut links = Vec::new();
    let mut targets = Vec::new();
    for (range, kind) in &clipped {
      let target = match kind {
        SpanKind::Link(url) => Target::Url(url.clone()),
        SpanKind::Mention(user_id) => Target::Person(user_id.clone()),
        _ => continue,
      };
      links.push(range.clone());
      targets.push(target);
    }

    let marks = mention_marks(slice, &clipped);
    let shown = pad_at(slice, &marks);
    let (highlights, fonts) = laid_out(&shown, &marks, compact_highlights(slice, styles.clone()), &code);
    let links = links.iter().map(|range| remap(range, &marks)).collect();
    Self {
      n,
      range: seg,
      shown: shown.into(),
      styles,
      code,
      marks,
      highlights,
      fonts,
      links,
      targets: targets.into(),
    }
  }

  /// The segment as an element, with the find bar's hits in the message (`ctx.find`) laid over its styles.
  fn draw(&self, row: &Row, ctx: &MsgCtx) -> AnyElement {
    let found = find_highlights(&self.range, ctx.find, ctx.find_hit.as_ref());
    let styled = StyledText::new(self.shown.clone());
    let styled = if found.is_empty() {
      styled
        .with_highlights(self.highlights.iter().cloned())
        .with_font_family_overrides(self.fonts.iter().cloned())
    } else {
      let slice = row.msg.text.get(self.range.clone()).unwrap_or_default();
      let mut highlights = found;
      highlights.extend(self.styles.iter().cloned());
      let (highlights, fonts) = laid_out(&self.shown, &self.marks, compact_highlights(slice, highlights), &self.code);
      styled.with_highlights(highlights).with_font_family_overrides(fonts)
    };
    if self.links.is_empty() {
      return styled.into_any_element();
    }
    let (open_person, targets) = (ctx.open_person.clone(), self.targets.clone());
    InteractiveText::new((row.id.clone(), PART_TEXT + self.n), styled)
      // Runs before the bubble's click, its listener being added after the bubble's: stopping here keeps a click on a
      // link or a mention from also opening the text panel.
      .on_click(self.links.clone(), move |ix, window, cx| {
        cx.stop_propagation();
        match targets.get(ix) {
          Some(Target::Url(url)) => cx.open_url(url),
          Some(Target::Person(user_id)) => open_person(user_id, window, cx),
          None => {},
        }
      })
      .into_any_element()
  }
}

/// A range of a segment's own text in its padded text (`pad_at`).
fn remap(range: &Range<usize>, marks: &[Mark]) -> Range<usize> {
  shift_start(range.start, marks)..shift_end(range.end, marks)
}

/// The highlights (compacted) and code fonts of a segment's own text, moved onto its padded text `shown` and split for
/// the emoji font (`emoji_font::runs`).
fn laid_out(
  shown: &str,
  marks: &[Mark],
  highlights: Vec<(Range<usize>, HighlightStyle)>,
  code: &[(Range<usize>, SharedString)],
) -> (Vec<(Range<usize>, HighlightStyle)>, Vec<(Range<usize>, SharedString)>) {
  let highlights = highlights
    .into_iter()
    .map(|(range, style)| (remap(&range, marks), style))
    .collect();
  let fonts = code.iter().map(|(range, font)| (remap(range, marks), font.clone())).collect();
  emoji_font::runs(shown, highlights, fonts)
}

/// What clicking a span of message text opens.
enum Target {
  Url(String),
  Person(String),
}

/// Padding inside a mention's highlight, at both ends: a text run's background has no padding of its own, so
/// "@Ana @Bia" would read as one block. Narrow and non-breaking, so a name never wraps away from it.
const MENTION_PAD: &str = "\u{202F}";

/// One end of a mention in a segment: its byte offset, whether it closes the mention, and whether a letter or digit
/// touches it ("up@Freddy"), which gets a space outside the highlight. Punctuation stays put ("@Ana's", "(@Ana)").
#[derive(Clone, Copy)]
struct Mark {
  offset: usize,
  closes: bool,
  gap: bool,
}

impl Mark {
  /// What goes into the text here: the pad, with the space on the outside of the highlight.
  const fn inserted(self) -> usize {
    MENTION_PAD.len() + if self.gap { 1 } else { 0 }
  }
}

/// The ends of every mention, sorted. At one offset a closing end comes before an opening one ("@A@B").
fn mention_marks(text: &str, clipped: &[(Range<usize>, &SpanKind)]) -> Vec<Mark> {
  let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
  let mut marks: Vec<Mark> = clipped
    .iter()
    .filter(|(_, kind)| matches!(kind, SpanKind::Mention(_) | SpanKind::MentionAll))
    .flat_map(|(range, _)| {
      let before = text.get(..range.start).and_then(|s| s.chars().next_back());
      let after = text.get(range.end..).and_then(|s| s.chars().next());
      [
        Mark {
          offset: range.start,
          closes: false,
          gap: word(before),
        },
        Mark {
          offset: range.end,
          closes: true,
          gap: word(after),
        },
      ]
    })
    .collect();
  // Two mentions that touch always get one space between them, from the opening end, so their highlights never
  // read as one block, whatever the first name ends with.
  let ends: Vec<usize> = marks.iter().filter(|mark| mark.closes).map(|mark| mark.offset).collect();
  for mark in &mut marks {
    if !mark.closes && ends.contains(&mark.offset) {
      mark.gap = true;
    }
  }
  marks.sort_unstable_by_key(|mark| (mark.offset, !mark.closes));
  marks
}

/// The text with the pads and spaces inserted at each mark.
fn pad_at(text: &str, marks: &[Mark]) -> String {
  let mut out = String::with_capacity(text.len() + marks.iter().map(|mark| mark.inserted()).sum::<usize>());
  let mut last = 0;
  for mark in marks {
    out.push_str(text.get(last..mark.offset).unwrap_or_default());
    match (mark.closes, mark.gap) {
      (false, true) => {
        out.push(' ');
        out.push_str(MENTION_PAD);
      },
      (true, true) => {
        out.push_str(MENTION_PAD);
        out.push(' ');
      },
      (_, false) => out.push_str(MENTION_PAD),
    }
    last = mark.offset;
  }
  out.push_str(text.get(last..).unwrap_or_default());
  out
}

/// A range start in the padded text: after a closing end's pad and space, and after an opening end's space but
/// before its pad, so a mention's highlight takes in its pads and never a space.
fn shift_start(offset: usize, marks: &[Mark]) -> usize {
  offset
    + marks
      .iter()
      .map(|mark| match (mark.offset.cmp(&offset), mark.closes, mark.gap) {
        (std::cmp::Ordering::Less, ..) | (std::cmp::Ordering::Equal, true, _) => mark.inserted(),
        (std::cmp::Ordering::Equal, false, true) => 1,
        _ => 0,
      })
      .sum::<usize>()
}

/// A range end in the padded text: after a closing end's pad but before its space, and before an opening end.
fn shift_end(offset: usize, marks: &[Mark]) -> usize {
  offset
    + marks
      .iter()
      .map(|mark| match (mark.offset.cmp(&offset), mark.closes) {
        (std::cmp::Ordering::Less, _) => mark.inserted(),
        (std::cmp::Ordering::Equal, true) => MENTION_PAD.len(),
        _ => 0,
      })
      .sum::<usize>()
}

// Attachments.

fn fit(width: Option<u32>, height: Option<u32>, max_w: f32, max_h: f32) -> (f32, f32) {
  match (width, height) {
    (Some(w), Some(h)) if w > 0 && h > 0 => {
      let (w, h) = (w as f32, h as f32);
      let scale = (max_w / w).min(max_h / h).min(1.0);
      ((w * scale).max(48.0), (h * scale).max(48.0))
    },
    _ => (max_w * 0.76, max_h * 0.62),
  }
}

/// The widest an image attachment is drawn, in points: the viewer's stand-in while the original loads is decoded for
/// it too, so the two share one decoded thumbnail.
pub const THUMB_MAX: f32 = 420.;

/// Opens the message's attachment `i`, as a click on it does.
fn open_attachment(msg: &Message, i: usize, window: &mut Window, cx: &mut App) {
  if let Some(att) = msg.attachments.get(i) {
    viewer::open_attachment(att, window, cx);
  }
}

fn attachment_view(msg: &Arc<Message>, i: usize, id: ElementId, bubble: Tone, ctx: &MsgCtx, cx: &App) -> AnyElement {
  let Some(att) = msg.attachments.get(i) else {
    return div().into_any_element();
  };
  if att.kind == AttachmentKind::LinkPreview {
    return link_card(msg, i, att, id, bubble, ctx, cx);
  }
  if let (AttachmentKind::Image, Some(thumb)) = (att.kind, att.thumb_url.as_deref()) {
    let (w, h) = fit(att.width, att.height, THUMB_MAX, 320.);
    let (open, menu_msg, menu_core) = (msg.clone(), msg.clone(), ctx.core.clone());
    let mut frame = div()
      .id(id)
      .w(px(w))
      .h(px(h))
      .flex_shrink_0()
      .rounded(px(14.))
      .bg(rgb(PLACEHOLDER))
      .cursor_pointer()
      .on_click(move |_, window, cx| open_attachment(&open, i, window, cx))
      .on_mouse_down(MouseButton::Right, move |event, window, cx| {
        if let Some(att) = menu_msg.attachments.get(i) {
          viewer::image_menu(&menu_core, att, event.position, window, cx);
        }
      });
    if let Some(image) = ctx.images.thumb(thumb, w.max(h), ctx.scale, cx) {
      frame = frame.child(img(image).size_full().rounded(px(14.)).object_fit(ObjectFit::Cover));
    }
    return frame.into_any_element();
  }
  // A file on its way up, or one that failed to go: it opens from the disk.
  if att.url.starts_with("file://") {
    return chip(msg, i, id, false);
  }
  if att.kind == AttachmentKind::Meet {
    return meet_card(att, id);
  }
  if viewer::is_video(att) {
    return video_tile(msg, i, att, id, ctx, cx);
  }
  chip(msg, i, id, true)
}

/// A link's preview, drawn as a second bubble of its message so it reads as part of it: the page's image at its
/// own proportions, then its title and domain. The whole card opens the link; hovering shows the full address.
fn link_card(
  msg: &Arc<Message>,
  i: usize,
  att: &Attachment,
  id: ElementId,
  bubble: Tone,
  ctx: &MsgCtx,
  cx: &App,
) -> AnyElement {
  const RADIUS: f32 = 16.;
  const GROUP: &str = "link-card";
  // The image keeps its proportions, as image attachments do; the card is never narrower than its text needs.
  let (image_w, image_h) = fit(att.width, att.height, 400., 300.);
  let card_w = image_w.max(280.);
  let (open, address) = (msg.clone(), msg.clone());
  let image = att.thumb_url.as_deref().map(|thumb| {
    let (menu_msg, menu_core) = (msg.clone(), ctx.core.clone());
    let mut frame = h_flex()
      .w_full()
      .h(px(image_h))
      .justify_center()
      .rounded_t(px(RADIUS))
      .bg(rgb(PLACEHOLDER))
      .on_mouse_down(MouseButton::Right, move |event, window, cx| {
        if let Some(att) = menu_msg.attachments.get(i) {
          viewer::image_menu(&menu_core, att, event.position, window, cx);
        }
      });
    if let Some(image) = ctx.images.thumb(thumb, image_w.max(image_h), ctx.scale, cx) {
      // An explicit size: with `size_full` the image's own aspect ratio sets its height.
      frame = frame.child(
        img(image)
          .w(px(image_w))
          .h(px(image_h))
          .flex_shrink_0()
          .when(image_w >= card_w, |image| image.rounded_t(px(RADIUS))),
      );
    }
    frame
  });
  let domain = link_domain(&att.url);
  let title = (!att.name.is_empty() && att.name != domain).then(|| att.name.clone());
  v_flex()
    .id(id)
    .group(GROUP)
    .w(px(card_w))
    .flex_shrink_0()
    .rounded(px(RADIUS))
    .bg(rgb(bubble))
    .cursor_pointer()
    .tooltip(move |window, cx| {
      let url = address.attachments.get(i).map_or_default(|att| att.url.clone());
      Tooltip::new(url).build(window, cx)
    })
    .on_click(move |_, _, cx| {
      if let Some(att) = open.attachments.get(i) {
        cx.open_url(&att.url);
      }
    })
    .children(image)
    .child(
      v_flex()
        .px_3p5()
        .py_2p5()
        .gap_1()
        .children(title.map(|title| {
          div()
            .text_size(px(14.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(TEXT))
            .line_clamp(2)
            .group_hover(GROUP, |title| title.text_color(rgb(ACCENT)))
            .child(title)
        }))
        .child(
          h_flex()
            .gap_1p5()
            .items_center()
            .child(icon(Icon::ExternalLink, 12., MUTED))
            .child(
              div()
                .min_w_0()
                .text_size(px(12.))
                .text_color(rgb(MUTED))
                .truncate()
                .child(domain),
            ),
        ),
    )
    .into_any_element()
}

/// The host a link points at, without `www.`, as the card's second line.
fn link_domain(url: &str) -> String {
  let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
  let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
  host.trim_prefix("www.").to_string()
}

/// A video: poster frame (when the server has one) under a play button, like the web app. Opens the viewer.
fn video_tile(msg: &Arc<Message>, i: usize, att: &Attachment, id: ElementId, ctx: &MsgCtx, cx: &App) -> AnyElement {
  let (w, h) = fit(att.width, att.height, THUMB_MAX, 320.);
  let open = msg.clone();
  let mut tile = div()
    .id(id)
    .relative()
    .w(px(w))
    .h(px(h))
    .flex_shrink_0()
    .overflow_hidden()
    .rounded(px(14.))
    .bg(gpui_kit::rgb(0x000000))
    .cursor_pointer()
    .on_click(move |_, window, cx| open_attachment(&open, i, window, cx));
  if let Some(image) = viewer::poster_url(att).and_then(|url| ctx.images.thumb(&url, w.max(h), ctx.scale, cx)) {
    tile = tile.child(img(image).size_full().object_fit(ObjectFit::Cover));
  }
  tile
    .child(
      div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
          div()
            .size(px(52.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(rgba(0x000000a6))
            .child(icon(Icon::Play, 24., WHITE)),
        ),
    )
    .child(
      div()
        .absolute()
        .left(px(10.))
        .bottom(px(8.))
        .max_w(px(w - 20.))
        .truncate()
        .text_size(px(12.))
        .text_color(gpui_kit::rgb(0xffffff))
        .child(if att.name.is_empty() {
          "Video".to_string()
        } else {
          att.name.clone()
        }),
    )
    .into_any_element()
}

/// A Google Meet invitation: what it is, the meeting code, and a Join button. Opens the meeting.
fn meet_card(att: &Attachment, id: ElementId) -> AnyElement {
  let url = att.url.clone();
  let code = att
    .url
    .split(['?', '#'])
    .next()
    .unwrap_or_default()
    .rsplit('/')
    .next()
    .unwrap_or_default()
    .to_string();
  h_flex()
    .id(id)
    .w(px(340.))
    .gap_3()
    .p_3()
    .items_center()
    .rounded(px(14.))
    .border_1()
    .border_color(rgb(BORDER))
    .bg(rgb(PANEL))
    .cursor_pointer()
    .hover(|s| s.border_color(rgb(ACCENT)))
    .on_click(move |_, _, cx| crate::call::open(&url, cx))
    .child(
      div()
        .size(px(40.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(10.))
        .bg(rgb(ACCENT_BG))
        .child(icon(Icon::Video, 20., ACCENT)),
    )
    .child(
      v_flex()
        .flex_1()
        .min_w_0()
        .child(
          div()
            .text_size(px(14.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(TEXT))
            .child("Video meeting"),
        )
        .child(div().truncate().text_size(px(12.)).text_color(rgb(MUTED)).child(code)),
    )
    .child(
      div()
        .flex_shrink_0()
        .px_4()
        .py_1p5()
        .rounded_full()
        .bg(rgb(ACCENT))
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(BG))
        .child("Join"),
    )
    .into_any_element()
}

/// A file row: icon and name. Opens the viewer (or the browser for Drive files, Meet links and link previews).
fn chip(msg: &Arc<Message>, i: usize, id: ElementId, viewer_first: bool) -> AnyElement {
  let Some(att) = msg.attachments.get(i) else {
    return div().into_any_element();
  };
  let label = if att.name.is_empty() {
    att.url.clone()
  } else {
    att.name.clone()
  };
  let msg = msg.clone();
  h_flex()
    .id(id)
    .max_w(px(420.))
    .gap_2()
    .px_3()
    .py_2()
    .rounded(px(10.))
    .border_1()
    .border_color(rgb(BORDER))
    .cursor_pointer()
    .hover(|s| s.bg(rgb(ROW_HOVER)))
    .on_click(move |_, window, cx| {
      let Some(att) = msg.attachments.get(i) else {
        return;
      };
      if viewer_first {
        viewer::open_attachment(att, window, cx);
      } else {
        cx.open_url(&att.url);
      }
    })
    .child(icon(Icon::File, 16., MUTED))
    .child(div().text_size(px(13.)).text_color(rgb(TEXT)).truncate().child(label))
    .into_any_element()
}

/// An app card as Google Chat draws it: a bordered box, the header first, then each section with a divider above it.
fn card_view(card: &Card, msg: &Arc<Message>, ctx: &MsgCtx, cx: &App, ix: usize) -> AnyElement {
  let core = ctx.core;
  let msg_id = msg.id.as_str();
  let block = |first: bool| {
    v_flex()
      .gap_1()
      .px_4()
      .py_3()
      .when(!first, |block| block.border_t_1().border_color(rgb(BORDER)))
  };
  let mut body = v_flex()
    .max_w(px(560.))
    .rounded(px(12.))
    .border_1()
    .border_color(rgb(BORDER))
    .overflow_hidden()
    .text_size(px(13.))
    .text_color(rgb(TEXT));
  let mut first = true;
  if !card.title.is_empty() || !card.subtitle.is_empty() || card.image.is_some() {
    let mut titles = v_flex().gap_1().min_w_0();
    if !card.title.is_empty() {
      titles = titles.child(div().text_size(px(15.)).child(card.title.clone()));
    }
    if !card.subtitle.is_empty() {
      titles = titles.child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(card.subtitle.clone()));
    }
    let picture = card.image.as_deref().map(|url| {
      let frame = div().size(px(36.)).flex_shrink_0().rounded(px(8.)).bg(rgb(PLACEHOLDER));
      match ctx.images.get(url, 144, cx) {
        Some(image) => frame.child(img(image).size_full().rounded(px(8.)).object_fit(ObjectFit::Cover)),
        None => frame,
      }
    });
    body = body.child(block(first).flex_row().items_center().gap_3().children(picture).child(titles));
    first = false;
  }
  for (s, section) in card.sections.iter().enumerate() {
    let mut out = block(first);
    first = false;
    if !section.header.is_empty() {
      out = out.child(
        div()
          .text_size(px(12.))
          .font_weight(FontWeight::SEMIBOLD)
          .text_color(rgb(MUTED))
          .child(section.header.clone()),
      );
    }
    for line in &section.lines {
      let bold: Vec<(Range<usize>, HighlightStyle)> = line
        .bold
        .iter()
        .map(|range| {
          (
            range.clone(),
            HighlightStyle {
              font_weight: Some(FontWeight::BOLD),
              ..HighlightStyle::default()
            },
          )
        })
        .collect();
      let text = StyledText::new(line.text.clone()).with_highlights(bold);
      out = out.child(if line.label.is_empty() {
        div().child(text)
      } else {
        v_flex()
          .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(line.label.clone()))
          .child(text)
      });
    }
    if !section.buttons.is_empty() {
      let mut row = h_flex().flex_wrap().gap_2().pt_2();
      for (b, button) in section.buttons.iter().enumerate() {
        let pill = div()
          .id(SharedString::from(format!("{msg_id}-c{ix}-{s}-{b}")))
          .px_3()
          .py_1()
          .rounded_full()
          .border_1()
          .border_color(rgb(BORDER))
          .child(button.label.clone());
        let pressable = |pill: Stateful<Div>| {
          pill
            .text_color(rgb(ACCENT))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(ROW_HOVER)))
        };
        row = row.child(match (&button.url, &button.action) {
          (Some(url), _) => {
            let url = url.clone();
            pressable(pill).on_click(move |_, _, cx| cx.open_url(&url))
          },
          // The app answers through Google Chat, by updating this message or posting a new one.
          (None, Some(_)) => {
            let (core, msg) = (core.clone(), msg.clone());
            pressable(pill).on_click(move |_, _, _| {
              let Some(card) = msg.cards.get(ix) else {
                return;
              };
              let action = card
                .sections
                .get(s)
                .and_then(|section| section.buttons.get(b))
                .and_then(|button| button.action.clone());
              if let Some(action) = action {
                core.send(Command::ClickCard {
                  group: msg.group.clone(),
                  topic_id: msg.topic_id.clone(),
                  message_id: msg.id.clone(),
                  card_id: card.id.clone(),
                  action,
                });
              }
            })
          },
          (None, None) => pill.text_color(rgb(MUTED)),
        });
      }
      out = out.child(row);
    }
    body = body.child(out);
  }
  body.into_any_element()
}

fn reactions_view(msg: &Arc<Message>, id: &SharedString, core: &Core) -> AnyElement {
  let mut row = h_flex().flex_wrap().gap_1();
  for (i, reaction) in msg.reactions.iter().enumerate() {
    let (core, target) = (core.clone(), msg.clone());
    row = row.child(
      h_flex()
        .id((id.clone(), PART_REACTION + i))
        .gap_1()
        .px_2()
        .h(px(24.))
        .rounded_full()
        .border_1()
        .text_size(px(12.5))
        .text_color(rgb(TEXT))
        .cursor_pointer()
        .when(reaction.mine, |d| d.bg(rgb(ACCENT_BG)).border_color(rgb(ACCENT)))
        .when(!reaction.mine, |d| d.border_color(rgb(BORDER)).hover(|s| s.bg(rgb(ROW_HOVER))))
        .on_click(move |_, _, _| {
          if let Some(reaction) = target.reactions.get(i) {
            react(
              &core,
              &target.group,
              &target.topic_id,
              &target.id,
              &reaction.emoji,
              !reaction.mine,
            );
          }
        })
        .child(emoji_font::text(reaction.emoji.clone()))
        .child(reaction.count.to_string()),
    );
  }
  row.into_any_element()
}

fn react(core: &Core, group: &GroupId, topic: &str, id: &str, emoji: &str, add: bool) {
  core.send(Command::React {
    group: group.clone(),
    topic_id: topic.to_string(),
    message_id: id.to_string(),
    emoji: emoji.to_string(),
    add,
  });
}

#[cfg(test)]
mod tests {
  use super::{MENTION_PAD, find_ranges, irc_clock, link_domain, mention_marks, pad_at, shift_end, shift_start};
  use chrono::{Local, TimeZone};
  use gchat_core::SpanKind;

  #[test]
  fn irc_times_fill_their_column() {
    let at = |hour, minute| {
      Local
        .with_ymd_and_hms(2026, 11, 13, hour, minute, 0)
        .single()
        .map(|at| at.timestamp_micros())
    };
    let (morning, afternoon) = (at(9, 5), at(13, 50));
    assert_eq!(morning.map(|time| irc_clock(time, false)).as_deref(), Some("09:05 AM"));
    assert_eq!(afternoon.map(|time| irc_clock(time, false)).as_deref(), Some("01:50 PM"));
    assert_eq!(morning.map(|time| irc_clock(time, true)).as_deref(), Some("09:05"));
    assert_eq!(afternoon.map(|time| irc_clock(time, true)).as_deref(), Some("13:50"));
  }

  #[test]
  fn link_domain_is_the_host_without_www() {
    assert_eq!(link_domain("https://www.example.com/a/b?c=1"), "example.com");
    assert_eq!(link_domain("https://docs.example.org#top"), "docs.example.org");
    assert_eq!(link_domain("example.net/path"), "example.net");
  }

  /// The padded text, and each mention's text as its highlight covers it.
  fn padded(text: &str, mentions: &[std::ops::Range<usize>]) -> (String, Vec<String>) {
    let kind = SpanKind::MentionAll;
    let clipped: Vec<_> = mentions.iter().map(|range| (range.clone(), &kind)).collect();
    let marks = mention_marks(text, &clipped);
    let shown = pad_at(text, &marks);
    let covered = mentions
      .iter()
      .map(|range| {
        let range = shift_start(range.start, &marks)..shift_end(range.end, &marks);
        shown.get(range).unwrap_or_default().to_string()
      })
      .collect();
    (shown, covered)
  }

  #[test]
  fn mention_pads_stay_inside_their_highlight() {
    let p = MENTION_PAD;
    let (shown, covered) = padded("@Ana @Bia!", &[0..4, 5..9]);
    assert_eq!(shown, format!("{p}@Ana{p} {p}@Bia{p}!"));
    assert_eq!(covered, [format!("{p}@Ana{p}"), format!("{p}@Bia{p}")]);
  }

  #[test]
  fn a_word_touching_a_mention_gets_a_space_outside_the_highlight() {
    let p = MENTION_PAD;
    let (shown, covered) = padded("up@Freddy. ok@Ana's", &[2..9, 13..17]);
    assert_eq!(shown, format!("up {p}@Freddy{p}. ok {p}@Ana{p}'s"));
    assert_eq!(covered, [format!("{p}@Freddy{p}"), format!("{p}@Ana{p}")]);
  }

  #[test]
  fn touching_mentions_get_one_space_between_them() {
    let p = MENTION_PAD;
    let (shown, covered) = padded("@A@B", &[0..2, 2..4]);
    assert_eq!(shown, format!("{p}@A{p} {p}@B{p}"));
    assert_eq!(covered, [format!("{p}@A{p}"), format!("{p}@B{p}")]);
    // A name that ends in punctuation still gets the space.
    let (shown, _) = padded("@A.@B", &[0..3, 3..5]);
    assert_eq!(shown, format!("{p}@A.{p} {p}@B{p}"));
  }

  #[test]
  fn find_ranges_are_case_insensitive() {
    assert_eq!(find_ranges("Hello there", "hello"), vec![0..5]);
    assert_eq!(find_ranges("abc abc", "bc"), vec![1..3, 5..7]);
    assert_eq!(find_ranges("hello", ""), [] as [std::ops::Range<usize>; 0]);
    assert_eq!(find_ranges("café", "é"), vec![3..5]);
  }

  #[test]
  fn find_hits_are_clipped_to_each_segment() {
    // "one two three": a hit over "e two t" crosses the segments 0..4 and 4..13.
    use std::ops::Range;
    let hit = Range { start: 2, end: 9 };
    let hits = std::slice::from_ref(&hit);
    let ranges = |seg: Range<usize>, current: Option<&Range<usize>>| -> Vec<Range<usize>> {
      super::find_highlights(&seg, hits, current)
        .into_iter()
        .map(|(range, _)| range)
        .collect()
    };
    assert_eq!(ranges(0..4, None), [Range { start: 2, end: 4 }]);
    assert_eq!(ranges(4..13, Some(&hit)), [Range { start: 0, end: 5 }]);
    assert_eq!(ranges(9..13, None), Vec::<Range<usize>>::new());
  }

  #[test]
  fn clip_cuts_on_a_char_boundary() {
    assert_eq!(super::clip("ééé", 2), "éé…");
    assert_eq!(super::clip("hi", 10), "hi");
  }

  #[test]
  fn reply_label_plural() {
    assert_eq!(super::reply_label(1), "1 reply");
    assert_eq!(super::reply_label(2), "2 replies");
  }

  #[test]
  fn nick_hue_is_stable_and_on_the_wheel() {
    let ids = ["user/human/101", "user/human/202", "jpark@example.com", ""];
    for id in ids {
      let hue = super::nick_hue(id);
      assert_eq!(hue.to_bits(), super::nick_hue(id).to_bits());
      assert!((0.0..1.0).contains(&hue), "{id}: {hue}");
    }
    assert_ne!(
      super::nick_hue("user/human/101").to_bits(),
      super::nick_hue("user/human/202").to_bits()
    );
  }

  #[test]
  fn fit_keeps_aspect_and_a_floor() {
    assert_eq!(super::fit(Some(400), Some(200), 200.0, 200.0), (200.0, 100.0));
    let (w, h) = super::fit(Some(10), Some(10), 200.0, 200.0);
    assert_eq!((w, h), (48.0, 48.0));
  }

  #[test]
  fn build_rows_skips_thread_replies() {
    use super::build_rows;
    use gchat_core::{GroupId, Message};
    use std::sync::Arc;
    let msg = |id: &str, topic: &str, sender: &str, time: i64| {
      Arc::new(Message {
        id: id.into(),
        topic_id: topic.into(),
        group: GroupId::Dm("g".into()),
        sender_id: sender.into(),
        sender_name: sender.into(),
        sender_avatar: None,
        text: id.into(),
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
    };
    let rows = build_rows(
      &[msg("t", "t", "me", 1), msg("r", "t", "you", 2), msg("n", "n", "you", 3)],
      "me",
    );
    assert_eq!(rows.iter().map(|row| row.msg.id.as_str()).collect::<Vec<_>>(), vec!["t", "n"]);
    assert!(rows[0].own);
    assert!(!rows[1].own);
    assert_eq!(rows[0].thread.map(|info| info.count), Some(1));
  }

  #[test]
  fn readers_sit_on_the_newest_delivered_message_they_have_read() {
    use super::{Readers, build_rows};
    use gchat_core::{GroupId, Message, ReadReceipt};
    use std::sync::Arc;
    let message = |id: &str, time: i64| Message {
      id: id.into(),
      topic_id: id.into(),
      group: GroupId::Dm("g".into()),
      sender_id: "you".into(),
      sender_name: "you".into(),
      sender_avatar: None,
      text: id.into(),
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
    };
    let mut joined = message("joined", 40);
    joined.system = true;
    let mut sending = message("sending", 50);
    sending.pending = true;
    let rows = build_rows(
      &[
        Arc::new(message("a", 10)),
        Arc::new(message("b", 20)),
        Arc::new(joined),
        Arc::new(sending),
      ],
      "me",
    );
    let reader = |id: &str, read_time: i64| ReadReceipt {
      user_id: id.into(),
      name: id.into(),
      avatar_url: None,
      read_time,
    };
    // Jordan stopped after `a`; Riley read past everything, so she is on `b` and not on the system line or the message
    // still going out; Sam read none of it.
    let placed = Readers::place(&rows, &[reader("jordan", 15), reader("riley", 60), reader("sam", 5)]);
    let at = |id: &str| {
      placed
        .of(id)
        .into_iter()
        .flat_map(|readers| readers.iter())
        .map(|reader| reader.user_id.as_str())
        .collect::<Vec<_>>()
    };
    assert_eq!(at("a"), ["jordan"]);
    assert_eq!(at("b"), ["riley"]);
    assert_eq!(at("joined"), Vec::<&str>::new());
    assert_eq!(at("sending"), Vec::<&str>::new());
    assert!(Readers::place(&rows, &[]).of("a").is_none());
  }
}
