//! A new email: To and Cc as chips with suggestions from the people directory, a subject, and a body with Rewrite. One
//! unsent draft is kept while the app runs when the form is left; Discard drops it. Send queues `ComposeMail` with the
//! other writes.

use gchat_core::MailCompose;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::calendar::pill;
use crate::keys;
use crate::mail::MailView;
use crate::mail_logic::{
  AddressField, AddressHint, TypedAddress, address_taken, commit_token, compose_blocker, rank_addresses, take_addresses,
};
use crate::meet::solid_button;
use crate::new_chat::match_people;
use crate::settings::{RewriteTool, Settings};
use crate::theme::{BORDER, FIELD, MUTED, POPOVER, RED, ROW_HOVER, ROW_SELECTED, TEXT, rgb};
use crate::ui::{Icon, icon};

const CHIP: f32 = 24.0;
const SUGGEST: f32 = 40.0;

/// The most suggestions shown at once.
const MAX_SUGGEST: usize = 6;
/// The new-email form on screen.
pub struct ComposeForm {
  to: Vec<TypedAddress>,
  cc: Vec<TypedAddress>,
  to_input: Entity<InputState>,
  cc_input: Entity<InputState>,
  subject: Entity<InputState>,
  pub(crate) body: Entity<TextareaState>,
  to_row: usize,
  cc_row: usize,
  /// Escape hid the suggestions until the next keystroke in the field.
  hide_suggest: bool,
  /// The user left Gmail's signature out of this email.
  sig_off: bool,
  _subs: Vec<Subscription>,
}

/// An unsent new email kept while the app runs, offered again by New email.
pub struct ComposeDraft {
  to: Vec<TypedAddress>,
  cc: Vec<TypedAddress>,
  subject: String,
  text: String,
}

impl ComposeDraft {
  fn is_empty(&self) -> bool {
    self.to.is_empty() && self.cc.is_empty() && self.subject.trim().is_empty() && self.text.trim().is_empty()
  }
}

