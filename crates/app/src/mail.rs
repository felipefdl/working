//! Gmail in the content panel: a toolbar with New email, the mailboxes and a search field, the threads of the mailbox
//! picked, and a reading pane with the emails of the thread open or a new-email form, beside the list when the panel
//! has room and in its place when it does not. It talks to the core through `LoadMail`, `OpenMail`, and the writes in
//! `mail_write.rs` and `mail_compose.rs`, and holds what the core answered in `Store::mail`. Names, sizes, the
//! selection's steps, and a new email's addresses are plain code in `mail_logic.rs`; the webviews that show emails'
//! HTML are in `mail_body.rs`, and their pages in `mail_page.rs`.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use gchat_core::{MailAttachment, MailMessage, MailThread, Mailbox};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::calendar::{TOOLBAR_HEIGHT, pill};
use crate::emoji_font;
use crate::home::minute_clock;
use crate::images::Images;
use crate::mail_body::{Bodies, PageEvent, other_background, serve_image};
use crate::mail_compose::{ComposeDraft, ComposeForm};
use crate::mail_logic::{
  LIST_WIDTH, attachment, empty_text, expanded, mailbox_label, mailboxes, one_line, person, recipients_line,
  senders_line, size_text, splits, step,
};
use crate::mail_write::{Archived, Draft, ReplyBox};
use crate::settings::Settings;
use crate::store::{MailData, Store, View};
use crate::theme::{
  ACCENT, ACCENT_BG, AMBER, BORDER, FIELD, MUTED, PANEL, RED, ROW_HOVER, ROW_SELECTED, TEXT, control_fill, rgb,
};
use crate::ui::{Icon, avatar, icon};
use crate::util;
use crate::viewer;

actions!(
  working,
  [
    OpenMail,
    SelectNextThread,
    SelectPreviousThread,
    OpenSelectedThread,
    CloseMailThread,
    OpenMailLabels
  ]
);

/// Height of a thread in the list: three lines of text beside the avatar.
const ROW_HEIGHT: f32 = 76.0;
/// Width of the search field, and the least it shrinks to when the toolbar is short of room.
const SEARCH_WIDTH: f32 = 240.0;
const SEARCH_MIN_WIDTH: f32 = 140.0;

/// The Mail view. The window root draws it in the content panel while `Store::view` is `View::Mail`.
pub struct MailView {
  pub(crate) store: Entity<Store>,
  pub(crate) focus: FocusHandle,
  /// The mailbox the list shows: one of the switcher's, or a search.
  pub(crate) mailbox: Mailbox,
  search: Entity<InputState>,
  /// The thread the arrow keys are on, by id: Enter opens it.
  pub(crate) selected: Option<String>,
  list_scroll: UniformListScrollHandle,
  pub(crate) reader_scroll: ScrollHandle,
  /// Emails of the open thread whose one-line or whole look a click swapped, by id.
  toggled: HashSet<String>,
  /// The thread `toggled` belongs to: another thread opening forgets them.
  toggled_for: Option<String>,
  /// Mail is the view on screen.
  active: bool,
  /// The webviews of the emails shown whole.
  pub(crate) bodies: Bodies,
  /// The labels popover is open (`mail_labels.rs`).
  pub(crate) labels_open: bool,
  /// Its filter field.
  pub(crate) label_filter: Entity<InputState>,
  /// Parents whose labels the popover's tree leaves out, by id. Kept while the app runs.
  pub(crate) label_folded: HashSet<String>,
  /// The popover's row the arrows are on.
  pub(crate) label_pick: Option<usize>,
  /// The reply being written under the thread open (`mail_write.rs`).
  pub(crate) reply: Option<ReplyBox>,
  /// Replies put aside as their threads closed, by thread, while the app runs.
  pub(crate) drafts: HashMap<String, Draft>,
  /// The new-email form (`mail_compose.rs`).
  pub(crate) compose: Option<ComposeForm>,
  /// An unsent new email kept while the app runs.
  pub(crate) compose_draft: Option<ComposeDraft>,
  /// A rewrite of the reply or the new-email body is running.
  pub(crate) rewriting: bool,
  pub(crate) rewrite_task: Option<Task<()>>,
  pub(crate) rewrite_error: Option<String>,
  pub(crate) _rewrite_error_clear: Option<Task<()>>,
  /// The thread just archived, while its Undo bar shows.
  pub(crate) archived: Option<Archived>,
  _subs: Vec<Subscription>,
  _clock: Task<()>,
  /// Hears the emails' pages: heights, the wheel, Escape, and links.
  _pages: Task<()>,
  /// Fetches the emails' images through the core.
  _images: Task<()>,
}

