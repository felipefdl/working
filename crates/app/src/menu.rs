//! One context menu for the whole window: a transparent catcher plus a small popup at the pointer, with one level of
//! submenus beside it.

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use gchat_core::{Command, Core, Group, GroupId, GroupKind, Notify, Placement};

use crate::emoji_font;
use crate::keys;
use crate::settings::{Place, Settings, Sort};
use crate::store::{Section, Store};
use crate::theme::{self, ACCENT, BORDER, MUTED, POPOVER, RED, ROW_HOVER, TEXT, rgb};
use crate::ui::{Icon, icon};

const WIDTH: f32 = 200.0;
/// Width of a menu that asks before doing something: room for the sentence that says what happens.
const ASK_WIDTH: f32 = 260.0;
const ITEM_HEIGHT: f32 = 30.0;
/// The line between two groups of a list, with its margins.
const SEPARATOR_HEIGHT: f32 = 9.0;
/// Side of one cell of a strip.
const CELL: f32 = 36.0;

pub struct MenuItem {
  label: SharedString,
  run: Rc<dyn Fn(&mut Window, &mut App)>,
  icon: Option<Icon>,
  checked: bool,
  /// Drawn in red: the choice leaves, blocks, or deletes.
  danger: bool,
  /// A line above it: it starts a group of its own.
  separated: bool,
  /// Choices it opens beside the menu instead of running anything.
  submenu: Vec<Self>,
}

impl MenuItem {
  pub fn new(label: impl Into<SharedString>, run: impl Fn(&mut Window, &mut App) + 'static) -> Self {
    Self {
      label: label.into(),
      run: Rc::new(run),
      icon: None,
      checked: false,
      danger: false,
      separated: false,
      submenu: Vec::new(),
    }
  }

  /// A choice that opens `items` beside the menu, on hover or click. Submenus go one level deep.
  pub fn submenu(label: impl Into<SharedString>, items: Vec<Self>) -> Self {
    Self {
      submenu: items,
      ..Self::new(label, |_, _| {})
    }
  }

  /// A leading icon.
  pub const fn icon(mut self, icon: Icon) -> Self {
    self.icon = Some(icon);
    self
  }

  /// A trailing check: this choice is the one in use.
  pub const fn checked(mut self, checked: bool) -> Self {
    self.checked = checked;
    self
  }

  /// Drawn in red: the choice leaves, blocks, or deletes.
  pub const fn danger(mut self) -> Self {
    self.danger = true;
    self
  }

  /// A line above it: it starts a group of its own.
  pub const fn separated(mut self) -> Self {
    self.separated = true;
    self
  }
}

/// The two choices of a list's sort menu, the one in use checked. `set` receives the pick.
pub fn sort_items(current: Sort, set: impl Fn(Sort, &mut App) + 'static) -> Vec<MenuItem> {
  let set = Rc::new(set);
  Sort::ALL
    .into_iter()
    .map(|sort| {
      let set = set.clone();
      let glyph = match sort {
        Sort::Recent => Icon::History,
        Sort::Alphabetical => Icon::ArrowDownAZ,
      };
      MenuItem::new(sort.label(), move |_, cx| set(sort, cx))
        .icon(glyph)
        .checked(sort == current)
    })
    .collect()
}

/// What a right-click at `at` on a conversation row offers. Every list builds its menu here, so they offer the same.
pub fn conversation_items(store: &Store, group: &Group, at: Point<Pixels>, cx: &App) -> Vec<MenuItem> {
  let core = &store.core;
  let id = &group.id;
  let mut items = vec![
    if group.unread {
      send(core, "Mark as read", Command::MarkRead(id.clone()))
    } else {
      send(core, "Mark as unread", Command::MarkUnread(id.clone()))
    },
    send(
      core,
      if group.pinned { "Unpin" } else { "Pin" },
      Command::SetPinned { group: id.clone(), pinned: !group.pinned },
    ),
    send(
      core,
      if group.muted { "Unmute" } else { "Mute" },
      Command::SetMuted { group: id.clone(), muted: !group.muted },
    ),
    notify_menu(core, group),
  ];
  items.extend(move_menu(core, group, &store.custom_sections()));
  items.push(hotkey_menu(store, &Place::Conversation(id.clone()), cx));
  let link = conversation_link(id);
  items.push(MenuItem::new("Copy link", move |_, cx| {
    cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
  }));
  let mut closing = closing_items(core, group, at).into_iter();
  items.extend(closing.next().map(MenuItem::separated));
  items.extend(closing);
  items
}