impl MailView {
  /// Opens the new-email form in the reading pane's place, restoring an unsent draft when there is one.
  pub(crate) fn open_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.compose.is_some() {
      if let Some(form) = &self.compose {
        form.to_input.update(cx, |state, cx| state.focus(window, cx));
      }
      return;
    }
    self.close_labels(window, cx);
    if self.store.read(cx).mail.open.is_some() {
      self.close_thread(window, cx);
    }
    let draft = self.compose_draft.take();
    self.compose = Some(self.compose_form(draft, window, cx));
    if let Some(form) = &self.compose {
      form.to_input.update(cx, |state, cx| state.focus(window, cx));
    }
    cx.notify();
  }

  fn compose_form(&self, draft: Option<ComposeDraft>, window: &mut Window, cx: &mut Context<Self>) -> ComposeForm {
    let draft = draft.unwrap_or(ComposeDraft {
      to: Vec::new(),
      cc: Vec::new(),
      subject: String::new(),
      text: String::new(),
    });
    let to_input = cx.new(|cx| InputState::new(window, cx).placeholder("Add people"));
    let cc_input = cx.new(|cx| InputState::new(window, cx).placeholder("Add people"));
    let subject = cx.new(|cx| InputState::new(window, cx).placeholder("Subject"));
    let body = cx.new(|cx| TextareaState::new(window, cx).auto_grow(5, 12).placeholder("Write an email"));
    subject.update(cx, |state, cx| state.set_value(draft.subject, window, cx));
    body.update(cx, |state, cx| state.set_value(draft.text, window, cx));
    let subs = vec![
      cx.subscribe_in(&to_input, window, |this, _, event: &InputEvent, window, cx| {
        this.on_addr_event(true, event, window, cx);
      }),
      cx.subscribe_in(&cc_input, window, |this, _, event: &InputEvent, window, cx| {
        this.on_addr_event(false, event, window, cx);
      }),
      cx.subscribe_in(&subject, window, |this, _, event: &InputEvent, window, cx| {
        if let InputEvent::PressEnter { .. } = event
          && let Some(form) = &this.compose
        {
          form.body.update(cx, |state, cx| state.focus(window, cx));
        }
        if matches!(event, InputEvent::Change) {
          cx.notify();
        }
      }),
      cx.subscribe_in(&body, window, |this, _, event: &InputEvent, window, cx| match event {
        InputEvent::PressEnter { secondary: true, .. } => this.send_compose(window, cx),
        InputEvent::Change => cx.notify(),
        _ => {},
      }),
    ];
    ComposeForm {
      to: draft.to,
      cc: draft.cc,
      to_input,
      cc_input,
      subject,
      body,
      to_row: 0,
      cc_row: 0,
      hide_suggest: false,
      sig_off: false,
      _subs: subs,
    }
  }

  fn on_addr_event(&mut self, to: bool, event: &InputEvent, window: &mut Window, cx: &mut Context<Self>) {
    match event {
      InputEvent::Change => {
        if let Some(form) = self.compose.as_mut() {
          form.hide_suggest = false;
        }
        self.take_typed(to, false, window, cx);
      },
      InputEvent::PressEnter { .. } => self.on_addr_enter(to, window, cx),
      InputEvent::Blur => self.commit_pending(to, window, cx),
      InputEvent::Focus => {},
    }
  }

  fn take_typed(&mut self, to: bool, finish: bool, window: &mut Window, cx: &mut Context<Self>) {
    let Some(form) = self.compose.as_mut() else {
      return;
    };
    let input = if to { &form.to_input } else { &form.cc_input };
    let value = input.read(cx).value().to_string();
    let (chips, rest) = take_addresses(&value, finish);
    let field = if to { &mut form.to } else { &mut form.cc };
    for chip in chips {
      if !address_taken(field, &chip.email) {
        field.push(chip);
      }
    }
    if rest != value {
      input.update(cx, |state, cx| state.set_value(rest, window, cx));
    }
    if to {
      form.to_row = 0;
    } else {
      form.cc_row = 0;
    }
    cx.notify();
  }

  fn on_addr_enter(&mut self, to: bool, window: &mut Window, cx: &mut Context<Self>) {
    let Some(form) = self.compose.as_ref() else {
      return;
    };
    let query = if to {
      form.to_input.read(cx).value().to_string()
    } else {
      form.cc_input.read(cx).value().to_string()
    };
    let row = if to { form.to_row } else { form.cc_row };
    let people = self.shown_suggestions(to, &query, cx);
    if let Some(person) = people.get(row) {
      self.add_person(to, person.name.clone(), person.email.clone(), window, cx);
      return;
    }
    self.commit_pending(to, window, cx);
  }

  /// The field lost the keyboard (or Send was asked): its token becomes a chip when it is a valid address or exactly
  /// one person (`mail_logic::commit_token`); otherwise it stays in the field, marked, and blocks Send.
  fn commit_pending(&mut self, to: bool, window: &mut Window, cx: &mut Context<Self>) {
    let people = self.suggest(to, "", cx);
    let Some(form) = self.compose.as_mut() else {
      return;
    };
    let input = if to {
      form.to_input.clone()
    } else {
      form.cc_input.clone()
    };
    let typed = input.read(cx).value().to_string();
    let Some(chip) = commit_token(&typed, &people) else {
      return;
    };
    let field = if to { &mut form.to } else { &mut form.cc };
    if !address_taken(field, &chip.email) {
      field.push(chip);
    }
    input.update(cx, |state, cx| state.set_value("", window, cx));
    cx.notify();
  }

  /// Why Send is off, from the chips, the text still typed in To and Cc, the subject, and the body.
  fn blocker(&self, cx: &App) -> Option<&'static str> {
    let form = self.compose.as_ref()?;
    let to_typed = form.to_input.read(cx).value().to_string();
    let cc_typed = form.cc_input.read(cx).value().to_string();
    let subject = form.subject.read(cx).value().to_string();
    let body = form.body.read(cx).value().to_string();
    let people = self.suggest(true, "", cx);
    compose_blocker(
      &AddressField { chips: &form.to, typed: &to_typed },
      &AddressField { chips: &form.cc, typed: &cc_typed },
      &subject,
      &body,
      &people,
    )
  }

  /// Leaves Gmail's signature out of this email, or puts it back.
  fn toggle_compose_signature(&mut self, cx: &mut Context<Self>) {
    if let Some(form) = self.compose.as_mut() {
      form.sig_off = !form.sig_off;
      cx.notify();
    }
  }

  fn add_person(&mut self, to: bool, name: String, email: String, window: &mut Window, cx: &mut Context<Self>) {
    let Some(form) = self.compose.as_mut() else {
      return;
    };
    let field = if to { &mut form.to } else { &mut form.cc };
    if !address_taken(field, &email) {
      field.push(TypedAddress::person(&name, &email));
    }
    let input = if to { &form.to_input } else { &form.cc_input };
    input.update(cx, |state, cx| state.set_value("", window, cx));
    if to {
      form.to_row = 0;
    } else {
      form.cc_row = 0;
    }
    cx.notify();
  }

  fn remove_chip(&mut self, to: bool, ix: usize, cx: &mut Context<Self>) {
    let Some(form) = self.compose.as_mut() else {
      return;
    };
    let field = if to { &mut form.to } else { &mut form.cc };
    if ix < field.len() {
      field.remove(ix);
    }
    cx.notify();
  }

  fn suggest(&self, to: bool, query: &str, cx: &App) -> Vec<AddressHint> {
    let Some(form) = self.compose.as_ref() else {
      return Vec::new();
    };
    let chips = if to { &form.to } else { &form.cc };
    let taken: Vec<&str> = chips.iter().map(|chip| chip.email.as_str()).collect();
    let store = self.store.read(cx);
    let people: Vec<AddressHint> = match_people(store, "", |user| user.email.is_empty() || user.is_bot || user.deleted)
      .iter()
      .map(|user| AddressHint {
        name: user.name.clone(),
        email: user.email.clone(),
      })
      .collect();
    rank_addresses(query, &people, &taken).into_iter().cloned().collect()
  }

  /// The suggestions the field shows: only while its token has text and Escape did not hide them, at most six.
  fn shown_suggestions(&self, to: bool, query: &str, cx: &App) -> Vec<AddressHint> {
    let hidden = self.compose.as_ref().is_none_or(|form| form.hide_suggest);
    if hidden || query.trim().is_empty() {
      return Vec::new();
    }
    let mut people = self.suggest(to, query, cx);
    people.truncate(MAX_SUGGEST);
    people
  }

  /// Puts the form aside as a draft, unless it is blank.
  pub(crate) fn stash_compose(&mut self, cx: &App) {
    let Some(form) = self.compose.take() else {
      return;
    };
    let draft = ComposeDraft {
      to: form.to,
      cc: form.cc,
      subject: form.subject.read(cx).value().to_string(),
      text: form.body.read(cx).value().to_string(),
    };
    self.compose_draft = if draft.is_empty() { None } else { Some(draft) };
  }

  /// Discard: the form and its draft go.
  fn discard_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.compose = None;
    self.compose_draft = None;
    window.focus(&self.focus, cx);
    cx.notify();
  }

  /// Escape from the form: it is put aside and the list shows again. Answers whether the form was there.
  pub(crate) fn leave_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    if self.compose.is_none() {
      return false;
    }
    self.stash_compose(cx);
    window.focus(&self.focus, cx);
    cx.notify();
    true
  }

  fn send_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.commit_pending(true, window, cx);
    self.commit_pending(false, window, cx);
    if self.blocker(cx).is_some() || self.store.read(cx).mail.sending {
      return;
    }
    let Some(form) = self.compose.as_ref() else {
      return;
    };
    let text = form.body.read(cx).value().to_string();
    let settings = cx.global::<Settings>();
    if settings.rewrite_always && settings.rewrite != RewriteTool::Off && !text.trim().is_empty() {
      self.rewrite_body(true, window, cx);
      return;
    }
    self.dispatch_compose(cx);
  }

  pub(crate) fn dispatch_compose(&mut self, cx: &mut Context<Self>) {
    let Some(form) = self.compose.as_ref() else {
      return;
    };
    // Rewrite only ever sees the body: the signature goes on in the core, as Gmail adds it.
    let signature = !form.sig_off && self.store.read(cx).mail.signature_new.is_some();
    let compose = MailCompose {
      to: form.to.iter().map(TypedAddress::mail).collect(),
      cc: form.cc.iter().map(TypedAddress::mail).collect(),
      bcc: Vec::new(),
      subject: form.subject.read(cx).value().to_string(),
      text: form.body.read(cx).value().to_string(),
      signature,
    };
    self.store.update(cx, |store, cx| store.compose_mail(compose, cx));
  }

  /// The new-email form: beside the list when there is room, over it when narrow, as the reader does.
  pub(crate) fn compose_pane(&self, narrow: bool, window: &Window, cx: &mut Context<Self>) -> AnyElement {
    let Some(form) = self.compose.as_ref() else {
      return div().into_any_element();
    };
    let store = self.store.read(cx);
    let sending = store.mail.sending;
    let blocker = self.blocker(cx);
    let ready = !sending && !self.rewriting && blocker.is_none();
    let signature = store.mail.signature_new.as_ref().map(|sig| {
      crate::mail_write::signature_note(
        &sig.text,
        form.sig_off,
        "mail-compose-signature",
        cx.listener(|this, _, _, cx| this.toggle_compose_signature(cx)),
      )
    });
    let close = div()
      .id("mail-compose-close")
      .size(px(30.))
      .flex()
      .flex_shrink_0()
      .items_center()
      .justify_center()
      .rounded_full()
      .cursor_pointer()
      .hover(|s| s.bg(rgb(ROW_HOVER)))
      .on_click(cx.listener(|this, _, window, cx| {
        this.stash_compose(cx);
        window.focus(&this.focus, cx);
        cx.notify();
      }))
      .child(icon(if narrow { Icon::ArrowLeft } else { Icon::X }, 16., MUTED));
    let title = div()
      .flex_1()
      .min_w_0()
      .text_size(px(18.))
      .font_weight(FontWeight::SEMIBOLD)
      .text_color(rgb(TEXT))
      .child("New email");
    let header = h_flex().flex_shrink_0().px_4().py_3().gap_2().items_center();
    let header = if narrow {
      header.child(close).child(title)
    } else {
      header.child(title).child(close)
    };
    let send = solid_button(
      "mail-compose-send",
      Icon::Mail,
      if sending { "Sending…" } else { "Send" },
      ready,
    )
    .tooltip({
      let tip = format!("Send ({})", keys::text("secondary-enter"));
      move |window, cx| Tooltip::new(tip.clone()).build(window, cx)
    })
    .when(ready, |button| {
      button.on_click(cx.listener(|this, _, window, cx| this.send_compose(window, cx)))
    });
    v_flex()
      .id("mail-compose")
      .size_full()
      .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| this.on_compose_key(event, window, cx)))
      .child(header)
      .child(
        div().id("mail-compose-body").flex_1().min_h_0().overflow_y_scroll().child(
          v_flex()
            .px_4()
            .pb_4()
            .gap_3()
            .child(self.addr_row(true, sending, window, cx))
            .child(self.addr_row(false, sending, window, cx))
            .child(
              h_flex()
                .h(px(36.))
                .px_1()
                .items_center()
                .border_b_1()
                .border_color(rgb(BORDER))
                .child(Input::new(&form.subject).appearance(false).bordered(false).disabled(sending)),
            )
            .child(self.compose_body(sending, cx))
            .children(signature)
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
                .items_center()
                .children(
                  blocker
                    .filter(|_| !sending)
                    .map(|reason| div().flex_1().min_w_0().text_size(px(12.)).text_color(rgb(RED)).child(reason)),
                )
                .child(
                  pill("mail-compose-discard")
                    .child("Discard")
                    .text_color(rgb(MUTED))
                    .on_click(cx.listener(|this, _, window, cx| this.discard_compose(window, cx))),
                )
                .child(send),
            ),
        ),
      )
      .into_any_element()
  }

  fn addr_row(&self, to: bool, sending: bool, window: &Window, cx: &Context<Self>) -> Div {
    let Some(form) = self.compose.as_ref() else {
      return v_flex();
    };
    let chips = if to { &form.to } else { &form.cc };
    let input = if to { &form.to_input } else { &form.cc_input };
    let query = input.read(cx).value().to_string();
    let focused = input.read(cx).focus_handle(cx).is_focused(window);
    let people = if focused {
      self.shown_suggestions(to, &query, cx)
    } else {
      Vec::new()
    };
    let pick = if to { form.to_row } else { form.cc_row };
    let pick = pick.min(people.len().saturating_sub(1));
    let mut row = h_flex().gap_1().flex_wrap().min_h(px(CHIP)).w_full().items_center();
    row = row.child(
      div()
        .w(px(28.))
        .flex_shrink_0()
        .text_size(px(13.))
        .text_color(rgb(MUTED))
        .child(if to { "To" } else { "Cc" }),
    );
    for (ix, chip) in chips.iter().enumerate() {
      let label = if chip.name.is_empty() {
        chip.email.clone()
      } else {
        chip.name.clone()
      };
      let tone = if chip.valid { TEXT } else { RED };
      row = row.child(
        h_flex()
          .id(((if to { "to-chip" } else { "cc-chip" }), ix))
          .h(px(CHIP))
          .pl_2()
          .pr_1()
          .gap_1()
          .items_center()
          .rounded_full()
          .bg(rgb(FIELD))
          .text_size(px(12.))
          .text_color(rgb(tone))
          .child(label)
          .child(
            div()
              .id(((if to { "to-chip-x" } else { "cc-chip-x" }), ix))
              .cursor_pointer()
              .on_click(cx.listener(move |this, _, _, cx| this.remove_chip(to, ix, cx)))
              .child(icon(Icon::X, 12., MUTED)),
          ),
      );
    }
    row = row.child(
      div()
        .flex_1()
        .min_w(px(80.))
        .child(Input::new(input).appearance(false).bordered(false).disabled(sending)),
    );
    let mut block = v_flex().relative().gap_1().border_b_1().border_color(rgb(BORDER)).pb_1();
    block = block.child(row);
    if !people.is_empty() {
      let mut list = v_flex().p_1();
      for (ix, person) in people.iter().enumerate() {
        let name = person.name.clone();
        let email = person.email.clone();
        let selected = ix == pick;
        list = list.child(
          h_flex()
            .id(((if to { "to-suggest" } else { "cc-suggest" }), ix))
            .h(px(SUGGEST))
            .px_2()
            .gap_2()
            .items_center()
            .rounded(px(8.))
            .cursor_pointer()
            .when(selected, |d| d.bg(rgb(ROW_SELECTED)))
            .when(!selected, |d| d.hover(|s| s.bg(rgb(ROW_HOVER))))
            .on_click(cx.listener(move |this, _, window, cx| {
              this.add_person(to, name.clone(), email.clone(), window, cx);
            }))
            .child(
              v_flex()
                .min_w_0()
                .child(
                  div()
                    .text_size(px(13.))
                    .text_color(rgb(TEXT))
                    .truncate()
                    .child(person.name.clone()),
                )
                .child(
                  div()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .truncate()
                    .child(person.email.clone()),
                ),
            ),
        );
      }
      // A floating layer under the field: the rest of the form stays where it is.
      block = block.child(
        deferred(
          div()
            .absolute()
            .top_full()
            .left_0()
            .right_0()
            .mt_1()
            .rounded(px(10.))
            .bg(rgb(POPOVER))
            .border_1()
            .border_color(rgb(BORDER))
            .occlude()
            .child(list),
        )
        .with_priority(1),
      );
    }
    block
  }

  fn compose_body(&self, sending: bool, cx: &Context<Self>) -> Div {
    let Some(form) = self.compose.as_ref() else {
      return v_flex();
    };
    let settings = cx.global::<Settings>();
    let rewrite = settings.rewrite != RewriteTool::Off;
    let always = settings.rewrite_always;
    let busy = self.rewriting;
    h_flex()
      .w_full()
      .items_end()
      .gap_2()
      .child(
        div().flex_1().min_w_0().px_1().text_size(px(14.)).child(
          Textarea::new(&form.body)
            .appearance(false)
            .bordered(false)
            .disabled(sending || busy),
        ),
      )
      .when(rewrite, |row| {
        row.child(crate::rewrite::button(
          "mail-compose-rewrite",
          "mail-compose-rewrite-pulse",
          busy,
          always,
          cx,
          |this, _, window, cx| this.rewrite_body(false, window, cx),
        ))
      })
  }

  fn on_compose_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
    let Some(form) = self.compose.as_ref() else {
      return;
    };
    let fields = [
      form.to_input.read(cx).focus_handle(cx),
      form.cc_input.read(cx).focus_handle(cx),
      form.subject.read(cx).focus_handle(cx),
      form.body.read(cx).focus_handle(cx),
    ];
    let Some(at) = fields.iter().position(|field| field.is_focused(window)) else {
      return;
    };
    let stroke = &event.keystroke;
    if stroke.key == "tab" && !stroke.modifiers.platform && !stroke.modifiers.control && !stroke.modifiers.alt {
      // To, Cc, Subject, body; Shift goes back. The ends stay put.
      let next = if stroke.modifiers.shift {
        at.checked_sub(1)
      } else {
        (at + 1 < fields.len()).then_some(at + 1)
      };
      if at < 2 {
        self.commit_pending(at == 0, window, cx);
      }
      if let Some(next) = next {
        window.focus(&fields[next], cx);
      }
      cx.stop_propagation();
      cx.notify();
      return;
    }
    if at > 1 {
      return;
    }
    let to = at == 0;
    let query = if to {
      form.to_input.read(cx).value().to_string()
    } else {
      form.cc_input.read(cx).value().to_string()
    };
    let len = self.shown_suggestions(to, &query, cx).len();
    let row = if to { form.to_row } else { form.cc_row };
    let field_len = if to { form.to.len() } else { form.cc.len() };
    match stroke.key.as_str() {
      // Escape closes the suggestions first; with none open the form's own Escape applies.
      "escape" if len > 0 => {
        if let Some(form) = self.compose.as_mut() {
          form.hide_suggest = true;
        }
      },
      "up" if row > 0 => {
        if let Some(form) = self.compose.as_mut() {
          if to {
            form.to_row -= 1;
          } else {
            form.cc_row -= 1;
          }
        }
      },
      "down" if row + 1 < len => {
        if let Some(form) = self.compose.as_mut() {
          if to {
            form.to_row += 1;
          } else {
            form.cc_row += 1;
          }
        }
      },
      "backspace" if query.is_empty() && field_len > 0 => self.remove_chip(to, field_len - 1, cx),
      _ => return,
    }
    cx.stop_propagation();
    cx.notify();
  }

  /// New email in the toolbar.
  pub(crate) fn compose_control(&self, cx: &Context<Self>) -> gpui_kit::Stateful<Div> {
    pill("mail-new")
      .gap_1p5()
      .child(icon(Icon::Pencil, 14., TEXT))
      .child("New email")
      .tooltip({
        let tip = format!("New email ({})", keys::text("c"));
        move |window, cx| Tooltip::new(tip.clone()).build(window, cx)
      })
      .on_click(cx.listener(|this, _, window, cx| this.open_compose(window, cx)))
  }
}
