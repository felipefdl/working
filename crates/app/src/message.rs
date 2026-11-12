//! Rendering of one conversation row: header, bubbles with formatted text, attachments, cards, reactions.

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gchat_core::{Attachment, AttachmentKind, Card, Command, Core, GroupId, Message, Micros, Reaction, Span, SpanKind};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::emoji_picker;
use crate::images::Images;
use crate::menu::{self, MenuItem};
use crate::settings::{Density, Settings};
use crate::theme::{
  ACCENT, ACCENT_BG, BG, BORDER, BUBBLE, BUBBLE_OWN, CODE_BG, FIELD, MARK, MONO, MUTED, PANEL, PLACEHOLDER, POPOVER,
  RED, ROW_HOVER, TEXT, WHITE, rgb,
};
use crate::ui::{Icon, avatar, icon};
use crate::util;
use crate::viewer;

const FIVE_MINUTES: i64 = 5 * 60 * 1_000_000;

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
  /// Date separator shown above this message (first message of a day).
  pub date: Option<SharedString>,
  /// Starts a run: shows avatar, name and time.
  pub head: bool,
  pub own: bool,
  /// Set on a message that started a thread with replies.
  pub thread: Option<ThreadInfo>,
}

impl Row {
  pub fn same(&self, other: &Self) -> bool {
    self.head == other.head
      && self.own == other.own
      && self.thread == other.thread
      && self.date == other.date
      && (Arc::ptr_eq(&self.msg, &other.msg) || *self.msg == *other.msg)
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
  for msg in messages.iter().filter(|m| !m.is_thread_reply()) {
    let day = util::day_key(msg.time);
    let date = (day != prev_day).then(|| SharedString::from(util::date_label(msg.time)));
    let head = starts_run(rows.last(), msg, date.is_some());
    prev_day = day;
    let thread = infos.get(msg.id.as_str()).copied();
    rows.push(Row {
      msg: msg.clone(),
      date,
      head,
      own: !me_id.is_empty() && msg.sender_id == me_id,
      thread,
    });
  }
  rows
}

/// Rows of the thread panel: the starting message, then its replies oldest first. None when the starter is gone.
pub fn build_thread_rows(messages: &[Arc<Message>], topic: &str, me_id: &str) -> Option<Vec<Row>> {
  let start = messages.iter().find(|m| m.id == topic)?;
  let infos = thread_infos(messages);
  let own = |msg: &Message| !me_id.is_empty() && msg.sender_id == me_id;
  let mut rows = vec![Row {
    msg: start.clone(),
    date: None,
    head: true,
    own: own(start),
    thread: infos.get(topic).copied(),
  }];
  for msg in messages.iter().filter(|m| m.is_thread_reply() && m.topic_id == topic) {
    let head = starts_run(rows.last(), msg, false);
    rows.push(Row {
      msg: msg.clone(),
      date: None,
      head,
      own: own(msg),
      thread: None,
    });
  }
  Some(rows)
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
  /// In-conversation find query, when the find bar is open.
  pub find: Option<&'a str>,
  /// The current find hit in this message, if any.
  pub find_hit: Option<Range<usize>>,
  /// Runs what a message's toolbar asks for.
  pub act: &'a Actor,
  /// "Reply in thread" is offered: the main list of a space.
  pub threads: bool,
  /// The message under the pointer: the one that shows its toolbar.
  pub hovered: Option<&'a str>,
  /// Set while the pointer is on a toolbar, which can reach over the row above its message.
  pub on_tools: &'a Rc<Cell<bool>>,
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
          .child(marked_plain(&msg.text, ctx.find, ctx.find_hit.as_ref())),
      )
      .into_any_element();
  }

  // Comfortable layout: the user's own messages sit on the right, with only their time above.
  let aside = row.own && cx.global::<Settings>().density == Density::Comfortable;
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
  let avatar_col = (!aside).then(|| {
    div().w(px(36.)).flex_shrink_0().when(row.head, |d| {
      d.child(avatar(ctx.images, cx, avatar_url, None, &msg.sender_name, 32.))
    })
  });

  let mut content = v_flex()
    .relative()
    .when(aside, gpui_kit::Styled::items_end)
    .when(!aside, gpui_kit::Styled::items_start)
    .gap_1()
    .min_w_0()
    .when(msg.pending, |d| d.opacity(0.55));
  if row.head {
    let time = div()
      .text_size(px(11.))
      .text_color(rgb(MUTED))
      .child(util::message_time(msg.time));
    content = content.child(if aside {
      time
    } else {
      h_flex()
        .gap_2()
        .items_baseline()
        .child(
          div()
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(TEXT))
            .child(name),
        )
        .child(time)
    });
  }

  let bubble_bg = if row.own { BUBBLE_OWN } else { BUBBLE };
  let has_text = !msg.text.trim().is_empty();
  if has_text || msg.quote.is_some() {
    let mut bubble = v_flex()
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
        bubble.on_mouse_down(MouseButton::Right, move |event, window, cx| {
          let msg = msg.clone();
          let copy = MenuItem::new("Copy Text", move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(msg.text.clone()));
          });
          menu::show(event.position, vec![copy], window, cx);
        })
      });
    if let Some(quote) = &msg.quote {
      bubble = bubble.child(
        v_flex()
          .gap_0p5()
          .pl_2()
          .border_l_2()
          .border_color(rgb(MUTED))
          .text_size(px(12.5))
          .text_color(rgb(MUTED))
          .child(div().font_weight(FontWeight::SEMIBOLD).child(quote.sender_name.clone()))
          .child(div().child(clip(&quote.text, 240))),
      );
    }
    if has_text {
      bubble = bubble.child(rich_text(&msg.id, &msg.text, &msg.spans, ctx.find, ctx.find_hit.as_ref()));
      if msg.edited {
        bubble = bubble.child(div().text_size(px(11.)).text_color(rgb(MUTED)).child("(edited)"));
      }
    }
    content = content.child(bubble);
  }

  for (i, attachment) in msg.attachments.iter().enumerate() {
    let id = SharedString::from(format!("{}-a{i}", msg.id));
    content = content.child(attachment_view(attachment, id, ctx, cx));
  }
  for (i, card) in msg.cards.iter().enumerate() {
    content = content.child(card_view(card, &msg.id, i));
  }
  if !msg.reactions.is_empty() {
    content = content.child(reactions_view(&msg.reactions, msg, ctx.core));
  }
  if msg.failed {
    content = content.child(div().text_size(px(12.)).text_color(rgb(RED)).child("Not sent"));
  }
  if let (Some(info), Some(open)) = (row.thread, ctx.open_thread) {
    content = content.child(thread_footer(&msg.id, info, open.clone()));
  }

  // Floats over the top edge of the first bubble, as in Google Chat, so it can reach over the row above.
  // Only the message under the pointer has one, and it swallows the pointer: the row beneath it does not
  // count as pointed at, so two toolbars never show together. It always sits on its bubble, which on a row
  // that starts a run puts it over the sender's name while the pointer is there.
  if !msg.pending && !msg.failed && ctx.hovered == Some(msg.id.as_str()) {
    // Where the first bubble starts: below the name or time line of a row that starts a run.
    let bubble_top = match (row.head, aside) {
      (true, false) => 24.,
      (true, true) => 19.,
      (false, _) => 0.,
    };
    let (on_tools, act, left) = (ctx.on_tools.clone(), ctx.act.clone(), msg.clone());
    content = content.child(
      h_flex()
        .id(SharedString::from(format!("{}-tools", msg.id)))
        .absolute()
        .top(px(bubble_top - 26.))
        .when(aside, |tools| tools.right(px(12.)))
        .when(!aside, |tools| tools.left(px(12.)))
        .gap_1()
        .occlude()
        .on_hover(move |hovered, window, cx| {
          on_tools.set(*hovered);
          if !*hovered {
            act(Act::Leave, &left, window, cx);
          }
        })
        .child(quick_reactions(msg, ctx.core))
        .child(toolbar(row, ctx)),
    );
  }
  let (act, entered) = (ctx.act.clone(), msg.clone());
  let out = out.child(
    h_flex()
      .id(SharedString::from(format!("{}-row", msg.id)))
      .w_full()
      .max_w(px(780.))
      .px_4()
      .pt(if row.head { px(10.) } else { px(2.) })
      .gap_2()
      .items_start()
      .when(aside, gpui_kit::Styled::justify_end)
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

/// The emoji offered by a message's "Add reaction" button.
const QUICK_EMOJI: [&str; 10] = ["👍", "❤️", "😂", "😮", "😢", "🙏", "🎉", "✅", "👀", "🚀"];

fn tool(id: SharedString, name: Icon) -> Stateful<Div> {
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

/// A tooltip naming a toolbar button. The buttons that open a menu go without: theirs would stay over it.
fn hint(text: &'static str) -> impl Fn(&mut Window, &mut App) -> AnyView {
  move |window, cx| Tooltip::new(text).build(window, cx)
}

/// The reactions one click away, left of the toolbar.
fn quick_reactions(msg: &Arc<Message>, core: &Core) -> Div {
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
        .id(SharedString::from(format!("{}-q{ix}", msg.id)))
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
        .child(*emoji),
    );
  }
  pill
}