/// What a number key's place is called in the Assign shortcut submenu. Nothing for a conversation that is gone.
fn place_name(store: &Store, place: &Place) -> Option<String> {
  Some(match place {
    Place::Home => "Home".into(),
    Place::Mentions => "Mentions".into(),
    Place::Starred => "Starred".into(),
    Place::Calendar => "Calendar".into(),
    Place::Meet => "Meet".into(),
    Place::Mail => "Mail".into(),
    Place::Conversation(id) => store.group(id)?.g.name.clone(),
  })
}

/// The Assign shortcut submenu of `place`: the number keys, its own checked and each taken one saying what it would
/// replace, then Remove shortcut while it has one. A pick gives `place` that key, taking it from what had it.
pub fn hotkey_menu(store: &Store, place: &Place, cx: &App) -> MenuItem {
  let settings = cx.global::<Settings>();
  let features = settings.features();
  let current = settings.hotkey_of(place);
  let mut items: Vec<MenuItem> = keys::HOTKEY_SLOTS
    .filter_map(|slot| {
      let keys = keys::text(keys::hotkey_keys(slot)?);
      // A key held by a feature that is off opens nothing, so it is not named as taken.
      let taken = settings
        .hotkeys
        .get(&slot)
        .filter(|held| *held != place && features.opens(held));
      let label = match taken.and_then(|held| place_name(store, held)) {
        Some(name) => format!("{keys} · replaces {name}"),
        None => keys,
      };
      let place = place.clone();
      Some(
        MenuItem::new(label, move |_, cx| {
          Settings::update(cx, |settings| settings.assign_hotkey(Some(slot), place.clone()));
        })
        .checked(current == Some(slot)),
      )
    })
    .collect();
  if current.is_some() {
    let place = place.clone();
    items.push(
      MenuItem::new("Remove shortcut", move |_, cx| {
        Settings::update(cx, |settings| settings.assign_hotkey(None, place.clone()));
      })
      .separated(),
    );
  }
  MenuItem::submenu("Assign shortcut", items)
}

/// A view button's right-click: its Assign shortcut submenu, at the pointer.
pub fn place_menu(handle: &Entity<Store>, place: Place) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
  let handle = handle.clone();
  move |event, window, cx| {
    cx.stop_propagation();
    let items = vec![hotkey_menu(handle.read(cx), &place, cx)];
    show(event.position, items, window, cx);
  }
}

/// A choice that sends `command`.
fn send(core: &Core, label: impl Into<SharedString>, command: Command) -> MenuItem {
  let core = core.clone();
  MenuItem::new(label, move |_, _| core.send(command.clone()))
}

/// A choice that asks first, saying `question` at `at`, and sends `command` once confirmed.
fn asking(
  core: &Core,
  label: &'static str,
  question: impl Into<SharedString>,
  command: Command,
  at: Point<Pixels>,
) -> MenuItem {
  let (core, question) = (core.clone(), question.into());
  MenuItem::new(label, move |window, cx| {
    ask(at, question.clone(), send(&core, label, command.clone()), window, cx);
  })
}

/// The notification levels a conversation offers, with Google Chat's names: only spaces have threads to tell apart.
const fn notify_levels(kind: GroupKind) -> &'static [(Notify, &'static str)] {
  match kind {
    GroupKind::Space => &[
      (Notify::All, "All"),
      (Notify::MainConversations, "Main conversations"),
      (Notify::Mentions, "Mentions only"),
      (Notify::Off, "None"),
    ],
    GroupKind::Dm | GroupKind::GroupDm | GroupKind::App => &[(Notify::All, "All"), (Notify::Off, "None")],
  }
}

/// The Notifications submenu, the level in use checked.
fn notify_menu(core: &Core, group: &Group) -> MenuItem {
  let items = notify_levels(group.kind)
    .iter()
    .map(|&(notify, label)| {
      send(core, label, Command::SetNotify { group: group.id.clone(), notify }).checked(group.notify == notify)
    })
    .collect();
  MenuItem::submenu("Notifications", items)
}

