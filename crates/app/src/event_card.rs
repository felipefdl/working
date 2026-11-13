//! An event's card: what it is, when, where, who is invited, and what can be done with it. It floats where the event
//! was pressed, over the whole window; a press outside closes it.

use gchat_core::{Calendar, CalendarEvent, Command, EditScope, EventKind, Guest, Reminder, Rsvp, Visibility};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::calendar::ask_scope;
use crate::calendar_layout::when_line;
use crate::conv_list::hint;
use crate::menu::{self, MenuItem};
use crate::store::Store;
use crate::theme::{ACCENT, ACCENT_BG, BG, BORDER, GREEN, MUTED, POPOVER, RED, ROW_HOVER, TEXT, data_color, rgb};
use crate::ui::{Icon, icon};

const WIDTH: f32 = 380.0;
/// Gap kept between the card and the window's edges.
const MARGIN: f32 = 12.0;
/// Guests listed before the rest are counted.
const GUESTS_SHOWN: usize = 8;

pub enum CardEvent {
  Close,
  /// Edit the event, or with `EditScope::All` its whole series.
  Edit(EditScope),
}

pub struct EventCard {
  store: Entity<Store>,
  event: CalendarEvent,
  calendar: Option<Calendar>,
  /// The color the event is painted in.
  color: u32,
  /// Where the event was pressed.
  at: Point<Pixels>,
}

impl EventEmitter<CardEvent> for EventCard {}

/// A line of the card: an icon, then what it says.
fn line(glyph: Icon) -> Div {
  h_flex()
    .gap_3()
    .items_start()
    .child(div().h(px(20.)).flex().items_center().child(icon(glyph, 16., MUTED)))
}

fn body(text: impl Into<SharedString>) -> Div {
  div()
    .flex_1()
    .min_w_0()
    .text_size(px(13.))
    .line_height(px(20.))
    .text_color(rgb(TEXT))
    .child(text.into())
}

fn action(id: &'static str, glyph: Icon, tip: &'static str) -> Stateful<Div> {
  div()
    .id(id)
    .size(px(30.))
    .flex()
    .items_center()
    .justify_center()
    .rounded_full()
    .cursor_pointer()
    .hover(|style| style.bg(rgb(ROW_HOVER)))
    .tooltip(hint(tip))
    .child(icon(glyph, 16., TEXT))
}

/// `3 guests: 2 yes, 1 awaiting`.
fn guest_summary(guests: &[Guest]) -> String {
  let count = |rsvp: Rsvp| guests.iter().filter(|guest| guest.rsvp == rsvp).count();
  let parts: Vec<String> = [
    (Rsvp::Accepted, "yes"),
    (Rsvp::Declined, "no"),
    (Rsvp::Tentative, "maybe"),
    (Rsvp::NeedsAction, "awaiting"),
  ]
  .into_iter()
  .filter(|(rsvp, _)| count(*rsvp) > 0)
  .map(|(rsvp, word)| format!("{} {word}", count(rsvp)))
  .collect();
  let noun = if guests.len() == 1 { "guest" } else { "guests" };
  format!("{} {noun}: {}", guests.len(), parts.join(", "))
}

fn reminder_text(reminder: Reminder) -> Option<String> {
  let Reminder::Minutes(minutes) = reminder else {
    return None;
  };
  Some(match minutes {
    0 => "At the time of the event".into(),
    1..60 => format!("{minutes} minutes before"),
    60 => "1 hour before".into(),
    1440 => "1 day before".into(),
    _ if minutes % 1440 == 0 => format!("{} days before", minutes / 1440),
    _ if minutes % 60 == 0 => format!("{} hours before", minutes / 60),
    _ => format!("{minutes} minutes before"),
  })
}

impl EventCard {
  pub const fn new(
    store: Entity<Store>,
    event: CalendarEvent,
    calendar: Option<Calendar>,
    color: u32,
    at: Point<Pixels>,
  ) -> Self {
    Self { store, event, calendar, color, at }
  }

  pub const fn event(&self) -> &CalendarEvent {
    &self.event
  }

  /// The user organizes it and others are invited: deleting it can email them.
  fn hosts_guests(&self) -> bool {
    let mine = self.event.guests.iter().any(|guest| guest.me && guest.organizer);
    mine && self.event.guests.iter().any(|guest| !guest.me)
  }

  fn delete(&self, scope: EditScope, notify: bool, cx: &mut Context<Self>) {
    let command = Command::DeleteEvent {
      calendar_id: self.event.calendar_id.clone(),
      event_id: self.event.id.clone(),
      scope,
      notify,
    };
    self.store.update(cx, |store, cx| store.write_calendar(command, cx));
    cx.emit(CardEvent::Close);
  }

  fn respond(&self, rsvp: Rsvp, scope: EditScope, cx: &mut Context<Self>) {
    let command = Command::RespondEvent {
      calendar_id: self.event.calendar_id.clone(),
      event_id: self.event.id.clone(),
      scope,
      rsvp,
    };
    self.store.update(cx, |store, cx| store.write_calendar(command, cx));
    cx.emit(CardEvent::Close);
  }