/// The buttons that float over a message under the pointer: react, edit, reply in thread, quote, more.
fn toolbar(row: &Row, ctx: &MsgCtx) -> Div {
  let msg = &row.msg;
  let id = |part: &str| SharedString::from(format!("{}-{part}", msg.id));
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
  tools = tools.child(
    tool(id("react"), Icon::SmilePlus).on_mouse_down(MouseButton::Left, move |event, window, cx| {
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
    }),
  );
  if row.own && !msg.text.trim().is_empty() {
    tools = tools.child(tool(id("edit"), Icon::Pencil).tooltip(hint("Edit")).on_click(act(Act::Edit)));
  }
  if ctx.open_thread.is_some() {
    if ctx.threads {
      tools = tools.child(
        tool(id("thread"), Icon::Thread)
          .tooltip(hint("Reply in thread"))
          .on_click(act(Act::Thread)),
      );
    }
    tools = tools.child(
      tool(id("quote"), Icon::Reply)
        .tooltip(hint("Quote in reply"))
        .on_click(act(Act::Quote)),
    );
  }

  let (core, own, target) = (ctx.core.clone(), row.own, msg.clone());
  tools.child(
    tool(id("more"), Icon::More).on_mouse_down(MouseButton::Left, move |event, window, cx| {
      let mut items = Vec::new();
      if !target.text.trim().is_empty() {
        let msg = target.clone();
        items.push(MenuItem::new("Copy Text", move |_, cx| {
          cx.write_to_clipboard(ClipboardItem::new_string(msg.text.clone()));
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
fn thread_footer(msg_id: &str, info: ThreadInfo, open: Opener) -> AnyElement {
  let topic = msg_id.to_string();
  h_flex()
    .id(SharedString::from(format!("{msg_id}-thread")))
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

fn find_highlights(
  text: &str,
  seg: Range<usize>,
  find: Option<&str>,
  current: Option<&Range<usize>>,
) -> Vec<(Range<usize>, HighlightStyle)> {
  let Some(query) = find else {
    return Vec::new();
  };
  find_ranges(text, query)
    .into_iter()
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

fn marked_plain(text: &str, find: Option<&str>, current: Option<&Range<usize>>) -> AnyElement {
  let highlights = compact_highlights(text, find_highlights(text, 0..text.len(), find, current));
  StyledText::new(text.to_string()).with_highlights(highlights).into_any_element()
}

fn rich_text(id: &str, text: &str, spans: &[Span], find: Option<&str>, current: Option<&Range<usize>>) -> AnyElement {
  let mut blocks: Vec<Range<usize>> = spans
    .iter()
    .filter(|s| matches!(s.kind, SpanKind::CodeBlock) && valid(text, &s.range))
    .map(|s| s.range.clone())
    .collect();
  blocks.sort_by_key(|r| r.start);

  let mut column = v_flex().gap_1().items_start().min_w_0();
  let mut pos = 0;
  let mut n = 0;
  let push_text = |column: Div, range: Range<usize>, n: &mut usize| -> Div {
    let slice = &text[range.clone()];
    let trimmed = slice.trim_end_matches(['\n', ' ']);
    if trimmed.is_empty() {
      return column;
    }
    let range = range.start..range.start + trimmed.len();
    *n += 1;
    column.child(text_segment(
      SharedString::from(format!("{id}-t{n}")),
      text,
      range,
      spans,
      find,
      current,
    ))
  };
  for block in blocks {
    if block.start < pos {
      continue;
    }
    if block.start > pos {
      column = push_text(column, pos..block.start, &mut n);
    }
    let code = text[block.clone()].trim_matches('\n').to_string();
    column = column.child(
      div()
        .px_3()
        .py_2()
        .rounded(px(8.))
        .bg(rgb(CODE_BG))
        .font_family(MONO)
        .text_size(px(12.5))
        .child(code),
    );
    pos = block.end;
  }
  if pos < text.len() {
    column = push_text(column, pos..text.len(), &mut n);
  }
  column.into_any_element()
}

fn text_segment(
  id: SharedString,
  text: &str,
  seg: Range<usize>,
  spans: &[Span],
  find: Option<&str>,
  current: Option<&Range<usize>>,
) -> AnyElement {
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

  let mut highlights: Vec<(Range<usize>, HighlightStyle)> = find_highlights(text, seg.clone(), find, current);
  let mut fonts: Vec<(Range<usize>, SharedString)> = Vec::new();
  for pair in cuts.windows(2) {
    let (a, b) = (pair[0], pair[1]);
    let mut style = HighlightStyle::default();
    let mut touched = false;
    let mut code = false;
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
          code = true;
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
        SpanKind::CodeBlock => {},
      }
    }
    if touched {
      highlights.push((a..b, style));
    }
    if code {
      fonts.push((a..b, SharedString::from(MONO)));
    }
  }
  let highlights = compact_highlights(slice, highlights);

  let mut link_ranges = Vec::new();
  let mut urls = Vec::new();
  for (range, kind) in &clipped {
    if let SpanKind::Link(url) = kind {
      link_ranges.push(range.clone());
      urls.push(url.clone());
    }
  }

  let styled = StyledText::new(slice.to_string())
    .with_highlights(highlights)
    .with_font_family_overrides(fonts);
  if link_ranges.is_empty() {
    styled.into_any_element()
  } else {
    InteractiveText::new(id, styled)
      .on_click(link_ranges, move |ix, _, cx| {
        if let Some(url) = urls.get(ix) {
          cx.open_url(url);
        }
      })
      .into_any_element()
  }
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

fn attachment_view(att: &Attachment, id: SharedString, ctx: &MsgCtx, cx: &App) -> AnyElement {
  let previewable = matches!(att.kind, AttachmentKind::Image | AttachmentKind::LinkPreview);
  if let (true, Some(thumb)) = (previewable, att.thumb_url.as_deref()) {
    let (w, h) = fit(att.width, att.height, 420., 320.);
    let (open, menu_att, menu_core) = (att.clone(), att.clone(), ctx.core.clone());
    let mut frame = div()
      .id(id.clone())
      .w(px(w))
      .h(px(h))
      .flex_shrink_0()
      .rounded(px(14.))
      .bg(rgb(PLACEHOLDER))
      .cursor_pointer()
      .on_click(move |_, window, cx| viewer::open_attachment(&open, window, cx))
      .on_mouse_down(MouseButton::Right, move |event, window, cx| {
        viewer::image_menu(&menu_core, &menu_att, event.position, window, cx);
      });
    if let Some(image) = ctx.images.get(thumb, 1600, cx) {
      frame = frame.child(img(image).size_full().rounded(px(14.)).object_fit(ObjectFit::Cover));
    }
    if att.kind == AttachmentKind::LinkPreview {
      let link = chip(att, SharedString::from(format!("{id}-link")), false);
      return v_flex().gap_1().items_start().child(frame).child(link).into_any_element();
    }
    return frame.into_any_element();
  }
  // A file on its way up, or one that failed to go: it opens from the disk.
  if att.url.starts_with("file://") {
    return chip(att, id, false);
  }
  if att.kind == AttachmentKind::Meet {
    return meet_card(att, id);
  }
  if viewer::is_video(att) {
    return video_tile(att, id, ctx, cx);
  }
  chip(att, id, true)
}

/// A video: poster frame (when the server has one) under a play button, like the web app. Opens the viewer.
fn video_tile(att: &Attachment, id: SharedString, ctx: &MsgCtx, cx: &App) -> AnyElement {
  let (w, h) = fit(att.width, att.height, 420., 320.);
  let open = att.clone();
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
    .on_click(move |_, window, cx| viewer::open_attachment(&open, window, cx));
  if let Some(image) = viewer::poster_url(att).and_then(|url| ctx.images.get(&url, 1600, cx)) {
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
fn meet_card(att: &Attachment, id: SharedString) -> AnyElement {
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
fn chip(att: &Attachment, id: SharedString, viewer_first: bool) -> AnyElement {
  let att = att.clone();
  let label = if att.name.is_empty() {
    att.url.clone()
  } else {
    att.name.clone()
  };
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
      if viewer_first {
        viewer::open_attachment(&att, window, cx);
      } else {
        cx.open_url(&att.url);
      }
    })
    .child(icon(Icon::File, 16., MUTED))
    .child(div().text_size(px(13.)).text_color(rgb(TEXT)).truncate().child(label))
    .into_any_element()
}

fn card_view(card: &Card, msg_id: &str, ix: usize) -> AnyElement {
  let mut body = v_flex()
    .max_w(px(480.))
    .gap_1()
    .p_3()
    .rounded(px(12.))
    .border_1()
    .border_color(rgb(BORDER))
    .text_size(px(13.))
    .text_color(rgb(TEXT));
  if !card.title.is_empty() {
    body = body.child(div().font_weight(FontWeight::SEMIBOLD).child(card.title.clone()));
  }
  if !card.subtitle.is_empty() {
    body = body.child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(card.subtitle.clone()));
  }
  for line in &card.lines {
    body = body.child(div().child(line.clone()));
  }
  if !card.links.is_empty() {
    let mut buttons = h_flex().flex_wrap().gap_2().pt_1();
    for (i, (label, url)) in card.links.iter().enumerate() {
      let url = url.clone();
      buttons = buttons.child(
        div()
          .id(SharedString::from(format!("{msg_id}-c{ix}-{i}")))
          .px_3()
          .py_1()
          .rounded_full()
          .border_1()
          .border_color(rgb(BORDER))
          .text_color(rgb(ACCENT))
          .cursor_pointer()
          .hover(|s| s.bg(rgb(ROW_HOVER)))
          .on_click(move |_, _, cx| cx.open_url(&url))
          .child(label.clone()),
      );
    }
    body = body.child(buttons);
  }
  body.into_any_element()
}

fn reactions_view(reactions: &[Reaction], msg: &Message, core: &Core) -> AnyElement {
  let mut row = h_flex().flex_wrap().gap_1();
  for (i, reaction) in reactions.iter().enumerate() {
    let (core, group, topic, id, emoji) = (
      core.clone(),
      msg.group.clone(),
      msg.topic_id.clone(),
      msg.id.clone(),
      reaction.emoji.clone(),
    );
    let add = !reaction.mine;
    row = row.child(
      h_flex()
        .id(SharedString::from(format!("{}-r{i}", msg.id)))
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
        .on_click(move |_, _, _| react(&core, &group, &topic, &id, &emoji, add))
        .child(reaction.emoji.clone())
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
  use super::find_ranges;

  #[test]
  fn find_ranges_are_case_insensitive() {
    assert_eq!(find_ranges("Hello there", "hello"), vec![0..5]);
    assert_eq!(find_ranges("abc abc", "bc"), vec![1..3, 5..7]);
    assert_eq!(find_ranges("hello", ""), [] as [std::ops::Range<usize>; 0]);
    assert_eq!(find_ranges("café", "é"), vec![3..5]);
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
}