/// Where a conversation can move: each of the user's sections but the one it is in, then, from one of those, back to
/// the section of its kind. Each is a label and the section id `Command::MoveToSection` takes.
fn move_targets(kind: GroupKind, placement: &Placement, sections: &[Section]) -> Vec<(String, Option<String>)> {
  let current = match placement {
    Placement::Custom { id, .. } => Some(id.as_str()),
    Placement::ByKind | Placement::Meeting => None,
  };
  let mut targets: Vec<(String, Option<String>)> = sections
    .iter()
    .filter_map(|section| {
      if let Section::Custom { id, name } = section
        && current != Some(id.as_str())
      {
        Some((name.clone(), Some(id.clone())))
      } else {
        None
      }
    })
    .collect();
  if current.is_some() {
    targets.push((Section::of_kind(kind).title().to_string(), None));
  }
  targets
}

/// The Move conversation submenu, or nothing when there is nowhere to move it.
fn move_menu(core: &Core, group: &Group, sections: &[Section]) -> Option<MenuItem> {
  let items: Vec<MenuItem> = move_targets(group.kind, &group.placement, sections)
    .into_iter()
    .map(|(label, section)| send(core, label, Command::MoveToSection { group: group.id.clone(), section }))
    .collect();
  (!items.is_empty()).then(|| MenuItem::submenu("Move conversation", items))
}

/// Leave, Hide or Unhide, Block or Unblock, and Delete conversation, for those the conversation allows. Leave, Block,
/// and Delete ask first.
fn closing_items(core: &Core, group: &Group, at: Point<Pixels>) -> Vec<MenuItem> {
  let id = &group.id;
  let mut items = Vec::new();
  if matches!(group.kind, GroupKind::Space | GroupKind::GroupDm) {
    let question = "You can't rejoin unless someone adds you again.";
    items.push(asking(core, "Leave", question, Command::Leave(id.clone()), at));
  }
  items.push(if group.hidden {
    send(
      core,
      "Unhide conversation",
      Command::SetHidden { group: id.clone(), hidden: false },
    )
  } else {
    send(
      core,
      "Hide conversation",
      Command::SetHidden { group: id.clone(), hidden: true },
    )
  });
  let block = Command::SetBlocked {
    group: id.clone(),
    blocked: !group.blocked,
  };
  match (group.kind, group.blocked) {
    (GroupKind::Space, false) => {
      let question = "You also leave the space, and it won't show up in search.";
      items.push(asking(core, "Block", question, block, at));
    },
    (GroupKind::Dm, false) => {
      let question = format!("{} won't be able to message you directly.", group.name);
      items.push(asking(core, "Block user", question, block, at));
    },
    (GroupKind::Dm, true) => items.push(send(core, "Unblock user", block)),
    // Blocking a space left it, so there is nothing to unblock from here. Group DMs and apps have no block the web
    // client was seen sending.
    (GroupKind::Space, true) | (GroupKind::GroupDm | GroupKind::App, _) => {},
  }
  if group.can_delete {
    let question = "Deletes the conversation and all its messages for every member.";
    items.push(asking(
      core,
      "Delete conversation",
      question,
      Command::DeleteConversation(id.clone()),
      at,
    ));
  }
  items
}

/// The address Google Chat opens a conversation at.
fn conversation_link(id: &GroupId) -> String {
  match id {
    GroupId::Space(id) => format!("https://chat.google.com/room/{id}"),
    GroupId::Dm(id) => format!("https://chat.google.com/dm/{id}"),
  }
}

struct Shown {
  at: Point<Pixels>,
  items: Vec<MenuItem>,
  /// Items side by side as square cells (an emoji picker) instead of a list.
  strip: bool,
  /// How wide a list is. A strip is as wide as its items.
  width: f32,
  /// A sentence above a list's items: what the choice under it does.
  note: Option<SharedString>,
  /// The item whose submenu shows beside the list.
  open: Option<usize>,
  previous_focus: Option<FocusHandle>,
}

impl Shown {
  const fn new(at: Point<Pixels>, items: Vec<MenuItem>, width: f32) -> Self {
    Self {
      at,
      items,
      strip: false,
      width,
      note: None,
      open: None,
      previous_focus: None,
    }
  }
}

pub struct ContextMenu {
  shown: Option<Shown>,
  focus: FocusHandle,
}

/// The window's context menu, reachable from any click handler.
pub struct MenuHandle(pub Entity<ContextMenu>);

impl Global for MenuHandle {}

/// Opens the context menu at `at` (window coordinates).
pub fn show(at: Point<Pixels>, items: Vec<MenuItem>, window: &mut Window, cx: &mut App) {
  show_sized(at, WIDTH, items, window, cx);
}

