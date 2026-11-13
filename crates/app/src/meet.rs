//! Google Meet in the content panel, modeled on meet.google.com: Meetings (join with a code, a link, or a nickname, make
//! a new meeting, and see the meetings of a day from Google Calendar) and Calls (people to call). This file has the
//! view, its toolbar, and its keys; `meet_body.rs` has the two tabs and the card, and what Join accepts and which
//! meetings a day lists is plain code in `meet_logic.rs`.

use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use gchat_core::User;
use gpui_kit::component::date_picker::{DatePickerEvent, DatePickerState, DateTime};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::calendar::{TOOLBAR_HEIGHT, pill};
use crate::calendar_layout::today;
use crate::meet_logic::{Meeting, day_range, meeting_link, meetings_on, shift_week};
use crate::menu::{self, MenuItem};
use crate::new_chat::match_people;
use crate::settings::Settings;
use crate::store::{Creating, Store, View};
use crate::theme::{ACCENT, ACCENT_BG, BG, BORDER, FIELD, MUTED, PANEL, RED, TEXT, control_fill, rgb};
use crate::ui::{Icon, icon};

actions!(working, [OpenMeet]);

/// How often the view redraws, for the week strip's mark of today, and how many of those pass between two reads of the
/// day's events.
const TICK: Duration = Duration::from_secs(60);
const TICKS_PER_READ: u32 = 5;
const CODE_WIDTH: f32 = 230.0;
/// The narrowest the field gets when the toolbar is short of room.
const CODE_MIN_WIDTH: f32 = 120.0;
/// Wide enough for the longest label of the New menu.
const MENU_WIDTH: f32 = 256.0;

/// What the window root is told.
pub enum MeetEvent {
  /// "Schedule in Google Calendar": show the calendar on this day, with an editor open on a new event that has Meet.
  Schedule(NaiveDate),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
  Meetings,
  Calls,
}

impl Tab {
  const ALL: [Self; 2] = [Self::Meetings, Self::Calls];

  const fn label(self) -> &'static str {
    match self {
      Self::Meetings => "Meetings",
      Self::Calls => "Calls",
    }
  }
}

pub struct MeetView {
  pub(crate) store: Entity<Store>,
  focus: FocusHandle,
  tab: Tab,
  /// The field of the Meetings tab: a meeting code, a link, or a nickname.
  code: Entity<InputState>,
  /// The field of the Calls tab.
  pub(crate) search: Entity<InputState>,
  /// The day the list shows.
  pub(crate) day: NaiveDate,
  pub(crate) picker: Entity<DatePickerState>,
  /// The day's meetings: the store's events of that day, without those of hidden calendars and declined ones.
  pub(crate) meetings: Vec<Meeting>,
  /// The people matching the search, best first, and how many people there were to match.
  pub(crate) people: Vec<Arc<User>>,
  people_total: usize,
  pub(crate) people_scroll: UniformListScrollHandle,
  /// The person picked in the list, by user id.
  pub(crate) picked: Option<String>,
  /// Why Join did not go on, until the field changes.
  failure: Option<String>,
  /// The link of the meeting made for later, in the card that shows it.
  card: Option<String>,
  /// The link in the card was just copied.
  pub(crate) copied: bool,
  /// Meet is the view on screen.
  active: bool,
  _copied_reset: Option<Task<()>>,
  _subs: Vec<Subscription>,
  _tick: Task<()>,
}

impl EventEmitter<MeetEvent> for MeetView {}