  /// Deletes after asking what a recurring event needs (this one or all) and what an event with guests needs (email
  /// them or not).
  fn ask_delete(&self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
    let (card, hosts) = (cx.entity(), self.hosts_guests());
    ask_scope(&self.event, at, window, cx, move |scope, window, cx| {
      if !hosts {
        card.update(cx, |this, cx| this.delete(scope, false, cx));
        return;
      }
      let choice = |label: &'static str, notify: bool| {
        let card = card.clone();
        MenuItem::new(label, move |_, cx| card.update(cx, |this, cx| this.delete(scope, notify, cx)))
      };
      let items = vec![choice("Delete, email guests", true), choice("Delete, no email", false)];
      menu::show(at, items, window, cx);
    });
  }

  fn actions(&self, cx: &Context<Self>) -> Div {
    let link = self.event.link.clone();
    h_flex()
      .justify_end()
      .when(self.event.can_edit, |row| {
        row.child(action("event-edit", Icon::Pencil, "Edit").on_mouse_down(
          MouseButton::Left,
          cx.listener(|this, event: &MouseDownEvent, window, cx| {
            cx.stop_propagation();
            let card = cx.entity();
            ask_scope(&this.event, event.position, window, cx, move |scope, _, cx| {
              card.update(cx, |_, cx| cx.emit(CardEvent::Edit(scope)));
            });
          }),
        ))
      })
      .when(self.event.can_delete, |row| {
        row.child(action("event-delete", Icon::Trash, "Delete").on_mouse_down(
          MouseButton::Left,
          cx.listener(|this, event: &MouseDownEvent, window, cx| {
            cx.stop_propagation();
            this.ask_delete(event.position, window, cx);
          }),
        ))
      })
      .when(!link.is_empty(), |row| {
        row.child(
          action("event-open", Icon::ExternalLink, "Open in Google Calendar")
            .on_click(move |_, _, cx| cx.open_url(&link)),
        )
      })
      .child(action("event-close", Icon::X, "Close").on_click(cx.listener(|_, _, _, cx| cx.emit(CardEvent::Close))))
  }

  fn heading(&self) -> Div {
    let kind = match self.event.kind {
      EventKind::Event => None,
      EventKind::OutOfOffice => Some("Out of office"),
      EventKind::FocusTime => Some("Focus time"),
      EventKind::WorkingLocation => Some("Working location"),
      EventKind::Birthday => Some("Birthday"),
    };
    let when = when_line(&self.event);
    h_flex()
      .gap_3()
      .items_start()
      .child(
        div()
          .h(px(26.))
          .w(px(16.))
          .flex()
          .items_center()
          .child(div().size(px(14.)).rounded(px(4.)).bg(data_color(self.color))),
      )
      .child(
        v_flex()
          .flex_1()
          .min_w_0()
          .gap_0p5()
          .child(
            div()
              .text_size(px(19.))
              .line_height(px(26.))
              .text_color(rgb(TEXT))
              .child(self.event.title.clone()),
          )
          .child(div().text_size(px(13.)).text_color(rgb(MUTED)).child(when))
          .children(kind.map(|kind| div().text_size(px(12.)).text_color(rgb(ACCENT)).child(kind))),
      )
  }

  fn meet(url: &str) -> Div {
    let (join, copy) = (url.to_string(), url.to_string());
    let shown = url.trim_start_matches("https://").to_string();
    line(Icon::Video).items_center().child(
      v_flex()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(
          h_flex()
            .gap_1()
            .items_center()
            .child(
              div()
                .id("event-join")
                .h(px(32.))
                .px_3()
                .flex()
                .items_center()
                .rounded_full()
                .bg(rgb(ACCENT))
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(BG))
                .cursor_pointer()
                .hover(|style| style.opacity(0.9))
                .on_click(move |_, _, cx| crate::call::open(&join, cx))
                .child("Join with Google Meet"),
            )
            .child(
              action("event-copy-meet", Icon::Copy, "Copy the meeting link")
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))),
            ),
        )
        .child(div().truncate().text_size(px(12.)).text_color(rgb(MUTED)).child(shown)),
    )
  }

  fn guests(&self) -> Div {
    let guests = &self.event.guests;
    let mut list = v_flex().flex_1().min_w_0().child(body(guest_summary(guests)));
    for guest in guests.iter().take(GUESTS_SHOWN) {
      let name = if guest.name.is_empty() {
        guest.email.clone()
      } else {
        guest.name.clone()
      };
      let mark = match guest.rsvp {
        Rsvp::Accepted => icon(Icon::Check, 13., GREEN).into_any_element(),
        Rsvp::Declined => icon(Icon::X, 13., RED).into_any_element(),
        Rsvp::Tentative => div().text_size(px(12.)).text_color(rgb(MUTED)).child("?").into_any_element(),
        Rsvp::NeedsAction => div().into_any_element(),
      };
      list = list.child(
        h_flex()
          .h(px(24.))
          .gap_2()
          .items_center()
          .child(div().w(px(14.)).flex().justify_center().child(mark))
          .child(div().min_w_0().truncate().text_size(px(13.)).text_color(rgb(TEXT)).child(name))
          .when(guest.organizer, |row| {
            row.child(
              div()
                .flex_shrink_0()
                .text_size(px(11.))
                .text_color(rgb(MUTED))
                .child("Organizer"),
            )
          })
          .when(guest.optional, |row| {
            row.child(
              div()
                .flex_shrink_0()
                .text_size(px(11.))
                .text_color(rgb(MUTED))
                .child("Optional"),
            )
          }),
      );
    }
    if guests.len() > GUESTS_SHOWN {
      list = list.child(
        div()
          .h(px(24.))
          .pl(px(22.))
          .flex()
          .items_center()
          .text_size(px(12.))
          .text_color(rgb(MUTED))
          .child(format!("and {} more", guests.len() - GUESTS_SHOWN)),
      );
    }
    line(Icon::Users).child(list)
  }

  /// Where it is kept, and what the calendar shows of it.
  fn calendar_line(&self) -> Div {
    let mut parts = Vec::new();
    if let Some(calendar) = &self.calendar {
      parts.push(calendar.name.clone());
    }
    if !self.event.organizer.is_empty() && self.event.guests.is_empty() && self.calendar.is_none() {
      parts.push(self.event.organizer.clone());
    }
    if !self.event.busy && !self.event.all_day {
      parts.push("Free".into());
    }
    if self.event.visibility == Visibility::Private {
      parts.push("Private".into());
    }
    line(Icon::Calendar).child(body(parts.join(" ⋅ ")))
  }

  /// The user's answer to the invitation: the one given is marked, and a press changes it.
  fn answer(&self, cx: &Context<Self>) -> Div {
    let given = self.event.my_rsvp;
    let choice = |id: &'static str, label: &'static str, rsvp: Rsvp| {
      let on = given == Some(rsvp);
      div()
        .id(id)
        .h(px(30.))
        .px_3()
        .flex()
        .items_center()
        .rounded_full()
        .text_size(px(13.))
        .cursor_pointer()
        .when(on, |choice| choice.bg(rgb(ACCENT_BG)).text_color(rgb(ACCENT)))
        .when(!on, |choice| {
          choice
            .border_1()
            .border_color(rgb(BORDER))
            .text_color(rgb(TEXT))
            .hover(|style| style.bg(rgb(ROW_HOVER)))
        })
        .on_mouse_down(
          MouseButton::Left,
          cx.listener(move |this, event: &MouseDownEvent, window, cx| {
            cx.stop_propagation();
            let card = cx.entity();
            ask_scope(&this.event, event.position, window, cx, move |scope, _, cx| {
              card.update(cx, |this, cx| this.respond(rsvp, scope, cx));
            });
          }),
        )
        .child(label)
    };
    h_flex()
      .pt_3()
      .gap_2()
      .items_center()
      .border_t_1()
      .border_color(rgb(BORDER))
      .child(div().flex_1().text_size(px(13.)).text_color(rgb(MUTED)).child("Going?"))
      .child(choice("event-yes", "Yes", Rsvp::Accepted))
      .child(choice("event-no", "No", Rsvp::Declined))
      .child(choice("event-maybe", "Maybe", Rsvp::Tentative))
  }
}