/// Opens the context menu at `at`, at least `width` wide and wider when a row's label needs it: for labels that do not
/// fit the usual width.
pub fn show_sized(at: Point<Pixels>, width: f32, items: Vec<MenuItem>, window: &mut Window, cx: &mut App) {
  let width = width.max(fit_width(&items, window));
  present(Shown::new(at, items, width), window, cx);
}

/// How wide a list must be for its widest row to show whole: the frame's border, the row's margins and padding, its
/// icon, its label as `row` draws it, and its check or chevron.
fn fit_width(items: &[MenuItem], window: &Window) -> f32 {
  let font = font(theme::sans());
  items
    .iter()
    .map(|item| {
      let run = TextRun {
        len: item.label.len(),
        font: font.clone(),
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
      };
      let label = window
        .text_system()
        .shape_line(item.label.clone(), px(13.), &[run], None)
        .width()
        .as_f32();
      let icon = if item.icon.is_some() { 16. + 8. } else { 0. };
      let trailing = if item.checked || !item.submenu.is_empty() {
        8. + 16.
      } else {
        0.
      };
      2. + 8. + 24. + icon + label.ceil() + trailing
    })
    .fold(0., f32::max)
}

/// Opens a strip of short choices (emoji) at `at`.
pub fn show_strip(at: Point<Pixels>, items: Vec<MenuItem>, window: &mut Window, cx: &mut App) {
  present(
    Shown {
      strip: true,
      ..Shown::new(at, items, WIDTH)
    },
    window,
    cx,
  );
}

/// Asks before `confirm` runs: `question` says in one sentence what it does, and under it are `confirm`, in red, and
/// Cancel. Opens at `at`.
pub fn ask(at: Point<Pixels>, question: impl Into<SharedString>, confirm: MenuItem, window: &mut Window, cx: &mut App) {
  let items = vec![confirm.danger(), MenuItem::new("Cancel", |_, _| {})];
  present(
    Shown {
      note: Some(question.into()),
      ..Shown::new(at, items, ASK_WIDTH)
    },
    window,
    cx,
  );
}

fn present(shown: Shown, window: &mut Window, cx: &mut App) {
  let menu = cx.global::<MenuHandle>().0.clone();
  menu.update(cx, |menu, cx| menu.open(shown, window, cx));
}

impl ContextMenu {
  pub fn new(cx: &mut Context<Self>) -> Self {
    Self { shown: None, focus: cx.focus_handle() }
  }

  pub const fn is_open(&self) -> bool {
    self.shown.is_some()
  }

  fn open(&mut self, mut shown: Shown, window: &mut Window, cx: &mut Context<Self>) {
    if shown.items.is_empty() {
      return;
    }
    shown.previous_focus = match self.shown.take() {
      Some(old) => old.previous_focus,
      None => window.focused(cx),
    };
    self.shown = Some(shown);
    window.focus(&self.focus, cx);
    // Again once the click is over: a left mouse press moves focus to what was pressed after this runs.
    let focus = self.focus.clone();
    window.defer(cx, move |window, cx| window.focus(&focus, cx));
    cx.notify();
  }

  pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(shown) = self.shown.take() else {
      return;
    };
    if let Some(handle) = shown.previous_focus {
      window.focus(&handle, cx);
    }
    cx.notify();
  }

  /// The pointer is on item `ix` of the list: its submenu opens beside it, and any other one closes.
  fn hover(&mut self, ix: usize, cx: &mut Context<Self>) {
    let Some(shown) = &mut self.shown else {
      return;
    };
    let open = shown.items.get(ix).is_some_and(|item| !item.submenu.is_empty()).then_some(ix);
    if shown.open != open {
      shown.open = open;
      cx.notify();
    }
  }

  /// Runs item `ix` of the list, or of the submenu of item `parent`, and closes the menu.
  fn pick(&mut self, parent: Option<usize>, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
    let Some(run) = self
      .shown
      .as_ref()
      .and_then(|shown| match parent {
        Some(parent) => shown.items.get(parent)?.submenu.get(ix),
        None => shown.items.get(ix),
      })
      .map(|item| item.run.clone())
    else {
      return;
    };
    self.close(window, cx);
    // After this update ends: an item that opens another menu (Do not disturb's durations, a question before
    // leaving) updates this entity.
    window.defer(cx, move |window, cx| run(window, cx));
  }

  /// The list or strip of `shown`, its top left corner at `left`, `top`.
  fn panel(&self, shown: &Shown, left: Pixels, top: Pixels, cx: &Context<Self>) -> Div {
    let mut panel = frame(left, top)
      .when(shown.strip, gpui_kit::Styled::px_1)
      .when(!shown.strip, |panel| panel.flex_col().w(px(shown.width)))
      .when_some(shown.note.clone(), |panel, note| {
        panel.child(
          div()
            .mx_1()
            .px_3()
            .pt_1()
            .pb_2()
            .text_size(px(12.))
            .text_color(rgb(MUTED))
            .child(note),
        )
      });
    for (ix, item) in shown.items.iter().enumerate() {
      if shown.strip {
        panel = panel.child(
          div()
            .id(("menu-item", ix))
            .size(px(CELL))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .text_size(px(19.))
            .text_color(rgb(TEXT))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(ROW_HOVER)))
            .on_click(cx.listener(move |this, _, window, cx| this.pick(None, ix, window, cx)))
            .child(emoji_font::text(item.label.clone())),
        );
        continue;
      }
      let parent = !item.submenu.is_empty();
      panel = panel.when(item.separated && ix > 0, |panel| panel.child(separator())).child(
        row(item, ("menu-item", ix), shown.open == Some(ix))
          .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if *hovered {
              this.hover(ix, cx);
            }
          }))
          .on_click(cx.listener(move |this, _, window, cx| {
            if parent {
              this.hover(ix, cx);
            } else {
              this.pick(None, ix, window, cx);
            }
          })),
      );
    }
    panel
  }

  /// The open submenu, beside the row that opened it: to its right, or to its left when the window has no room.
  /// `left`, `top`, and `note` place the list it belongs to.
  fn submenu(
    &self,
    shown: &Shown,
    (left, top, note): (Pixels, Pixels, f32),
    window_size: Size<Pixels>,
    cx: &Context<Self>,
  ) -> Option<Div> {
    let parent = shown.open?;
    let items = &shown.items.get(parent)?.submenu;
    // The parent row's top: the list's padding, its note, and the rows above it.
    let row_top = top + px(4. + note + list_height(shown.items.iter().take(parent + 1)) - ITEM_HEIGHT);
    let height = list_height(items) + 8.;
    let sub_top = (row_top - px(4.)).min(window_size.height - px(height + 8.)).max(px(8.));
    let right = left + px(shown.width - 4.);
    let sub_left = if right + px(WIDTH + 8.) <= window_size.width {
      right
    } else {
      (left - px(WIDTH - 4.)).max(px(8.))
    };
    let mut panel = frame(sub_left, sub_top).flex_col().w(px(WIDTH));
    for (ix, item) in items.iter().enumerate() {
      panel = panel.when(item.separated && ix > 0, |panel| panel.child(separator())).child(
        row(item, ("submenu-item", ix), false)
          .on_click(cx.listener(move |this, _, window, cx| this.pick(Some(parent), ix, window, cx))),
      );
    }
    Some(panel)
  }
}

/// A popup's surface at `left`, `top`. Presses on it stay on it.
fn frame(left: Pixels, top: Pixels) -> Div {
  div()
    .absolute()
    .left(left)
    .top(top)
    .flex()
    .py_1()
    .rounded(px(10.))
    .bg(rgb(POPOVER))
    .border_1()
    .border_color(rgb(BORDER))
    .shadow_lg()
    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
}

/// One row of a list: its icon and label, then its check or, for a submenu, a chevron. `open` keeps it highlighted
/// while its submenu shows.
fn row(item: &MenuItem, id: impl Into<ElementId>, open: bool) -> Stateful<Div> {
  let color = if item.danger { RED } else { TEXT };
  div()
    .id(id)
    .h(px(ITEM_HEIGHT))
    .mx_1()
    .px_3()
    .gap_2()
    .flex()
    .items_center()
    .rounded(px(6.))
    .text_size(px(13.))
    .text_color(rgb(color))
    .cursor_pointer()
    .hover(|s| s.bg(rgb(ROW_HOVER)))
    .when(open, |row| row.bg(rgb(ROW_HOVER)))
    .when_some(item.icon, |row, glyph| row.child(icon(glyph, 16., color)))
    .child(div().flex_1().min_w_0().truncate().child(item.label.clone()))
    .when(item.checked, |row| row.child(icon(Icon::Check, 16., ACCENT)))
    .when(!item.submenu.is_empty(), |row| row.child(icon(Icon::ChevronRight, 14., MUTED)))
}

