//! What the Mail view changes in Gmail: a thread marked unread, archived (with an Undo bar for a moment), and a reply
//! written in a box under the thread's emails. The core applies each change to the lists at once and answers
//! `MailWritten`; the view keeps only the reply drafts, per thread, while the app runs. The one-letter keys (`u`, `e`,
//! `r`, `a`) work while the view itself has the keyboard, never while a field has it.

use std::time::Duration;

use gchat_core::{MailAddress, MailReply, Mailbox};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::calendar::pill;
use crate::keys;
use crate::mail::MailView;
use crate::mail_logic::{adds_people, after_removal, reply_line};
use crate::meet::solid_button;
use crate::theme::{ACCENT, BORDER, MUTED, RED, ROW_HOVER, TEXT, rgb};
use crate::ui::{Icon, icon};

/// How long the Undo bar of an archive stays.
const UNDO_FOR: Duration = Duration::from_secs(8);

/// The reply being written under the thread open.
pub struct ReplyBox {
  /// The thread it answers.
  thread: String,
  /// Reply all: everyone the newest email reached, not only its sender.
  all: bool,
  editor: Entity<TextareaState>,
  /// The user left Gmail's signature out of this answer.
  sig_off: bool,
  _sub: Subscription,
}

/// A reply put aside when its thread closed, for when it opens again.
pub struct Draft {
  all: bool,
  text: String,
}

/// The thread just archived, while its Undo bar shows.
pub struct Archived {
  thread: String,
  _expire: Task<()>,
}

impl MailView {
  /// The thread an action goes to: the one open, or else the one selected.
  fn target_thread(&self, cx: &App) -> Option<String> {
    self.store.read(cx).mail.open.clone().or_else(|| self.selected.clone())
  }

  /// Marks a thread unread in Gmail and goes back to the list: open, the view would read it again.
  pub(crate) fn mark_unread(&mut self, thread: String, window: &mut Window, cx: &mut Context<Self>) {
    self.archived = None;
    if self.store.read(cx).mail.open.as_ref() == Some(&thread) {
      self.close_thread(window, cx);
    }
    self.store.update(cx, |store, cx| store.mark_mail(thread, false, cx));
  }

  /// Archives a thread of the inbox, closes it when it was open, and selects the one after it (before it at the end)
  /// without opening it: opening would mark that one read. Enter opens it. An Undo bar puts the archived one back for
  /// a moment.
  pub(crate) fn archive(&mut self, thread: String, window: &mut Window, cx: &mut Context<Self>) {
    if self.mailbox != Mailbox::Inbox {
      return;
    }
    let threads = self.threads(cx);
    let ids: Vec<&str> = threads.iter().map(|thread| thread.id.as_str()).collect();
    let next = after_removal(&ids, &thread);
    if self.store.read(cx).mail.open.as_ref() == Some(&thread) {
      self.close_thread(window, cx);
    }
    self.store.update(cx, |store, cx| store.archive_mail(thread.clone(), true, cx));
    self.selected.clone_from(&next);
    let expire = cx.spawn(async move |this, cx| {
      cx.background_executor().timer(UNDO_FOR).await;
      this
        .update(cx, |this, cx| {
          this.archived = None;
          cx.notify();
        })
        .ok();
    });
    self.archived = Some(Archived { thread, _expire: expire });
    cx.notify();
  }

  /// Undo: the thread just archived goes back into the inbox.
  fn undo_archive(&mut self, cx: &mut Context<Self>) {
    if let Some(archived) = self.archived.take() {
      self
        .store
        .update(cx, |store, cx| store.archive_mail(archived.thread, false, cx));
    }
  }

  /// Who a reply to the newest email of the thread open reaches, `(to, cc)`, and that email's id.
  fn reply_target(&self, all: bool, cx: &App) -> Option<(Vec<MailAddress>, Vec<MailAddress>, String)> {
    let store = self.store.read(cx);
    let newest = store.mail.messages.as_ref()?.last()?;
    let me = store.me.as_ref().map_or("", |me| me.email.as_str());
    let (to, cc) = gchat_core::reply_recipients(newest, me, all);
    Some((to, cc, newest.id.clone()))
  }