impl MeetView {
  pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let code = cx.new(|cx| InputState::new(window, cx).placeholder("Enter a code or nickname"));
    let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search contacts"));
    let day = today();
    let picker = cx.new(|cx| {
      let mut state = DatePickerState::new(window, cx).date_format("%a, %b %-d");
      state.set_date(day, window, cx);
      state
    });
    let subs = vec![
      cx.observe_in(&store, window, |this, _, window, cx| this.sync(window, cx)),
      cx.subscribe_in(&code, window, |this, _, event: &InputEvent, window, cx| match event {
        InputEvent::Change => {
          this.failure = None;
          cx.notify();
        },
        InputEvent::PressEnter { .. } => this.join(window, cx),
        _ => {},
      }),
      cx.subscribe_in(&search, window, |this, _, event: &InputEvent, _, cx| match event {
        InputEvent::Change => this.refresh_people(cx),
        InputEvent::PressEnter { .. } => this.call_picked(cx),
        _ => {},
      }),
      cx.subscribe_in(&picker, window, |this, _, event: &DatePickerEvent, window, cx| {
        let DatePickerEvent::Change(DateTime::Single(Some(picked))) = event else {
          return;
        };
        window.focus(&this.focus, cx);
        this.go_to(picked.date(), window, cx);
      }),
    ];
    let tick = cx.spawn(async move |this, cx| {
      let mut ticks = 0_u32;
      loop {
        cx.background_executor().timer(TICK).await;
        ticks = ticks.wrapping_add(1);
        let alive = this.update(cx, |this, cx| {
          if !this.active {
            return;
          }
          if ticks.is_multiple_of(TICKS_PER_READ) {
            this.load(cx);
          }
          cx.notify();
        });
        if alive.is_err() {
          break;
        }
      }
    });
    Self {
      store,
      focus: cx.focus_handle(),
      tab: Tab::Meetings,
      code,
      search,
      day,
      picker,
      meetings: Vec::new(),
      people: Vec::new(),
      people_total: 0,
      people_scroll: UniformListScrollHandle::new(),
      picked: None,
      failure: None,
      card: None,
      copied: false,
      active: false,
      _copied_reset: None,
      _subs: subs,
      _tick: tick,
    }
  }

  /// Follows the store: loads the day when Meet comes on screen, takes its meetings, and shows the link of a meeting
  /// made for later.
  fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let shown = self.store.read(cx).view == View::Meet;
    if shown && !self.active {
      self.active = true;
      self.failure = None;
      self.go_to(today(), window, cx);
    } else if !shown && self.active {
      self.active = false;
      self.card = None;
    }
    if !self.active {
      return;
    }
    self.refresh_meetings(cx);
    if self.tab == Tab::Calls && self.people_total != self.store.read(cx).people.len() {
      self.refresh_people(cx);
    }
    if let Some(url) = self.store.update(cx, |store, _| store.take_meeting()) {
      self.card = Some(url);
      self.copied = false;
      window.focus(&self.focus, cx);
    }
    cx.notify();
  }

  /// The day's meetings from the store's events, without the calendars hidden in the Calendar view.
  fn refresh_meetings(&mut self, cx: &App) {
    let events = self.store.read(cx).meet.events.clone();
    self.meetings = meetings_on(&events, self.day, &cx.global::<Settings>().calendar_hidden);
  }

  /// Matches the people against the search text, as New chat does. With text, the best match is the one picked, so
  /// that Enter calls them; without, the pick stays while that person is listed.
  fn refresh_people(&mut self, cx: &mut Context<Self>) {
    let query = self.search.read(cx).value().trim().to_string();
    let store = self.store.read(cx);
    self.people_total = store.people.len();
    self.people = match_people(store, &query, |_| false);
    self.picked = if query.is_empty() {
      self.picked.take().filter(|id| self.people.iter().any(|user| user.id == *id))
    } else {
      self.people.first().map(|user| user.id.clone())
    };
    self.people_scroll.scroll_to_item_strict(0, ScrollStrategy::Top);
    cx.notify();
  }

  /// What takes the keyboard when Meet opens: the search field of the Calls tab, or else the view, for its keys.
  pub fn focus_handle(&self, cx: &App) -> FocusHandle {
    match self.tab {
      Tab::Meetings => self.focus.clone(),
      Tab::Calls => self.search.read(cx).focus_handle(cx),
    }
  }

  /// Closes the link card, or else gives the keyboard back from a text field of the view. Answers whether either was
  /// there to do.
  pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    let typing = self.code.read(cx).focus_handle(cx).is_focused(window)
      || self.search.read(cx).focus_handle(cx).is_focused(window);
    if self.card.take().is_none() && !typing {
      return false;
    }
    window.focus(&self.focus, cx);
    cx.notify();
    true
  }

  // Moving around.

  /// Shows another day, and asks for its meetings.
  pub(crate) fn go_to(&mut self, day: NaiveDate, window: &mut Window, cx: &mut Context<Self>) {
    self.day = day;
    self.picker.update(cx, |picker, cx| picker.set_date(day, window, cx));
    self.load(cx);
    self.refresh_meetings(cx);
    cx.notify();
  }

  pub(crate) fn step_week(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
    self.go_to(shift_week(self.day, forward), window, cx);
  }

  /// Asks for the events of the day on screen.
  fn load(&self, cx: &mut Context<Self>) {
    let (start, end) = day_range(self.day);
    self.store.update(cx, |store, cx| store.load_meet(start, end, cx));
  }

  fn set_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
    if tab == self.tab {
      return;
    }
    self.tab = tab;
    self.failure = None;
    match tab {
      Tab::Calls => {
        self.refresh_people(cx);
        self.search.update(cx, |state, cx| state.focus(window, cx));
      },
      Tab::Meetings => window.focus(&self.focus, cx),
    }
    cx.notify();
  }

  /// The field of the tab on screen takes the keyboard.
  fn focus_field(&self, window: &mut Window, cx: &mut Context<Self>) {
    let field = match self.tab {
      Tab::Meetings => &self.code,
      Tab::Calls => &self.search,
    };
    field.update(cx, |state, cx| state.focus(window, cx));
  }

  // Doing things.

  /// Joins the meeting the Join field names.
  fn join(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let text = self.code.read(cx).value();
    if text.trim().is_empty() {
      return;
    }
    if let Some(url) = meeting_link(&text) {
      crate::call::open(&url, cx);
      self.failure = None;
      self.code.update(cx, |state, cx| state.set_value("", window, cx));
    } else {
      self.failure = Some("Enter a meeting code, a link, or a nickname.".into());
    }
    cx.notify();
  }

  /// Makes a meeting of its own: the store waits for the core's answer and does with the link as `intent` says.
  fn create(&mut self, intent: Creating, cx: &mut Context<Self>) {
    self.failure = None;
    self.store.update(cx, |store, cx| store.create_meeting(intent, cx));
  }

  /// "Create a meeting for later": its link comes back in a card.
  fn create_later(&mut self, cx: &mut Context<Self>) {
    self.create(Creating::ForLater, cx);
  }

  /// "Start an instant meeting": its call opens when the link arrives.
  fn create_instant(&mut self, cx: &mut Context<Self>) {
    self.create(Creating::Instant, cx);
  }

  /// Asks the window to open the calendar's editor on a new event with Meet, on the day on screen.
  fn schedule(&mut self, cx: &mut Context<Self>) {
    cx.emit(MeetEvent::Schedule(self.day));
  }

  /// Calls the person picked.
  pub(crate) fn call_picked(&mut self, cx: &mut Context<Self>) {
    let Some(user) = self
      .picked
      .as_ref()
      .and_then(|id| self.people.iter().find(|user| user.id == *id))
      .cloned()
    else {
      return;
    };
    self.store.update(cx, |store, cx| store.call_person(&user, cx));
  }

  /// Moves the pick one person down or up the list, and shows it.
  fn move_pick(&mut self, forward: bool, cx: &mut Context<Self>) {
    let Some(last) = self.people.len().checked_sub(1) else {
      return;
    };
    let current = self
      .picked
      .as_ref()
      .and_then(|id| self.people.iter().position(|user| user.id == *id));
    let next = match (current, forward) {
      (Some(ix), true) => (ix + 1).min(last),
      (Some(ix), false) => ix.saturating_sub(1),
      (None, _) => 0,
    };
    if let Some(user) = self.people.get(next) {
      self.picked = Some(user.id.clone());
      self.people_scroll.scroll_to_item(next, ScrollStrategy::Nearest);
      cx.notify();
    }
  }

  pub(crate) fn close_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.card = None;
    window.focus(&self.focus, cx);
    cx.notify();
  }

  pub(crate) fn copy_link(&mut self, url: &str, cx: &mut Context<Self>) {
    cx.write_to_clipboard(ClipboardItem::new_string(url.to_string()));
    self.copied = true;
    self._copied_reset = Some(cx.spawn(async move |this, cx| {
      cx.background_executor().timer(Duration::from_millis(1500)).await;
      this
        .update(cx, |this, cx| {
          this.copied = false;
          cx.notify();
        })
        .ok();
    }));
    cx.notify();
  }

  // Keys.

  fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
    let stroke = &event.keystroke;
    if stroke.modifiers.platform || stroke.modifiers.control || stroke.modifiers.alt {
      return;
    }
    let key = stroke.key.as_str();
    let handled = if window.focused(cx).is_some_and(|focused| focused == self.focus) {
      let slash = key == "/" || stroke.key_char.as_deref() == Some("/");
      self.on_view_key(key, slash, window, cx)
    } else {
      self.on_search_key(key, window, cx)
    };
    if handled {
      cx.stop_propagation();
    }
  }

  /// The keys of the view while no text field has the keyboard: Escape closes the card, and `/` goes to the field
  /// unless the card is over it.
  fn on_view_key(&mut self, key: &str, slash: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
    if key == "escape" {
      return self.dismiss(window, cx);
    }
    if slash && self.card.is_none() {
      self.focus_field(window, cx);
      return true;
    }
    false
  }

  /// The arrow keys move the pick of the Calls tab while its search field has the keyboard.
  fn on_search_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
    let searching = self.tab == Tab::Calls && self.search.read(cx).focus_handle(cx).is_focused(window);
    if !searching || !matches!(key, "up" | "down") {
      return false;
    }
    self.move_pick(key == "down", cx);
    true
  }

  // Chrome.

  /// The two tabs, as a row of choices of which one is selected.
  fn tabs(&self, cx: &Context<Self>) -> Div {
    let mut tabs = h_flex().flex_shrink_0().p(px(3.)).gap_1().rounded(px(9.)).bg(control_fill());
    for tab in Tab::ALL {
      let selected = tab == self.tab;
      tabs = tabs.child(
        div()
          .id(tab.label())
          .px_3()
          .py_1()
          .rounded(px(7.))
          .text_size(px(12.))
          .text_color(rgb(if selected { TEXT } else { MUTED }))
          .cursor_pointer()
          .border_1()
          .border_color(if selected {
            rgb(ACCENT)
          } else {
            transparent_black().into()
          })
          .when(selected, |option| option.bg(rgb(ACCENT_BG)))
          .on_click(cx.listener(move |this, _, window, cx| this.set_tab(tab, window, cx)))
          .child(tab.label()),
      );
    }
    tabs
  }

  /// What the toolbar says while something is on its way or went wrong.
  fn status(&self, cx: &App) -> Option<Div> {
    let store = self.store.read(cx);
    let line = |text: String, tone| {
      div()
        .max_w(px(280.))
        .truncate()
        .text_size(px(12.))
        .text_color(rgb(tone))
        .child(text)
    };
    // Google Calendar failing to answer matters to the list of meetings, not to the people.
    let unread = store.calendar.error.as_ref().filter(|_| self.tab == Tab::Meetings);
    if let Some(error) = self.failure.as_ref().or(store.meet.error.as_ref()).or(unread) {
      return Some(line(error.clone(), RED));
    }
    if let Some(call) = &store.meet.calling {
      return Some(line(format!("Calling {}…", call.name), MUTED));
    }
    if store.meet.creating.is_some() {
      return Some(line("Creating the meeting…".into(), MUTED));
    }
    (self.tab == Tab::Meetings && !store.meet.loaded).then(|| line("Loading…".into(), MUTED))
  }

  fn toolbar(&self, cx: &Context<Self>) -> Div {
    let meetings = self.tab == Tab::Meetings;
    let busy = self.store.read(cx).meet.creating.is_some();
    h_flex()
      .h(px(TOOLBAR_HEIGHT))
      .flex_shrink_0()
      .px_4()
      .gap_2()
      .items_center()
      .child(self.tabs(cx))
      .child(div().flex_1().min_w_0().flex().justify_end().children(self.status(cx)))
      .when(meetings, |bar| {
        bar
          .child(self.code_field())
          .child(self.join_button(cx))
          .child(new_button("meet-new", cx.entity(), busy))
      })
  }

  /// The field that takes a meeting code, a link, or a nickname.
  fn code_field(&self) -> Div {
    h_flex()
      .w(px(CODE_WIDTH))
      .min_w(px(CODE_MIN_WIDTH))
      .h(px(34.))
      .px_3()
      .gap_2()
      .items_center()
      .rounded_full()
      .bg(rgb(FIELD))
      .child(icon(Icon::Link, 15., MUTED))
      .child(div().flex_1().child(Input::new(&self.code).appearance(false).bordered(false)))
  }

  /// Join: it waits for something to join.
  fn join_button(&self, cx: &Context<Self>) -> Stateful<Div> {
    let ready = !self.code.read(cx).value().trim().is_empty();
    pill("meet-join")
      .child("Join")
      .when(ready, |button| {
        button
          .text_color(rgb(ACCENT))
          .on_click(cx.listener(|this, _, window, cx| this.join(window, cx)))
      })
      .when(!ready, |button| {
        button.text_color(rgb(MUTED)).opacity(0.6).cursor_default().hover(|style| style)
      })
  }
}