impl MailView {
  pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search mail"));
    let label_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter labels"));
    let subs = vec![
      cx.observe_in(&store, window, |this, _, window, cx| this.sync(window, cx)),
      cx.subscribe_in(&search, window, |this, _, event: &InputEvent, window, cx| {
        if let InputEvent::PressEnter { .. } = event {
          this.run_search(window, cx);
        }
      }),
      // The contrast button's setting (`mail_other_background`) makes every page again.
      cx.observe_global::<Settings>(|_, cx| cx.notify()),
      cx.subscribe_in(&label_filter, window, |this, _, event: &InputEvent, window, cx| match event {
        // Another filter lists other rows: the pick starts again from the first.
        InputEvent::Change => {
          this.label_pick = None;
          cx.notify();
        },
        InputEvent::PressEnter { .. } => this.pick_label(None, window, cx),
        _ => {},
      }),
    ];
    let (events, pages) = flume::unbounded::<PageEvent>();
    let pages = cx.spawn_in(window, async move |this, cx| {
      while let Ok(event) = pages.recv_async().await {
        let heard = this.update_in(cx, |this, window, cx| this.on_page_event(event, window, cx));
        if heard.is_err() {
          break;
        }
      }
    });
    let (images, requests) = flume::unbounded();
    let core = store.read(cx).core.clone();
    let images_task = cx.spawn(async move |_, cx| {
      while let Ok((url, responder)) = requests.recv_async().await {
        let core = core.clone();
        cx.spawn(async move |_| serve_image(core, url, responder).await).detach();
      }
    });
    Self {
      store,
      focus: cx.focus_handle(),
      mailbox: Mailbox::Inbox,
      search,
      selected: None,
      list_scroll: UniformListScrollHandle::new(),
      reader_scroll: ScrollHandle::new(),
      toggled: HashSet::new(),
      toggled_for: None,
      active: false,
      bodies: Bodies::new(events, images),
      labels_open: false,
      label_filter,
      label_folded: HashSet::new(),
      label_pick: None,
      reply: None,
      drafts: HashMap::new(),
      compose: None,
      compose_draft: None,
      rewriting: false,
      rewrite_task: None,
      rewrite_error: None,
      _rewrite_error_clear: None,
      archived: None,
      _subs: subs,
      // The list's times ("4 min") move on.
      _clock: minute_clock(cx),
      _pages: pages,
      _images: images_task,
    }
  }

  /// What takes the keyboard when Mail opens, for its keys.
  pub fn focus_handle(&self) -> FocusHandle {
    self.focus.clone()
  }

  /// Gives the keyboard back from the search field, or else closes the thread open, while Mail is on screen. Answers
  /// whether either was there to do.
  pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    if self.store.read(cx).view != View::Mail {
      return false;
    }
    if self.close_labels(window, cx) {
      return true;
    }
    if self.leave_compose(window, cx) {
      return true;
    }
    if self.leave_reply(window, cx) {
      return true;
    }
    if self.search.read(cx).focus_handle(cx).is_focused(window) {
      window.focus(&self.focus, cx);
      cx.notify();
      return true;
    }
    if self.store.read(cx).mail.open.is_none() {
      return false;
    }
    self.close_thread(window, cx);
    true
  }

  /// Follows the store: asks for the mailbox on screen when Mail comes back to it, forgets which emails were swapped
  /// once another thread opens, and drops the emails' webviews then and as Mail leaves the screen. The reply of the
  /// thread left is put aside, the draft of the thread opened comes back, and a reply that went closes its box.
  fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.take_sent_reply(cx);
    let store = self.store.read(cx);
    let shown = store.view == View::Mail;
    let open = store.mail.open.clone();
    // The mailbox on screen is never left with nothing held, nothing on its way, and no failure to show: whatever
    // order the core's answers took, it is asked for again (`MailLists::needs_asking`).
    if shown && store.mail.lists.needs_asking(&self.mailbox) {
      let mailbox = self.mailbox.clone();
      self.store.update(cx, |store, cx| store.load_mail(mailbox, false, cx));
    }
    self.active = shown;
    if !shown {
      self.bodies.clear();
      self.labels_open = false;
      self.stash_compose(cx);
    }
    if open != self.toggled_for {
      self.toggled.clear();
      self.toggled_for = open;
      self.bodies.clear();
      self.reader_scroll.set_offset(point(px(0.), px(0.)));
      self.stash_reply(cx);
      self.restore_reply(window, cx);
    }
    cx.notify();
  }

  /// The threads of the mailbox on screen, newest first.
  pub(crate) fn threads(&self, cx: &App) -> Arc<Vec<MailThread>> {
    self
      .store
      .read(cx)
      .mail
      .lists
      .get(&self.mailbox)
      .map_or_default(|list| list.threads.clone())
  }

  /// The list, with the Undo bar of an archive under it while it shows.
  fn list_column(&self, cx: &Context<Self>) -> AnyElement {
    v_flex()
      .size_full()
      .child(div().flex_1().min_h_0().child(self.list(cx)))
      .children(self.undo_bar(cx))
      .into_any_element()
  }

  // Doing things.

  /// Shows another mailbox from its first thread, and asks for its newest threads.
  fn show_mailbox(&mut self, mailbox: Mailbox, cx: &mut Context<Self>) {
    self.stash_compose(cx);
    self.mailbox = mailbox.clone();
    self.selected = None;
    self.archived = None;
    self.list_scroll.scroll_to_item_strict(0, ScrollStrategy::Top);
    // The thread open belongs to the list that was there.
    self.store.update(cx, |store, cx| {
      store.close_mail_thread(cx);
      store.load_mail(mailbox, false, cx);
    });
    cx.notify();
  }

  /// The signed-in user's address, empty until the core says who they are.
  fn my_email<'a>(&self, cx: &'a App) -> &'a str {
    self.store.read(cx).me.as_ref().map_or("", |me| me.email.as_str())
  }

  /// How an email's sender reads: the signed-in user by their name when the email carries none, else as `person`.
  fn sender_name(&self, from: &gchat_core::MailAddress, cx: &App) -> String {
    let me = self.store.read(cx).me.as_ref();
    match me {
      Some(me) if crate::mail_logic::is_me(from, &me.email) && from.name.trim().is_empty() && !me.name.is_empty() => {
        me.name.clone()
      },
      _ => person(from).to_string(),
    }
  }

  /// A mailbox of the switcher: the search text goes.
  pub(crate) fn pick_mailbox(&mut self, mailbox: Mailbox, window: &mut Window, cx: &mut Context<Self>) {
    self.search.update(cx, |state, cx| state.set_value("", window, cx));
    window.focus(&self.focus, cx);
    self.show_mailbox(mailbox, cx);
  }

  /// Enter in the search field: the threads that match its text, as Gmail's search box reads it. With no text, the
  /// inbox again.
  fn run_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let query = self.search.read(cx).value().trim().to_string();
    let mailbox = if query.is_empty() {
      Mailbox::Inbox
    } else {
      Mailbox::Search(query)
    };
    window.focus(&self.focus, cx);
    self.show_mailbox(mailbox, cx);
  }

  /// Asks for the mailbox on screen again, after a failure.
  fn reload(&mut self, cx: &mut Context<Self>) {
    let mailbox = self.mailbox.clone();
    self.store.update(cx, |store, cx| store.load_mail(mailbox, false, cx));
  }

  /// Asks for the threads older than those listed.
  fn load_more(&mut self, cx: &mut Context<Self>) {
    let mailbox = self.mailbox.clone();
    self.store.update(cx, |store, cx| store.load_mail(mailbox, true, cx));
  }

  pub(crate) fn open_thread(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
    self.stash_compose(cx);
    self.selected = Some(id.clone());
    window.focus(&self.focus, cx);
    self.store.update(cx, |store, cx| store.open_mail_thread(id, cx));
  }

  /// Asks for the emails of the thread open again, after a failure.
  fn reopen(&mut self, cx: &mut Context<Self>) {
    let Some(id) = self.store.read(cx).mail.open.clone() else {
      return;
    };
    self.store.update(cx, |store, cx| store.open_mail_thread(id, cx));
  }

  pub(crate) fn close_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    window.focus(&self.focus, cx);
    self.store.update(cx, Store::close_mail_thread);
  }

  /// Shows an email whole, or on one line again.
  fn toggle(&mut self, id: &str, cx: &mut Context<Self>) {
    if !self.toggled.remove(id) {
      self.toggled.insert(id.to_string());
    }
    cx.notify();
  }

  /// The emails shown whole that have HTML, by id: each has a webview while Mail is on screen.
  fn wanted_bodies(&self, cx: &App) -> Vec<String> {
    let Some(messages) = self.store.read(cx).mail.messages.as_ref().filter(|_| self.active) else {
      return Vec::new();
    };
    let last = messages.len().saturating_sub(1);
    messages
      .iter()
      .enumerate()
      .filter(|(ix, message)| expanded(*ix == last, self.toggled.contains(&message.id)))
      .filter(|(_, message)| !message.html.trim().is_empty())
      .map(|(_, message)| message.id.clone())
      .collect()
  }

  /// Scrolls the reading pane `delta` pixels down (up when negative), for the wheel turned over an email's page.
  pub(crate) fn scroll_reader(&self, delta: f32, cx: &mut Context<Self>) {
    let offset = self.reader_scroll.offset();
    let bottom = self.reader_scroll.max_offset().y.as_f32();
    let y = (offset.y.as_f32() - delta).clamp(-bottom, 0.);
    self.reader_scroll.set_offset(point(offset.x, px(y)));
    cx.notify();
  }

  // Keys.

  /// Moves the selection one thread down or up the list, and shows it.
  fn select_step(&mut self, forward: bool, cx: &mut Context<Self>) {
    let threads = self.threads(cx);
    let current = self
      .selected
      .as_ref()
      .and_then(|id| threads.iter().position(|thread| thread.id == *id));
    let Some(next) = step(current, threads.len(), forward) else {
      return;
    };
    if let Some(thread) = threads.get(next) {
      self.selected = Some(thread.id.clone());
      self.list_scroll.scroll_to_item(next, ScrollStrategy::Nearest);
      cx.notify();
    }
  }

  fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if let Some(id) = self.selected.clone() {
      self.open_thread(id, window, cx);
    }
  }

  // The toolbar.

  /// The mailboxes, as a row of choices of which the one on screen is selected. A search selects none.
  fn switcher(&self, cx: &Context<Self>) -> Div {
    let mut row = h_flex().flex_shrink_0().p(px(3.)).gap_1().rounded(px(9.)).bg(control_fill());
    for (ix, mailbox) in mailboxes().into_iter().enumerate() {
      let selected = mailbox == self.mailbox;
      let label = mailbox_label(&mailbox);
      row = row.child(
        div()
          .id(("mailbox", ix))
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
          .on_click(cx.listener(move |this, _, window, cx| this.pick_mailbox(mailbox.clone(), window, cx)))
          .child(label),
      );
    }
    row
  }

  /// What the toolbar says: why Gmail could not be read while the list on screen still shows (as when its older
  /// threads did not come), with a Retry that asks for the mailbox again, or that a search is on its way. A thread
  /// that did not open says why in its own pane.
  fn status(&self, cx: &Context<Self>) -> Option<Div> {
    let mail = &self.store.read(cx).mail;
    let list = mail.lists.get(&self.mailbox);
    let line = |text: String, tone| div().min_w_0().truncate().text_size(px(12.)).text_color(rgb(tone)).child(text);
    let reader_failed = mail.open.is_some() && mail.messages.is_none();
    let failed = list.and_then(|list| list.failed.as_ref().filter(|_| list.loaded));
    if let Some(error) = failed.or(mail.error.as_ref().filter(|_| list.is_some_and(|list| list.loaded)))
      && !reader_failed
    {
      let retry = pill("mail-retry")
        .child("Retry")
        .text_color(rgb(ACCENT))
        .on_click(cx.listener(|this, _, _, cx| this.reload(cx)));
      return Some(
        h_flex()
          .min_w_0()
          .gap_2()
          .items_center()
          .child(line(error.clone(), RED))
          .child(retry),
      );
    }
    match (&self.mailbox, list) {
      (Mailbox::Search(_), Some(list)) if list.loaded && list.loading => Some(line("Searching…".into(), MUTED)),
      _ => None,
    }
  }

  fn toolbar(&self, cx: &Context<Self>) -> Div {
    h_flex()
      .h(px(TOOLBAR_HEIGHT))
      .flex_shrink_0()
      .px_4()
      .gap_2()
      .items_center()
      .child(self.compose_control(cx))
      .child(self.switcher(cx))
      .children(self.labels_control(cx))
      .child(div().flex_1().min_w_0().flex().justify_end().children(self.status(cx)))
      .child(
        h_flex()
          .w(px(SEARCH_WIDTH))
          .min_w(px(SEARCH_MIN_WIDTH))
          .h(px(34.))
          .px_3()
          .gap_2()
          .items_center()
          .rounded_full()
          .bg(rgb(FIELD))
          .child(icon(Icon::Search, 15., MUTED))
          .child(div().flex_1().child(Input::new(&self.search).appearance(false).bordered(false))),
      )
  }

  // The list.

  fn list(&self, cx: &Context<Self>) -> AnyElement {
    let mail = &self.store.read(cx).mail;
    let list = mail.lists.get(&self.mailbox);
    let loaded = list.is_some_and(|list| list.loaded);
    if !loaded {
      return match list.and_then(|list| list.failed.clone()) {
        Some(error) => failure(error, "mail-list-retry", cx.listener(|this, _, _, cx| this.reload(cx))),
        None => note("Loading…"),
      };
    }
    let Some(list) = list else {
      return note("Loading…");
    };
    if list.threads.is_empty() {
      return note(empty_text(&self.mailbox));
    }
    let count = list.threads.len() + usize::from(list.more);
    div()
      .size_full()
      .child(
        uniform_list(
          "mail-threads",
          count,
          cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
        )
        .track_scroll(&self.list_scroll)
        .size_full(),
      )
      .into_any_element()
  }

  fn rows(&self, range: Range<usize>, cx: &Context<Self>) -> Vec<AnyElement> {
    let store = self.store.read(cx);
    let Some(list) = store.mail.lists.get(&self.mailbox) else {
      return Vec::new();
    };
    let open = store.mail.open.as_ref();
    range
      .map(|ix| match list.threads.get(ix) {
        Some(thread) => {
          let selected = open == Some(&thread.id) || self.selected.as_ref() == Some(&thread.id);
          self.thread_row(ix, thread, selected, cx)
        },
        None => self.more_row(list.loading, cx),
      })
      .collect()
  }

  /// One thread: the newest sender's avatar, the people, the subject, and the start of the newest email, with the time,
  /// the star and paperclip, and the dot while it is unread. A click opens it.
  fn thread_row(&self, ix: usize, thread: &MailThread, selected: bool, cx: &Context<Self>) -> AnyElement {
    let store = self.store.read(cx);
    let unread = thread.unread;
    let newest = thread.senders.last().map_or("", person);
    let id = thread.id.clone();
    let line = |text: String, size: f32, tone| {
      div()
        .truncate()
        .text_size(px(size))
        .text_color(rgb(tone))
        .child(emoji_font::text(text))
    };
    let row = h_flex()
      .id(("mail-thread", ix))
      .size_full()
      .px_3()
      .gap_3()
      .items_center()
      .rounded(px(10.))
      .cursor_pointer()
      .when(selected, |row| row.bg(rgb(ROW_SELECTED)))
      .when(!selected, |row| row.hover(|s| s.bg(rgb(ROW_HOVER))))
      .on_click(cx.listener(move |this, _, window, cx| this.open_thread(id.clone(), window, cx)))
      .child(avatar(&store.images, cx, None, None, newest, 36.))
      .child(
        v_flex()
          .flex_1()
          .min_w_0()
          .gap_0p5()
          .child(
            line(senders_line(&thread.senders, thread.messages, self.my_email(cx)), 14., TEXT)
              .when(unread, |d| d.font_weight(FontWeight::SEMIBOLD)),
          )
          .child(line(thread.subject.clone(), 13., TEXT).when(unread, |d| d.font_weight(FontWeight::SEMIBOLD)))
          .child(line(thread.snippet.clone(), 12., MUTED)),
      )
      .child(
        v_flex()
          .flex_shrink_0()
          .gap_1p5()
          .items_end()
          .child(
            div()
              .text_size(px(12.))
              .text_color(rgb(MUTED))
              .when(unread, |d| d.font_weight(FontWeight::SEMIBOLD).text_color(rgb(TEXT)))
              .child(util::rel_time(thread.time)),
          )
          .child(
            h_flex()
              .gap_1p5()
              .items_center()
              .when(thread.attachments, |marks| marks.child(icon(Icon::Paperclip, 13., MUTED)))
              .when(thread.starred, |marks| marks.child(icon(Icon::Star, 13., AMBER)))
              .child(div().size(px(7.)).rounded_full().when(unread, |dot| dot.bg(rgb(ACCENT))))
              .when(selected, |marks| marks.child(self.thread_actions(&thread.id, cx))),
          ),
      );
    div().h(px(ROW_HEIGHT)).w_full().px_2().py_0p5().child(row).into_any_element()
  }

  /// The row after the last thread while Gmail has older ones: a click asks for the next page.
  fn more_row(&self, loading: bool, cx: &Context<Self>) -> AnyElement {
    let label = if loading {
      "Loading…"
    } else {
      "Load older mail"
    };
    let line = h_flex()
      .id("mail-more")
      .size_full()
      .justify_center()
      .items_center()
      .rounded(px(10.))
      .text_size(px(13.))
      .text_color(rgb(MUTED))
      .when(!loading, |line| {
        line
          .cursor_pointer()
          .hover(|s| s.bg(rgb(ROW_HOVER)))
          .on_click(cx.listener(|this, _, _, cx| this.load_more(cx)))
      })
      .child(label);
    div().h(px(ROW_HEIGHT)).w_full().px_2().py_0p5().child(line).into_any_element()
  }

  // The reading pane.

  /// The thread open: its subject, then each email as a card, oldest first. `narrow`: the pane stands in for the
  /// list, so a button first in its head goes back to it; beside the list, Escape closes the thread.
  fn reader(&self, narrow: bool, cx: &Context<Self>) -> AnyElement {
    let store = self.store.read(cx);
    let mail = &store.mail;
    let thread = mail.open.as_deref().and_then(|id| find_thread(mail, &self.mailbox, id));
    let subject = thread.map_or_else(String::new, |thread| thread.subject.clone());
    let back = narrow.then(|| {
      div()
        .id("mail-back")
        .size(px(30.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .hover(|s| s.bg(rgb(ROW_HOVER)))
        .on_click(cx.listener(|this, _, window, cx| this.close_thread(window, cx)))
        .child(icon(Icon::ArrowLeft, 16., MUTED))
    });
    let title = div()
      .flex_1()
      .min_w_0()
      .text_size(px(18.))
      .font_weight(FontWeight::SEMIBOLD)
      .text_color(rgb(TEXT))
      .child(emoji_font::text(subject));
    let star = thread.filter(|thread| thread.starred).map(|_| icon(Icon::Star, 16., AMBER));
    let header = h_flex().flex_shrink_0().px_4().py_3().gap_2().items_center();
    // Move to Trash ends the head's group only: the selected row's actions leave it out.
    let actions = mail
      .open
      .as_deref()
      .map(|open| self.thread_actions(open, cx).child(Self::trash_button(open, cx)));
    let header = header.children(back).child(title).children(star).children(actions);
    let body = match &mail.messages {
      Some(messages) => self.cards(messages.as_slice(), cx),
      None => match &mail.error {
        Some(error) if !mail.opening => failure(
          error.clone(),
          "mail-thread-retry",
          cx.listener(|this, _, _, cx| this.reopen(cx)),
        ),
        _ => note("Loading…"),
      },
    };
    v_flex()
      .size_full()
      .child(header)
      .child(div().flex_1().min_h_0().child(body))
      .into_any_element()
  }

  /// The emails of the thread, oldest first: the newest whole, the older ones on one line, until a click swaps one.
  fn cards(&self, messages: &[MailMessage], cx: &Context<Self>) -> AnyElement {
    let store = self.store.read(cx);
    let last = messages.len().saturating_sub(1);
    let cards = messages.iter().enumerate().map(|(ix, message)| {
      if expanded(ix == last, self.toggled.contains(&message.id)) {
        self.whole_card(ix, message, &store.images, cx)
      } else {
        self.line_card(ix, message, &store.images, cx)
      }
    });
    div()
      .id("mail-reader")
      .size_full()
      .overflow_y_scroll()
      .track_scroll(&self.reader_scroll)
      .child(v_flex().px_4().pb_4().gap_2().children(cards).child(self.reply_area(cx)))
      .into_any_element()
  }

  /// An older email on one line: who, the start of its text, and when. A click shows it whole.
  fn line_card(&self, ix: usize, message: &MailMessage, images: &Images, cx: &Context<Self>) -> AnyElement {
    let id = message.id.clone();
    h_flex()
      .id(("mail-line", ix))
      .px_4()
      .py_2p5()
      .gap_3()
      .items_center()
      .rounded(px(12.))
      .border_1()
      .border_color(rgb(BORDER))
      .cursor_pointer()
      .hover(|s| s.bg(rgb(ROW_HOVER)))
      .on_click(cx.listener(move |this, _, _, cx| this.toggle(&id, cx)))
      .child(avatar(images, cx, None, None, &self.sender_name(&message.from, cx), 28.))
      .child(
        div()
          .id(("mail-line-from", ix))
          .min_w_0()
          .max_w(px(200.))
          .truncate()
          .text_size(px(13.))
          .text_color(rgb(TEXT))
          .when(message.unread, |d| d.font_weight(FontWeight::SEMIBOLD))
          .tooltip(Self::sender_tip(self.sender_name(&message.from, cx), &message.from.email))
          .child(emoji_font::text(self.sender_name(&message.from, cx))),
      )
      .child(
        div()
          .flex_1()
          .min_w_0()
          .truncate()
          .text_size(px(13.))
          .text_color(rgb(MUTED))
          .child(emoji_font::text(one_line(&message.text))),
      )
      .child(
        div()
          .flex_shrink_0()
          .text_size(px(12.))
          .text_color(rgb(MUTED))
          .child(util::rel_time(message.time)),
      )
      .into_any_element()
  }

  /// An email whole: who wrote it and to whom, when, its text, and its files. A click on its head puts it on one line.
  fn whole_card(&self, ix: usize, message: &MailMessage, images: &Images, cx: &Context<Self>) -> AnyElement {
    let id = message.id.clone();
    let from = &message.from;
    let recipients = recipients_line(&message.to, &message.cc);
    // An email's own colors expect white, so one may be unreadable on a dark theme: this shows every email on the
    // other background, and back, and is saved (`Settings::mail_other_background`).
    let flipped = cx.global::<Settings>().mail_other_background;
    let contrast = (!message.html.trim().is_empty()).then(|| {
      let tip = if flipped {
        "Show every email on the theme's background"
      } else if crate::theme::is_light() {
        "Show every email on a dark background"
      } else {
        "Show every email on a light background"
      };
      div()
        .id(("mail-contrast", ix))
        .size(px(24.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .hover(|s| s.bg(rgb(ROW_HOVER)))
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx))
        .on_click(|_, _, cx| {
          cx.stop_propagation();
          Settings::update(cx, |settings| settings.mail_other_background.toggle());
        })
        .child(icon(Icon::Contrast, 14., if flipped { ACCENT } else { MUTED }))
    });
    let head = h_flex()
      .id(("mail-head", ix))
      .gap_3()
      .items_start()
      .cursor_pointer()
      .on_click(cx.listener(move |this, _, _, cx| this.toggle(&id, cx)))
      .child(avatar(images, cx, None, None, &self.sender_name(from, cx), 36.))
      .child(
        v_flex()
          .flex_1()
          .min_w_0()
          .gap_0p5()
          .child(
            // The name and the address give way, with an ellipsis, before the time does: it keeps its width.
            h_flex()
              .gap_2()
              .items_baseline()
              .min_w_0()
              .overflow_hidden()
              .child(
                div()
                  .id(("mail-from", ix))
                  .min_w_0()
                  .truncate()
                  .text_size(px(14.))
                  .font_weight(FontWeight::SEMIBOLD)
                  .text_color(rgb(TEXT))
                  .tooltip(Self::sender_tip(self.sender_name(from, cx), &from.email))
                  .child(emoji_font::text(self.sender_name(from, cx))),
              )
              .when(!from.name.trim().is_empty(), |line| {
                line.child(
                  div()
                    .min_w_0()
                    .flex_shrink_0()
                    .max_w(px(240.))
                    .truncate()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child(format!("<{}>", from.email)),
                )
              }),
          )
          .when(!recipients.is_empty(), |who| {
            who.child(
              div()
                .truncate()
                .text_size(px(12.))
                .text_color(rgb(MUTED))
                .child(emoji_font::text(recipients)),
            )
          }),
      )
      .children(contrast)
      .child(
        div()
          .flex_shrink_0()
          .text_size(px(12.))
          .text_color(rgb(MUTED))
          .child(full_time(message.time)),
      );
    let files = message.attachments.iter().enumerate().map(|(i, file)| attachment_chip(i, file));
    v_flex()
      .id(("mail-card", ix))
      .px_4()
      .py_3()
      .gap_3()
      .rounded(px(12.))
      .border_1()
      .border_color(rgb(BORDER))
      .child(head)
      .children(self.email_body(message, cx))
      .when(!message.attachments.is_empty(), |card| {
        card.child(h_flex().flex_wrap().gap_2().children(files))
      })
      .into_any_element()
  }

  /// The tooltip of a sender's name, which may be cut short: the whole name and the address.
  fn sender_tip(name: String, email: &str) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let tip: SharedString = if name == email || email.is_empty() {
      name.into()
    } else {
      format!("{name} <{email}>").into()
    };
    move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
  }

  /// What an email shown whole says: its HTML in a webview sized to the email, while it has HTML and the webview
  /// could be built; else its text.
  fn email_body(&self, message: &MailMessage, cx: &App) -> Option<AnyElement> {
    if !message.html.trim().is_empty()
      && let Some(body) = self.bodies.get(&message.id)
    {
      let page = div().w_full().h(px(body.height)).flex_shrink_0().children(body.web.clone());
      // On the other background the page gets a frame of its own color, so its text does not touch the card.
      let page = match other_background(cx) {
        Some(background) => div()
          .w_full()
          .p_3()
          .rounded(px(8.))
          .bg(gpui_kit::rgb(background))
          .child(page)
          .into_any_element(),
        None => page.into_any_element(),
      };
      return Some(page);
    }
    let text = message.text.trim_end();
    (!text.is_empty()).then(|| {
      div()
        .text_size(px(14.))
        .text_color(rgb(TEXT))
        .child(emoji_font::text(text.to_string()))
        .into_any_element()
    })
  }
}