impl Render for EventCard {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let event = &self.event;
    let max_height = (window.viewport_size().height - px(2. * MARGIN)).max(px(200.));
    // Beside the pointer; once its height is known, the card is moved to stay inside the window.
    let at = point(self.at.x + px(MARGIN), self.at.y - px(40.));

    let details = v_flex()
      .id("event-details")
      .px_4()
      .pb_4()
      .gap_3()
      .overflow_y_scroll()
      .child(self.heading())
      .when(event.series.is_some(), |card| {
        card.child(line(Icon::Repeat).child(body("Repeats")))
      })
      .children(event.meet_url.as_deref().map(Self::meet))
      .when(!event.location.is_empty(), |card| {
        card.child(line(Icon::MapPin).child(body(event.location.clone())))
      })
      .when(!event.guests.is_empty(), |card| card.child(self.guests()))
      .when(!event.description.is_empty(), |card| {
        card.child(line(Icon::Text).child(body(event.description.clone())))
      })
      .children(reminder_text(event.reminder).map(|text| line(Icon::Bell).child(body(text))))
      .child(self.calendar_line())
      .when(event.my_rsvp.is_some(), |card| card.child(self.answer(cx)));

    div()
      .absolute()
      .top_0()
      .left_0()
      .size_full()
      .occlude()
      .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(CardEvent::Close)))
      .on_mouse_down(MouseButton::Right, cx.listener(|_, _, _, cx| cx.emit(CardEvent::Close)))
      .child(
        anchored().position(at).snap_to_window_with_margin(px(MARGIN)).child(
          v_flex()
            .w(px(WIDTH))
            .max_h(max_height)
            .rounded(px(14.))
            .bg(rgb(POPOVER))
            .border_1()
            .border_color(rgb(BORDER))
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(self.actions(cx).px_2().pt_2().bg(rgb(POPOVER)))
            .child(details),
        ),
      )
  }
}