  /// Whether Reply all would reach someone Reply does not.
  fn reply_all_adds(&self, cx: &App) -> bool {
    match (self.reply_target(false, cx), self.reply_target(true, cx)) {
      (Some((to, cc, _)), Some((all_to, all_cc, _))) => adds_people((&to, &cc), (&all_to, &all_cc)),
      _ => false,
    }
  }

  /// Opens the reply box under the thread open (Reply all with `all`), with its draft when one was put aside, gives it
  /// the keyboard, and scrolls it into view.
  pub(crate) fn open_reply(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
    let Some(thread) = self.store.read(cx).mail.open.clone() else {
      return;
    };
    if self
      .store
      .read(cx)
      .mail
      .messages
      .as_ref()
      .is_none_or(|messages| messages.is_empty())
    {
      return;
    }
    let all = all && self.reply_all_adds(cx);
    if let Some(reply) = self.reply.as_mut().filter(|reply| reply.thread == thread) {
      reply.all = all;
    } else {
      let text = self.drafts.remove(&thread).map_or_default(|draft| draft.text);
      self.reply = Some(self.reply_box(thread, all, &text, window, cx));
    }
    if let Some(reply) = &self.reply {
      reply.editor.update(cx, |state, cx| state.focus(window, cx));
    }
    self.reader_scroll.scroll_to_bottom();
    cx.notify();
  }