/// The thread `id`, as the mailbox on screen lists it, or else as another one does.
fn find_thread<'a>(mail: &'a MailData, mailbox: &Mailbox, id: &str) -> Option<&'a MailThread> {
  let find = |threads: &'a [MailThread]| threads.iter().find(|thread| thread.id == id);
  mail
    .lists
    .get(mailbox)
    .and_then(|list| find(list.threads.as_slice()))
    .or_else(|| mail.lists.values().find_map(|list| find(list.threads.as_slice())))
}

/// When an email arrived, in full: `Today, 1:50 PM`, `Tuesday, Nov 3, 9:05 AM`.
fn full_time(time: gchat_core::Micros) -> String {
  let clock = util::clock_text(util::local(time), crate::clock::is_24_hour());
  format!("{}, {clock}", util::date_label(time))
}

/// A file of an email: its name and size. A click opens it in the media viewer, which reads it through the core. Its
/// id is `ix`, the file's place in the email, under the card's own.
fn attachment_chip(ix: usize, file: &MailAttachment) -> Stateful<Div> {
  let opened = attachment(file);
  h_flex()
    .id(("mail-file", ix))
    .max_w(px(420.))
    .gap_2()
    .px_3()
    .py_2()
    .items_center()
    .rounded(px(10.))
    .border_1()
    .border_color(rgb(BORDER))
    .cursor_pointer()
    .hover(|s| s.bg(rgb(ROW_HOVER)))
    .on_click(move |_, window, cx| viewer::open_attachment(&opened, window, cx))
    .child(icon(Icon::File, 16., MUTED))
    .child(
      div()
        .min_w_0()
        .truncate()
        .text_size(px(13.))
        .text_color(rgb(TEXT))
        .child(emoji_font::text(file.name.clone())),
    )
    .child(
      div()
        .flex_shrink_0()
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .child(size_text(file.size)),
    )
}