/// The line between two groups of a list.
fn separator() -> Div {
  div().h(px(1.)).mx_1().my_1().bg(rgb(BORDER))
}

/// The height of a list's rows and the lines between them, without the list's padding.
fn list_height<'a>(items: impl IntoIterator<Item = &'a MenuItem>) -> f32 {
  items
    .into_iter()
    .enumerate()
    .map(|(ix, item)| {
      if item.separated && ix > 0 {
        ITEM_HEIGHT + SEPARATOR_HEIGHT
      } else {
        ITEM_HEIGHT
      }
    })
    .sum()
}

/// About how tall a note is once wrapped to a list `width` wide, to keep the menu on screen: 12 px text runs near
/// 6 px a character.
fn note_height(note: &str, width: f32) -> f32 {
  let per_line = ((width - 32.) / 6.).max(1.);
  let lines = (note.chars().count() as f32 / per_line).ceil().max(1.);
  lines.mul_add(18., 12.)
}

impl Render for ContextMenu {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let Some(shown) = &self.shown else {
      return div().into_any_element();
    };
    let window_size = window.viewport_size();
    let note = shown.note.as_ref().map_or(0., |note| note_height(note, shown.width));
    let (width, height) = if shown.strip {
      ((shown.items.len() as f32).mul_add(CELL, 8.), CELL + 8.)
    } else {
      (shown.width, note + list_height(&shown.items) + 8.)
    };
    let left = shown.at.x.min(window_size.width - px(width + 8.)).max(px(8.));
    let top = shown.at.y.min(window_size.height - px(height + 8.)).max(px(8.));

    div()
      .absolute()
      .top_0()
      .left_0()
      .size_full()
      .occlude()
      .track_focus(&self.focus)
      .key_context("ContextMenu")
      .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
        if event.keystroke.key == "escape" {
          this.close(window, cx);
          cx.stop_propagation();
        }
      }))
      .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close(window, cx)))
      .on_mouse_down(MouseButton::Right, cx.listener(|this, _, window, cx| this.close(window, cx)))
      .child(self.panel(shown, left, top, cx))
      .children(self.submenu(shown, (left, top, note), window_size, cx))
      .into_any_element()
  }
}

#[cfg(test)]
mod tests {
  use gchat_core::{GroupId, GroupKind, Notify, Placement};

  use super::{conversation_link, move_targets, notify_levels};
  use crate::store::Section;

  #[test]
  fn links_a_space_as_a_room_and_a_dm_as_a_dm() {
    assert_eq!(
      conversation_link(&GroupId::Space("AAAAb1c2d3".into())),
      "https://chat.google.com/room/AAAAb1c2d3"
    );
    assert_eq!(
      conversation_link(&GroupId::Dm("x9y8z7".into())),
      "https://chat.google.com/dm/x9y8z7"
    );
  }

  #[test]
  fn spaces_offer_every_level_and_chats_all_or_none() {
    let levels = |kind| notify_levels(kind).iter().map(|(notify, _)| *notify).collect::<Vec<_>>();
    assert_eq!(
      levels(GroupKind::Space),
      [Notify::All, Notify::MainConversations, Notify::Mentions, Notify::Off]
    );
    for kind in [GroupKind::Dm, GroupKind::GroupDm, GroupKind::App] {
      assert_eq!(levels(kind), [Notify::All, Notify::Off]);
    }
  }

  fn custom(id: &str, name: &str) -> Section {
    Section::Custom { id: id.into(), name: name.into() }
  }

  #[test]
  fn moves_to_the_other_sections_and_back_to_its_kind() {
    let sections = [custom("a", "Design"), custom("b", "Launch")];
    let placed = Placement::Custom { id: "a".into(), name: "Design".into() };
    assert_eq!(
      move_targets(GroupKind::Space, &placed, &sections),
      [("Launch".to_string(), Some("b".to_string())), ("Spaces".to_string(), None)]
    );
    assert_eq!(
      move_targets(GroupKind::Dm, &Placement::ByKind, &sections),
      [
        ("Design".to_string(), Some("a".to_string())),
        ("Launch".to_string(), Some("b".to_string()))
      ]
    );
    assert_eq!(move_targets(GroupKind::App, &Placement::ByKind, &[]), Vec::new());
  }
}