/// A button filled with the accent color: an icon, then a label.
pub(crate) fn solid_button(id: &'static str, glyph: Icon, label: &'static str, enabled: bool) -> Stateful<Div> {
  h_flex()
    .id(id)
    .h(px(32.))
    .pl_2p5()
    .pr_3()
    .gap_1p5()
    .flex_shrink_0()
    .items_center()
    .rounded_full()
    .bg(rgb(ACCENT))
    .text_size(px(13.))
    .font_weight(FontWeight::MEDIUM)
    .text_color(rgb(BG))
    .when(enabled, |button| button.cursor_pointer().hover(|style| style.opacity(0.9)))
    .when(!enabled, |button| button.opacity(0.5))
    .child(icon(glyph, 15., BG))
    .child(label)
}

/// The New button, in the toolbar and in the empty list: it opens the menu of ways to make a meeting. While one is
/// being made it waits.
pub(crate) fn new_button(id: &'static str, view: Entity<MeetView>, busy: bool) -> Stateful<Div> {
  solid_button(id, Icon::Plus, "New", !busy).when(!busy, |button| {
    button.on_mouse_down(MouseButton::Left, move |event, window, cx| {
      cx.stop_propagation();
      show_new_menu(&view, event.position, window, cx);
    })
  })
}

/// The menu of the New button: a link to keep, a meeting to join now, or an event to plan in Google Calendar.
fn show_new_menu(view: &Entity<MeetView>, at: Point<Pixels>, window: &mut Window, cx: &mut App) {
  let item = |label: &'static str, glyph: Icon, run: fn(&mut MeetView, &mut Context<MeetView>)| {
    let view = view.clone();
    MenuItem::new(label, move |_, cx| view.update(cx, run)).icon(glyph)
  };
  let items = vec![
    item("Create a meeting for later", Icon::Link, MeetView::create_later),
    item("Start an instant meeting", Icon::Plus, MeetView::create_instant),
    item("Schedule in Google Calendar", Icon::Calendar, MeetView::schedule),
  ];
  menu::show_sized(at, MENU_WIDTH, items, window, cx);
}

impl Render for MeetView {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let body = match self.tab {
      Tab::Meetings => self.meetings_body(cx),
      Tab::Calls => self.calls_body(cx),
    };
    let card = self.card.clone().map(|url| self.link_card(&url, cx));
    v_flex()
      .id("meet")
      .size_full()
      .relative()
      .bg(rgb(PANEL))
      .track_focus(&self.focus)
      .key_context("Meet")
      .on_key_down(cx.listener(Self::on_key))
      .child(self.toolbar(cx))
      .child(div().flex_1().min_h_0().border_t_1().border_color(rgb(BORDER)).child(body))
      .children(card)
  }
}