/// A muted line in the middle of an empty pane.
fn note(text: impl Into<SharedString>) -> AnyElement {
  div()
    .size_full()
    .flex()
    .items_center()
    .justify_center()
    .px_6()
    .text_center()
    .text_color(rgb(MUTED))
    .child(text.into())
    .into_any_element()
}

/// Why Gmail could not be read, with a Retry button that asks again.
fn failure(
  error: String,
  id: &'static str,
  retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
  v_flex()
    .size_full()
    .items_center()
    .justify_center()
    .gap_3()
    .px_6()
    .child(div().text_center().text_color(rgb(MUTED)).child(error))
    .child(pill(id).child("Retry").text_color(rgb(ACCENT)).on_click(retry))
    .into_any_element()
}

impl Render for MailView {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let wanted = self.wanted_bodies(cx);
    self.sync_bodies(&wanted, window, cx);
    let split = splits(crate::root::panel_width(window, cx));
    let reading = self.store.read(cx).mail.open.is_some();
    let composing = self.compose.is_some();
    let pane = |narrow: bool, cx: &mut Context<Self>| {
      if composing {
        self.compose_pane(narrow, window, cx)
      } else if reading {
        self.reader(narrow, cx)
      } else if narrow {
        self.list_column(cx)
      } else {
        note("Pick a thread to read it.")
      }
    };
    let body = if split {
      h_flex()
        .size_full()
        .child(
          div()
            .w(px(LIST_WIDTH))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(self.list_column(cx)),
        )
        .child(div().flex_1().min_w_0().h_full().child(pane(false, cx)))
        .into_any_element()
    } else {
      pane(true, cx)
    };
    v_flex()
      .id("mail")
      .size_full()
      .relative()
      .bg(rgb(PANEL))
      .track_focus(&self.focus)
      .key_context("Mail")
      .on_key_down(cx.listener(Self::on_mail_key))
      .on_action(cx.listener(|this, _: &OpenMailLabels, window, cx| this.toggle_labels(window, cx)))
      .on_action(cx.listener(|this, _: &SelectNextThread, _, cx| this.select_step(true, cx)))
      .on_action(cx.listener(|this, _: &SelectPreviousThread, _, cx| this.select_step(false, cx)))
      .on_action(cx.listener(|this, _: &OpenSelectedThread, window, cx| this.open_selected(window, cx)))
      .on_action(cx.listener(|this, _: &CloseMailThread, window, cx| {
        if !this.dismiss(window, cx) {
          cx.propagate();
        }
      }))
      .child(self.toolbar(cx))
      .child(div().flex_1().min_h_0().border_t_1().border_color(rgb(BORDER)).child(body))
      .children(self.labels_popover(cx))
  }
}
