//! The status button's menu, and the small composer for a custom status.

use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Local};
use gchat_core::{Command, Connection, Core, DndFor, MyStatus, Presence};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{h_flex, v_flex};

use gpui_kit::*;

use crate::emoji_picker;
use crate::menu::{self, MenuItem};
use crate::settings::Settings;
use crate::status::{self, Mine};
use crate::theme::{ACCENT, ACCENT_BG, AMBER, BORDER, FIELD, GREEN, MUTED, POPOVER, RED, ROW_HOVER, TEXT, WHITE, rgb};

actions!(working, [OpenNotificationSettings]);

const WIDTH: f32 = 280.0;

/// How long a custom status stays, in the order the composer lists them.
const DURATIONS: [(&str, Span); 5] = [
  ("30 min", Span::For(30 * 60)),
  ("1 hour", Span::For(60 * 60)),
  ("4 hours", Span::For(4 * 60 * 60)),
  ("Today", Span::Today),
  ("This week", Span::Week),
];

/// The suggestions Google Chat offers, filled in as a starting point.
const PRESETS: [(&str, &str); 4] = [
  ("👟", "Be right back"),
  ("🚌", "Commuting"),
  ("🤒", "Out sick"),
  ("🌴", "Vacationing"),
];

#[derive(Clone, Copy)]
enum Span {
  /// Seconds from now.
  For(i64),
  /// Until the next local midnight.
  Today,
  /// Until next Monday, local midnight.
  Week,
}

/// The window's status composer, reachable from the status menu.
pub struct ComposerHandle(pub Entity<StatusComposer>);

impl Global for ComposerHandle {}

pub struct StatusComposer {
  open: bool,
  at: Point<Pixels>,
  emoji: String,
  text: Entity<InputState>,
  /// Which of `DURATIONS` is selected. One hour, as Google Chat starts.
  duration: usize,
  core: Core,
  focus: FocusHandle,
  previous_focus: Option<FocusHandle>,
}

impl StatusComposer {
  pub fn new(core: Core, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let text = cx.new(|cx| InputState::new(window, cx).placeholder("What's happening?"));
    Self {
      open: false,
      at: Point::default(),
      emoji: "😀".into(),
      text,
      duration: 1,
      core,
      focus: cx.focus_handle(),
      previous_focus: None,
    }
  }

  pub const fn is_open(&self) -> bool {
    self.open
  }

  fn open(&mut self, at: Point<Pixels>, emoji: String, text: String, window: &mut Window, cx: &mut Context<Self>) {
    self.previous_focus = if self.open {
      self.previous_focus.take()
    } else {
      window.focused(cx)
    };
    self.open = true;
    self.at = at;
    self.emoji = if emoji.is_empty() {
      "😀".into()
    } else {
      emoji
    };
    self.text.update(cx, |state, cx| state.set_value(&text, window, cx));
    window.focus(&self.focus, cx);
    cx.notify();
  }

  pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if !self.open {
      return;
    }
    self.open = false;
    if let Some(handle) = self.previous_focus.take() {
      window.focus(&handle, cx);
    }
    cx.notify();
  }

  fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let text = self.text.read(cx).value().trim().to_string();
    let emoji = if text.is_empty() && self.emoji == "😀" {
      String::new()
    } else {
      self.emoji.clone()
    };
    if text.is_empty() && emoji.is_empty() {
      return;
    }
    let until = match DURATIONS[self.duration].1 {
      Span::For(seconds) => Some(now_usec() + seconds * 1_000_000),
      Span::Today => Some(end_of_today_usec()),
      Span::Week => Some(end_of_week_usec()),
    };
    self.core.send(Command::SetStatus { emoji, text, until });
    self.close(window, cx);
  }
}

impl Render for StatusComposer {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    if !self.open {
      return div().into_any_element();
    }
    let window_size = window.viewport_size();
    let left = self.at.x.min(window_size.width - px(WIDTH + 8.)).max(px(8.));
    let top = self.at.y.min(window_size.height - px(360.)).max(px(8.));
    let duration = self.duration;
    let emoji = self.emoji.clone();

    let mut spans = h_flex().gap_1().flex_wrap();
    for (ix, (label, _)) in DURATIONS.iter().enumerate() {
      let selected = ix == duration;
      spans = spans.child(
        div()
          .id(("status-span", ix))
          .px_2()
          .py_1()
          .rounded_full()
          .text_size(px(12.))
          .cursor_pointer()
          .bg(rgb(if selected { ACCENT_BG } else { FIELD }))
          .text_color(rgb(if selected { ACCENT } else { TEXT }))
          .on_click(cx.listener(move |this, _, _, cx| {
            this.duration = ix;
            cx.notify();
          }))
          .child(*label),
      );
    }