  fn reply_box(&self, thread: String, all: bool, text: &str, window: &mut Window, cx: &mut Context<Self>) -> ReplyBox {
    let editor = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 12).placeholder("Write a reply"));
    editor.update(cx, |state, cx| state.set_value(text.to_string(), window, cx));
    let sub = cx.subscribe_in(&editor, window, |this, _, event: &InputEvent, window, cx| match event {
      // The Send key (`secondary-enter`): the kit's text field answers it as a secondary Enter.
      InputEvent::PressEnter { secondary: true, .. } => this.send_reply(window, cx),
      InputEvent::Change => cx.notify(),
      _ => {},
    });
    ReplyBox {
      thread,
      all,
      editor,
      sig_off: false,
      _sub: sub,
    }
  }

  /// Leaves Gmail's signature out of this answer, or puts it back.
  fn toggle_reply_signature(&mut self, cx: &mut Context<Self>) {
    if let Some(reply) = self.reply.as_mut() {
      reply.sig_off = !reply.sig_off;
      cx.notify();
    }
  }

  /// The text of the reply box, without the blank around it.
  fn reply_text(&self, cx: &App) -> Option<String> {
    let text = self.reply.as_ref()?.editor.read(cx).value().trim().to_string();
    (!text.is_empty()).then_some(text)
  }

  /// Sends the reply, unless it is blank or a send is on its way. Its box stays until the core says it went.
  fn send_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.reply_text(cx).is_none() || self.store.read(cx).mail.sending {
      return;
    }
    let settings = cx.global::<crate::settings::Settings>();
    if settings.rewrite_always && settings.rewrite != crate::settings::RewriteTool::Off {
      self.rewrite_body(true, window, cx);
      return;
    }
    self.dispatch_reply(cx);
  }

  fn dispatch_reply(&mut self, cx: &mut Context<Self>) {
    let (Some(reply), Some(text)) = (self.reply.as_ref(), self.reply_text(cx)) else {
      return;
    };
    if self.store.read(cx).mail.sending {
      return;
    }
    let (thread, all) = (reply.thread.clone(), reply.all);
    let Some((to, cc, message)) = self.reply_target(all, cx) else {
      return;
    };
    self.archived = None;
    // Rewrite only ever sees the body: the signature goes on in the core, as Gmail adds it.
    let signature =
      !self.reply.as_ref().is_some_and(|reply| reply.sig_off) && self.store.read(cx).mail.signature_reply.is_some();
    let reply = MailReply { thread, message, to, cc, text, signature };
    self.store.update(cx, |store, cx| store.reply_mail(reply, cx));
  }

  /// Discard: the box closes and its draft goes.
  fn discard_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if let Some(reply) = self.reply.take() {
      self.drafts.remove(&reply.thread);
    }
    window.focus(&self.focus, cx);
    cx.notify();
  }

  /// The thread open changed: the box of the one left is put aside as a draft, unless it is blank.
  pub(crate) fn stash_reply(&mut self, cx: &App) {
    let Some(reply) = self.reply.take() else {
      return;
    };
    let text = reply.editor.read(cx).value().to_string();
    if text.trim().is_empty() {
      self.drafts.remove(&reply.thread);
    } else {
      self.drafts.insert(reply.thread, Draft { all: reply.all, text });
    }
  }

  /// The thread open changed: its draft, when it has one, comes back in its box.
  pub(crate) fn restore_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(thread) = self.store.read(cx).mail.open.clone() else {
      return;
    };
    if let Some(draft) = self.drafts.remove(&thread) {
      self.reply = Some(self.reply_box(thread, draft.all, &draft.text, window, cx));
    }
  }

  /// A send went: the reply box or the new-email form that it belonged to closes.
  pub(crate) fn take_sent_reply(&mut self, cx: &mut Context<Self>) {
    match self.store.update(cx, |store, _| store.take_sent()) {
      Some(crate::store::MailSent::Reply) => {
        if let Some(reply) = self.reply.take() {
          self.drafts.remove(&reply.thread);
        }
      },
      Some(crate::store::MailSent::Compose) => {
        self.compose = None;
        self.compose_draft = None;
      },
      None => {},
    }
  }

  /// Escape from the reply box's text: the view takes the keyboard back and the box stays. Answers whether it had it.
  pub(crate) fn leave_reply(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    let typing = self
      .reply
      .as_ref()
      .is_some_and(|reply| reply.editor.read(cx).focus_handle(cx).is_focused(window));
    if typing {
      window.focus(&self.focus, cx);
    }
    typing
  }

  /// The one-letter keys, while the view itself has the keyboard: `c` writes a new email, `u` marks unread, `e`
  /// archives, `r` replies, `a` replies to all.
  pub(crate) fn on_mail_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
    let stroke = &event.keystroke;
    if stroke.modifiers.platform || stroke.modifiers.control || stroke.modifiers.alt || stroke.modifiers.shift {
      return;
    }
    if !self.focus.is_focused(window) {
      return;
    }
    match stroke.key.as_str() {
      "c" => self.open_compose(window, cx),
      "u" => match self.target_thread(cx) {
        Some(thread) => self.mark_unread(thread, window, cx),
        None => return,
      },
      "e" if self.mailbox == Mailbox::Inbox => match self.target_thread(cx) {
        Some(thread) => self.archive(thread, window, cx),
        None => return,
      },
      "r" | "a" if self.store.read(cx).mail.open.is_some() => self.open_reply(stroke.key == "a", window, cx),
      _ => return,
    }
    cx.stop_propagation();
  }

  /// The Rewrite key: the new-email body if that form is open, else the reply box.
  pub(crate) fn rewrite_hotkey(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.rewrite_body(false, window, cx);
  }

  /// Rewrites the email body in place. With `then_send`, a successful rewrite goes out right away.
  pub(crate) fn rewrite_body(&mut self, then_send: bool, window: &mut Window, cx: &mut Context<Self>) {
    if self.rewrite_task.is_some() {
      return;
    }
    let tool = cx.global::<crate::settings::Settings>().rewrite;
    if tool == crate::settings::RewriteTool::Off {
      return;
    }
    let composing = self.compose.is_some();
    let composer = if let Some(form) = &self.compose {
      form.body.clone()
    } else if let Some(reply) = &self.reply {
      reply.editor.clone()
    } else {
      return;
    };
    if composer.read(cx).value().trim().is_empty() {
      if then_send {
        if composing {
          self.dispatch_compose(cx);
        } else {
          self.dispatch_reply(cx);
        }
      }
      return;
    }
    self.rewrite_error = None;
    self.rewriting = true;
    self.rewrite_task = Some(crate::rewrite::spawn(
      composer,
      crate::rewrite::Kind::Mail,
      window,
      cx,
      move |this, result, _, cx| {
        this.rewrite_task = None;
        this.rewriting = false;
        if then_send && result.is_ok() {
          if composing {
            this.dispatch_compose(cx);
          } else {
            this.dispatch_reply(cx);
          }
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

  /// "Rewriting with …" or the last failure, under the body.
  pub(crate) fn rewrite_status(&self, cx: &App) -> Option<Div> {
    if self.rewriting {
      let name = cx.global::<crate::settings::Settings>().rewrite.label();
      return Some(
        div()
          .text_size(px(12.))
          .text_color(rgb(MUTED))
          .child(format!("Rewriting with {name}...")),
      );
    }
    self
      .rewrite_error
      .clone()
      .map(|error| div().text_size(px(12.)).text_color(rgb(RED)).child(error))
  }

  // Drawing.

  /// Mark as unread, and in the inbox Archive: on the reading pane's head and on the selected row.
  pub(crate) fn thread_actions(&self, thread: &str, cx: &Context<Self>) -> Div {
    let button = |id: &'static str, glyph: Icon, tip: String| {
      div()
        .id(id)
        .size(px(28.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .hover(|s| s.bg(rgb(ROW_HOVER)))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .child(icon(glyph, 15., MUTED))
    };
    let unread = thread.to_string();
    let archived = thread.to_string();
    h_flex()
      .flex_shrink_0()
      .gap_0p5()
      .child(
        button("mail-unread", Icon::Mail, format!("Mark as unread ({})", keys::text("u"))).on_click(cx.listener(
          move |this, _, window, cx| {
            cx.stop_propagation();
            this.mark_unread(unread.clone(), window, cx);
          },
        )),
      )
      .when(self.mailbox == Mailbox::Inbox, |actions| {
        actions.child(
          button("mail-archive", Icon::Archive, format!("Archive ({})", keys::text("e"))).on_click(cx.listener(
            move |this, _, window, cx| {
              cx.stop_propagation();
              this.archive(archived.clone(), window, cx);
            },
          )),
        )
      })
  }

  /// Under the thread's emails: the reply box while it is open, else Reply and, when it reaches someone more, Reply
  /// all.
  pub(crate) fn reply_area(&self, cx: &Context<Self>) -> AnyElement {
    let open = self.store.read(cx).mail.open.clone();
    let Some(reply) = self.reply.as_ref().filter(|reply| Some(&reply.thread) == open.as_ref()) else {
      let all = self.reply_all_adds(cx);
      let start = |id: &'static str, glyph: Icon, label: &'static str, key: &str, all: bool| {
        pill(id)
          .gap_1p5()
          .child(icon(glyph, 14., TEXT))
          .child(label)
          .tooltip({
            let tip = format!("{label} ({})", keys::text(key));
            move |window, cx| Tooltip::new(tip.clone()).build(window, cx)
          })
          .on_click(cx.listener(move |this, _, window, cx| this.open_reply(all, window, cx)))
      };
      return h_flex()
        .gap_2()
        .child(start("mail-reply", Icon::Reply, "Reply", "r", false))
        .when(all, |row| {
          row.child(start("mail-reply-all", Icon::ReplyAll, "Reply all", "a", true))
        })
        .into_any_element();
    };
    let store = self.store.read(cx);
    let sending = store.mail.sending;
    let line = self
      .reply_target(reply.all, cx)
      .map_or_else(String::new, |(to, cc, _)| reply_line(&to, &cc));
    let ready = !sending && !self.rewriting && self.reply_text(cx).is_some();
    let send = solid_button("mail-send", Icon::Reply, if sending { "Sending…" } else { "Send" }, ready)
      .tooltip({
        let tip = format!("Send ({})", keys::text("secondary-enter"));
        move |window, cx| Tooltip::new(tip.clone()).build(window, cx)
      })
      .when(ready, |button| {
        button.on_click(cx.listener(|this, _, window, cx| this.send_reply(window, cx)))
      });
    let settings = cx.global::<crate::settings::Settings>();
    let rewrite = settings.rewrite != crate::settings::RewriteTool::Off;
    let always = settings.rewrite_always;
    v_flex()
      .id("mail-reply-box")
      .p_3()
      .gap_2()
      .rounded(px(12.))
      .border_1()
      .border_color(rgb(ACCENT))
      .child(
        h_flex()
          .gap_2()
          .items_center()
          .text_size(px(12.))
          .text_color(rgb(MUTED))
          .child(icon(
            if reply.all {
              Icon::ReplyAll
            } else {
              Icon::Reply
            },
            13.,
            MUTED,
          ))
          .child(div().min_w_0().truncate().child(line)),
      )
      .child(
        h_flex()
          .w_full()
          .items_end()
          .gap_2()
          .child(
            div().flex_1().min_w_0().px_1().text_size(px(14.)).child(
              Textarea::new(&reply.editor)
                .appearance(false)
                .bordered(false)
                .disabled(sending || self.rewriting),
            ),
          )
          .when(rewrite, |row| {
            row.child(crate::rewrite::button(
              "mail-reply-rewrite",
              "mail-reply-rewrite-pulse",
              self.rewriting,
              always,
              cx,
              |this, _, window, cx| this.rewrite_body(false, window, cx),
            ))
          }),
      )
      .children(store.mail.signature_reply.as_ref().map(|sig| {
        signature_note(
          &sig.text,
          reply.sig_off,
          "mail-reply-signature",
          cx.listener(|this, _, _, cx| this.toggle_reply_signature(cx)),
        )
      }))
      .children(
        store
          .mail
          .send_error
          .clone()
          .map(|error| div().text_size(px(12.)).text_color(rgb(RED)).child(error)),
      )
      .children(self.rewrite_status(cx))
      .child(
        h_flex()
          .gap_2()
          .justify_end()
          .child(
            pill("mail-discard")
              .child("Discard")
              .text_color(rgb(MUTED))
              .on_click(cx.listener(|this, _, window, cx| this.discard_reply(window, cx))),
          )
          .child(send),
      )
      .into_any_element()
  }

  /// The slim bar under the list while a thread just archived can come back.
  pub(crate) fn undo_bar(&self, cx: &Context<Self>) -> Option<Div> {
    self.archived.as_ref()?;
    Some(
      h_flex()
        .flex_shrink_0()
        .h(px(40.))
        .px_4()
        .gap_3()
        .items_center()
        .border_t_1()
        .border_color(rgb(BORDER))
        .text_size(px(13.))
        .text_color(rgb(TEXT))
        .child(div().flex_1().child("Archived"))
        .child(
          pill("mail-undo")
            .child("Undo")
            .text_color(rgb(ACCENT))
            .on_click(cx.listener(|this, _, _, cx| this.undo_archive(cx))),
        ),
    )
  }
}

/// Gmail's signature under an email's body, as it will be sent: its plain text dimmed, four lines at most, with a
/// control to leave it out of this one email or put it back.
pub(crate) fn signature_note(
  text: &str,
  off: bool,
  id: &'static str,
  toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
  let control = pill(id)
    .text_size(px(12.))
    .text_color(rgb(MUTED))
    .child(if off {
      "Add signature"
    } else {
      "Leave out signature"
    })
    .on_click(toggle);
  let body = if off {
    div()
      .text_size(px(12.))
      .text_color(rgb(MUTED))
      .child("No signature on this email")
  } else {
    div()
      .text_size(px(12.))
      .text_color(rgb(MUTED))
      .opacity(0.8)
      .whitespace_normal()
      .child(crate::mail_logic::signature_preview(text, 4))
  };
  h_flex()
    .w_full()
    .px_1()
    .gap_2()
    .items_start()
    .child(div().flex_1().min_w_0().child(body))
    .child(control)
}