    let mut presets = v_flex();

    for (ix, (mark, label)) in PRESETS.iter().enumerate() {
      let mark = (*mark).to_string();
      let label = (*label).to_string();
      let shown = mark.clone();
      let shown_label = label.clone();
      presets = presets.child(
        div()
          .id(("status-preset", ix))
          .h(px(28.))
          .px_2()
          .flex()
          .items_center()
          .gap_2()
          .rounded(px(6.))
          .cursor_pointer()
          .hover(|s| s.bg(rgb(ROW_HOVER)))
          .on_click(cx.listener(move |this, _, window, cx| {
            this.emoji = mark.clone();
            this.text.update(cx, |state, cx| state.set_value(&label, window, cx));
            this.save(window, cx);
          }))
          .child(div().w(px(20.)).child(shown))
          .child(shown_label),
      );
    }

    let panel = v_flex()
      .absolute()
      .left(left)
      .top(top)
      .w(px(WIDTH))
      .p_2()
      .gap_2()
      .rounded(px(12.))
      .bg(rgb(POPOVER))
      .border_1()
      .border_color(rgb(BORDER))
      .shadow_lg()
      .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
      .child(
        div()
          .px_1()
          .text_size(px(12.))
          .font_weight(FontWeight::SEMIBOLD)
          .child("Add a status"),
      )
      .child(
        h_flex()
          .gap_2()
          .items_center()
          .child(
            div()
              .id("status-emoji")
              .size(px(32.))
              .flex()
              .items_center()
              .justify_center()
              .rounded(px(8.))
              .bg(rgb(FIELD))
              .cursor_pointer()
              .text_size(px(18.))
              .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                  cx.stop_propagation();
                  let at = event.position;
                  let composer = cx.entity().downgrade();
                  emoji_picker::show(
                    at,
                    Rc::new(move |picked, _, cx| {
                      if let Some(composer) = composer.upgrade() {
                        let picked = picked.to_string();
                        composer.update(cx, |this, cx| {
                          this.emoji = picked;
                          cx.notify();
                        });
                      }
                    }),
                    window,
                    cx,
                  );
                  let _ = this;
                }),
              )
              .child(emoji),
          )
          .child(div().flex_1().child(Input::new(&self.text).appearance(false).bordered(false))),
      )
      .child(div().px_1().text_size(px(11.)).text_color(rgb(MUTED)).child("Clear after"))
      .child(spans)
      .child(presets)
      .child(
        h_flex().justify_end().gap_2().child(
          div()
            .id("status-save")
            .px_3()
            .h(px(28.))
            .flex()
            .items_center()
            .rounded(px(8.))
            .bg(rgb(ACCENT))
            .text_color(rgb(WHITE))
            .cursor_pointer()
            .on_mouse_down(
              MouseButton::Left,
              cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.save(window, cx);
              }),
            )
            .child("Save"),
        ),
      );

    div()
      .absolute()
      .top_0()
      .left_0()
      .size_full()
      .occlude()
      .track_focus(&self.focus)
      .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
        if event.keystroke.key == "escape" {
          this.close(window, cx);
          cx.stop_propagation();
        }
      }))
      .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close(window, cx)))
      .child(panel)
      .into_any_element()
  }
}

/// Opens the status menu at `at`.
pub fn show(at: Point<Pixels>, core: Core, window: &mut Window, cx: &mut App) {
  let mine = cx.try_global::<Mine>().map(|mine| mine.0.clone()).unwrap_or_default();
  let away = cx.global::<Settings>().away;
  let now = now_usec();
  let dnd = mine.dnd_on(now);
  let (emoji, text) = mine.custom(now).unwrap_or(("", ""));
  let emoji = emoji.to_string();
  let text = text.to_string();
  let editing = mine.custom(now).is_some();
  let mut items = vec![
    item(mark(!away && !dnd, "Automatic"), {
      let core = core.clone();
      move |_, cx| choose_automatic(&core, cx)
    }),
    item(mark(dnd, "Do not disturb"), {
      let core = core.clone();
      move |window, cx| show_dnd(at, core.clone(), dnd, window, cx)
    }),
    item(mark(away && !dnd, "Set as away"), {
      let core = core.clone();
      move |_, cx| choose_away(&core, cx)
    }),
    item(
      if editing {
        "Edit status"
      } else {
        "Add a status"
      },
      move |window, cx| {
        let composer = cx.global::<ComposerHandle>().0.clone();
        composer.update(cx, |composer, cx| composer.open(at, emoji.clone(), text.clone(), window, cx));
      },
    ),
  ];
  if editing {
    let core = core;
    items.push(item("Clear status", move |_, _| {
      core.send(Command::SetStatus {
        emoji: String::new(),
        text: String::new(),
        until: None,
      });
    }));
  }
  items.push(item("Notification settings", |window, cx| {
    window.dispatch_action(Box::new(OpenNotificationSettings), cx);
  }));
  menu::show(at, items, window, cx);
}

fn show_dnd(at: Point<Pixels>, core: Core, on: bool, window: &mut Window, cx: &mut App) {
  let choices = [
    ("For 30 minutes", DndFor::For(30 * 60 * 1_000_000)),
    ("For 1 hour", DndFor::For(60 * 60 * 1_000_000)),
    ("For 2 hours", DndFor::For(2 * 60 * 60 * 1_000_000)),
    ("For 4 hours", DndFor::For(4 * 60 * 60 * 1_000_000)),
    ("For 8 hours", DndFor::For(8 * 60 * 60 * 1_000_000)),
    ("For 24 hours", DndFor::For(24 * 60 * 60 * 1_000_000)),
  ];
  let mut items: Vec<MenuItem> = choices
    .into_iter()
    .map(|(label, choice)| {
      let core = core.clone();
      item(label, move |_, cx| {
        // Do not disturb replaces "set as away": when it ends, the activity status applies again.
        Settings::update(cx, |settings| settings.away = false);
        status::apply(&core, cx);
        core.send(Command::SetDnd(choice));
      })
    })
    .collect();
  if on {
    let core = core;
    items.push(item("Turn off", move |_, _| core.send(Command::SetDnd(DndFor::Off))));
  }
  menu::show(at, items, window, cx);
}

fn choose_automatic(core: &Core, cx: &mut App) {
  Settings::update(cx, |settings| settings.away = false);
  status::apply(core, cx);
  core.send(Command::SetDnd(DndFor::Off));
}

fn choose_away(core: &Core, cx: &mut App) {
  Settings::update(cx, |settings| settings.away = true);
  status::apply(core, cx);
  core.send(Command::SetDnd(DndFor::Off));
}

fn item(label: impl Into<SharedString>, run: impl Fn(&mut Window, &mut App) + 'static) -> MenuItem {
  MenuItem::new(label, run)
}

fn mark(on: bool, label: &str) -> String {
  if on {
    format!("✓ {label}")
  } else {
    label.to_string()
  }
}

/// The dot and the words the status button shows.
pub fn button(
  connection: Connection,
  mine: &MyStatus,
  away: bool,
  settings: &Settings,
) -> (crate::theme::Tone, String) {
  if connection != Connection::Online {
    return match connection {
      Connection::Connecting => (AMBER, "Connecting".into()),
      Connection::Online | Connection::Offline => (RED, "Offline".into()),
    };
  }
  let now = now_usec();
  let presence = if away {
    Presence::Away
  } else {
    status::presence(settings)
  };
  let (color, word) = if mine.dnd_on(now) {
    (RED, "Do not disturb")
  } else if presence == Presence::Active {
    (GREEN, "Active")
  } else {
    (AMBER, "Away")
  };
  let label = mine
    .custom(now)
    .map_or_else(|| word.into(), |(emoji, text)| join_status(emoji, text));
  (color, shorten(&label))
}

fn join_status(emoji: &str, text: &str) -> String {
  match (emoji.is_empty(), text.is_empty()) {
    (false, false) => format!("{emoji} {text}"),
    (false, true) => emoji.to_string(),
    _ => text.to_string(),
  }
}

fn shorten(text: &str) -> String {
  let mut out = String::new();
  for (ix, ch) in text.chars().enumerate() {
    if ix == 22 {
      out.push('…');
      break;
    }
    out.push(ch);
  }
  out
}

pub fn now_usec() -> i64 {
  SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_micros() as i64)
}

fn end_of_today_usec() -> i64 {
  let now = Local::now();
  let Some(tomorrow) = now.date_naive().succ_opt() else {
    return now_usec();
  };
  let Some(midnight) = tomorrow.and_hms_opt(0, 0, 0) else {
    return now_usec();
  };
  midnight
    .and_local_timezone(Local)
    .earliest()
    .map_or_else(now_usec, |time| time.timestamp_micros())
}

fn end_of_week_usec() -> i64 {
  let now = Local::now();
  let days = 8 - i64::from(now.weekday().number_from_monday());
  let Some(day) = now.date_naive().checked_add_days(chrono::Days::new(days as u64)) else {
    return now_usec();
  };
  let Some(midnight) = day.and_hms_opt(0, 0, 0) else {
    return now_usec();
  };
  midnight
    .and_local_timezone(Local)
    .earliest()
    .map_or_else(now_usec, |time| time.timestamp_micros())
}
