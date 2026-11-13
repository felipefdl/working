use std::time::Duration;

use super::data::{APPROVE, BUILD_BOT, ME};
use super::{Demo, Window};
use crate::model::{Chip, ChipKind, Command, Event, GroupId, Shortcut};

fn demo() -> (Demo, flume::Receiver<Event>) {
  let (events, received) = flume::unbounded();
  (Demo::new(events, 1_800_000_000_000_000), received)
}

fn drain(events: &flume::Receiver<Event>) -> Vec<Event> {
  events.try_iter().collect()
}

/// Runs every scheduled action, as if their time had come.
fn run_all(demo: &mut Demo) {
  for _ in 0..4 {
    for (at, _) in &mut demo.timeline {
      *at -= Duration::from_secs(600);
    }
    demo.run_due();
  }
}

fn release() -> GroupId {
  GroupId::Space("demo-release".into())
}

#[test]
fn open_pages_back_to_the_first_message() {
  let (mut demo, events) = demo();
  let total = demo.messages(&release()).len();
  demo.handle(Command::Open(release()));
  let mut held = 0;
  let mut pages = 0;
  loop {
    let Some(Event::History {
      messages,
      reached_start,
      reached_end,
      prepended,
      ..
    }) = drain(&events).pop()
    else {
      panic!("no history");
    };
    assert!(reached_end);
    if held > 0 {
      assert_eq!(messages.len(), held + prepended);
    }
    held = messages.len();
    if reached_start {
      break;
    }
    pages += 1;
    demo.handle(Command::LoadOlder(release()));
  }
  assert_eq!(held, total);
  assert!(pages > 2, "the release space is meant to page");
}

#[test]
fn jump_opens_a_window_that_loads_newer() {
  let (mut demo, events) = demo();
  let target = demo.messages(&release()).get(20).map(|message| message.id.clone()).unwrap();
  demo.handle(Command::Jump {
    group: release(),
    message_id: target.clone(),
    time: 0,
  });
  let Some(Event::History { reached_end, focus, .. }) = drain(&events).pop() else {
    panic!("no history");
  };
  assert!(!reached_end);
  assert_eq!(focus, Some(target));
  for _ in 0..10 {
    demo.handle(Command::LoadNewer(release()));
  }
  assert!(matches!(demo.windows.get(&release()), Some(Window { end: None, .. })));
}

#[test]
fn send_confirms_and_the_person_replies() {
  let (mut demo, events) = demo();
  let group = GroupId::Dm("demo-riley-dm".into());
  demo.handle(Command::Open(group.clone()));
  drain(&events);
  demo.handle(Command::Send {
    group,
    topic_id: None,
    text: "Hello".into(),
    quote: None,
    files: Vec::new(),
    chips: Vec::new(),
  });
  run_all(&mut demo);
  let events = drain(&events);
  let pending = events.iter().find_map(|event| match event {
    Event::MessageUpsert { message, .. } if message.pending => Some(message.id.clone()),
    _ => None,
  });
  let confirmed = events.iter().any(
    |event| matches!(event, Event::MessageUpsert { message, replaces, .. } if !message.pending && *replaces == pending),
  );
  assert!(confirmed, "the pending message is replaced");
  assert!(events.iter().any(|event| matches!(event, Event::Typing { typing: true, .. })));
  assert!(events.iter().any(|event| matches!(
    event,
    Event::MessageUpsert { message, incoming: true, .. } if message.sender_id == "demo-riley"
  )));
}

#[test]
fn slash_command_and_card_button_answer() {
  let (mut demo, events) = demo();
  let group = GroupId::Dm("demo-build-bot-dm".into());
  demo.handle(Command::Send {
    group: group.clone(),
    topic_id: None,
    text: "/deploy".into(),
    quote: None,
    files: Vec::new(),
    chips: vec![Chip {
      range: 0..7,
      kind: ChipKind::Command {
        bot_id: BUILD_BOT.into(),
        command_id: 2,
        name: "/deploy".into(),
      },
    }],
  });
  run_all(&mut demo);
  let card = drain(&events).into_iter().find_map(|event| match event {
    Event::MessageUpsert { message, .. } if message.sender_id == BUILD_BOT => Some(message),
    _ => None,
  });
  let card = card.expect("the app answers");
  let held = card.cards.first().unwrap();
  let action = held.sections.first().unwrap().buttons.first().unwrap().action.clone().unwrap();
  assert_eq!(action.method, APPROVE);
  demo.handle(Command::ClickCard {
    group,
    topic_id: card.topic_id.clone(),
    message_id: card.id.clone(),
    card_id: held.id.clone(),
    action,
  });
  let updated = drain(&events).into_iter().any(|event| {
    matches!(event, Event::MessageUpsert { message, .. }
      if message.id == card.id && message.cards.iter().all(|card| card.sections.iter().all(|s| s.buttons.iter().all(|b| b.action.is_none()))))
  });
  assert!(updated, "the card drops its buttons once answered");
}

#[test]
fn shortcuts_and_search_find_messages() {
  let (mut demo, events) = demo();
  demo.handle(Command::LoadShortcut {
    shortcut: Shortcut::Mentions,
    cursor: None,
  });
  demo.handle(Command::LoadShortcut {
    shortcut: Shortcut::Starred,
    cursor: None,
  });
  demo.handle(Command::Search("embargo".into()));
  let events = drain(&events);
  let count = |wanted: Shortcut| {
    events.iter().find_map(|event| match event {
      Event::Shortcut { shortcut, hits, .. } if *shortcut == wanted => Some(hits.len()),
      _ => None,
    })
  };
  assert!(count(Shortcut::Mentions).is_some_and(|hits| hits >= 2));
  assert!(count(Shortcut::Starred).is_some_and(|hits| hits >= 1));
  assert!(
    events
      .iter()
      .any(|event| matches!(event, Event::SearchResults { hits, .. } if hits.len() == 1))
  );
}

#[test]
fn react_star_edit_delete_and_read() {
  let (mut demo, events) = demo();
  let group = GroupId::Dm("demo-jordan-dm".into());
  let first = demo.messages(&group).first().unwrap().clone();
  demo.handle(Command::React {
    group: group.clone(),
    topic_id: first.topic_id.clone(),
    message_id: first.id.clone(),
    emoji: "🎉".into(),
    add: true,
  });
  demo.handle(Command::Star {
    group: group.clone(),
    topic_id: first.topic_id.clone(),
    message_id: first.id.clone(),
    starred: true,
  });
  let held = demo.held(&group, &first.id).unwrap();
  assert!(held.starred);
  assert!(held.reactions.iter().any(|reaction| reaction.emoji == "🎉" && reaction.mine));
  demo.handle(Command::Edit {
    group: group.clone(),
    message_id: first.id.clone(),
    text: "Changed".into(),
  });
  assert!(
    demo
      .held(&group, &first.id)
      .is_some_and(|message| message.edited && message.text == "Changed")
  );
  demo.handle(Command::Delete {
    group: group.clone(),
    message_id: first.id.clone(),
  });
  assert!(demo.held(&group, &first.id).is_none());
  assert!(demo.groups.get(&group).unwrap().unread);
  demo.handle(Command::MarkRead(group.clone()));
  assert!(!demo.groups.get(&group).unwrap().unread);
  assert!(drain(&events).iter().any(|event| matches!(event, Event::MessageDeleted { .. })));
}

#[test]
fn create_chat_finds_or_makes_one() {
  let (mut demo, events) = demo();
  demo.handle(Command::CreateChat {
    user_ids: vec!["demo-jordan".into(), ME.into()],
  });
  assert!(matches!(drain(&events).last(), Some(Event::OpenChat(GroupId::Dm(id))) if id == "demo-jordan-dm"));
  demo.handle(Command::CreateChat {
    user_ids: vec!["demo-mei".into(), "demo-noah".into()],
  });
  let events = drain(&events);
  assert!(
    events
      .iter()
      .any(|event| matches!(event, Event::GroupUpsert(group) if group.name == "Mei, Noah"))
  );
  assert!(matches!(events.last(), Some(Event::OpenChat(_))));
}

#[test]
fn creating_a_meeting_answers_a_link_of_its_own_each_time() {
  let (mut demo, events) = demo();
  demo.handle(Command::CreateMeeting);
  demo.handle(Command::CreateMeeting);
  let links: Vec<String> = drain(&events)
    .into_iter()
    .map(|event| match event {
      Event::MeetingCreated(Ok(link)) => link,
      other => panic!("only the meetings are answered, got {other:?}"),
    })
    .collect();
  assert_eq!(links.len(), 2);
  assert!(links.iter().all(|link| link.starts_with("https://meet.google.com/demo-")));
  assert_ne!(links[0], links[1]);
}

/// Asks for the events of the week that holds the demo's "today", and answers what came back.
fn week_of_events(demo: &mut Demo, events: &flume::Receiver<Event>) -> Vec<crate::model::CalendarEvent> {
  let day = 24 * 3600 * 1_000_000_i64;
  let now = 1_800_000_000_000_000;
  demo.handle(Command::LoadEvents { start: now - 3 * day, end: now + 4 * day });
  let Some(Event::CalendarEvents { events: held, .. }) = drain(events).pop() else {
    panic!("no calendar events");
  };
  held.to_vec()
}

#[test]
fn the_demo_calendar_lists_calendars_and_sorted_events() {
  let (mut demo, events) = demo();
  demo.handle(Command::LoadCalendars);
  let Some(Event::Calendars(calendars)) = drain(&events).pop() else {
    panic!("no calendars");
  };
  assert!(calendars.first().is_some_and(|calendar| calendar.primary));
  assert!(calendars.iter().any(|calendar| !calendar.access.can_write()));
  let held = week_of_events(&mut demo, &events);
  assert!(held.len() > 3, "the week around today has events");
  assert!(held.windows(2).all(|pair| pair[0].start <= pair[1].start));
  // Reads only reach the range asked for.
  assert!(
    held
      .iter()
      .all(|event| event.end > 1_800_000_000_000_000 - 3 * 24 * 3600 * 1_000_000)
  );
}

#[test]
fn saving_creates_edits_and_deletes_an_event() {
  use crate::model::{EditScope, EventDraft};
  let (mut demo, events) = demo();
  let start = 1_800_000_000_000_000 + 3_600_000_000;
  let draft = EventDraft {
    calendar_id: "amorgan@example.com".into(),
    title: "Roadmap review".into(),
    start,
    end: start + 1_800_000_000,
    busy: true,
    guests: vec!["jpark@example.com".into()],
    add_meet: true,
    ..EventDraft::default()
  };
  demo.handle(Command::LoadEvents {
    start: start - 86_400_000_000,
    end: start + 86_400_000_000,
  });
  drain(&events);
  demo.handle(Command::SaveEvent(Box::new(draft)));
  let answered = drain(&events);
  assert!(matches!(answered.first(), Some(Event::CalendarWritten(Ok(())))));
  let Some(Event::CalendarEvents { events: held, .. }) = answered.into_iter().last() else {
    panic!("the range is not sent again after a write");
  };
  let created = held.iter().find(|event| event.title == "Roadmap review").expect("created");
  assert!(created.meet_url.is_some());
  assert_eq!(created.guests.len(), 2, "the user and the guest");

  let mut changed = EventDraft {
    calendar_id: created.calendar_id.clone(),
    editing: Some((created.calendar_id.clone(), created.id.clone())),
    scope: EditScope::This,
    title: "Roadmap review, moved".into(),
    start: created.start + 3_600_000_000,
    end: created.end + 3_600_000_000,
    busy: true,
    ..EventDraft::default()
  };
  changed.guests = vec!["jpark@example.com".into()];
  demo.handle(Command::SaveEvent(Box::new(changed)));
  let Some(Event::CalendarEvents { events: held, .. }) = drain(&events).into_iter().last() else {
    panic!("no events after the edit");
  };
  assert!(held.iter().all(|event| event.title != "Roadmap review"));
  let moved = held
    .iter()
    .find(|event| event.title == "Roadmap review, moved")
    .expect("renamed");
  assert_eq!(moved.start, created.start + 3_600_000_000);

  demo.handle(Command::DeleteEvent {
    calendar_id: moved.calendar_id.clone(),
    event_id: moved.id.clone(),
    scope: EditScope::This,
    notify: false,
  });
  let Some(Event::CalendarEvents { events: held, .. }) = drain(&events).into_iter().last() else {
    panic!("no events after the deletion");
  };
  assert!(held.iter().all(|event| event.title != "Roadmap review, moved"));
}

#[test]
fn a_series_edit_reaches_every_occurrence_and_a_single_one_only_itself() {
  use crate::model::{EditScope, EventDraft};
  let (mut demo, events) = demo();
  let day = 24 * 3600 * 1_000_000_i64;
  let now = 1_800_000_000_000_000;
  demo.handle(Command::LoadEvents {
    start: now - 30 * day,
    end: now + 30 * day,
  });
  let Some(Event::CalendarEvents { events: held, .. }) = drain(&events).pop() else {
    panic!("no events");
  };
  let standups: Vec<_> = held.iter().filter(|event| event.title == "Daily standup").collect();
  assert!(standups.len() > 10, "the standup repeats on weekdays");
  let picked = standups[3];
  let draft = |scope| EventDraft {
    calendar_id: picked.calendar_id.clone(),
    editing: Some((picked.calendar_id.clone(), picked.id.clone())),
    scope,
    title: "Standup, retitled".into(),
    start: picked.start,
    end: picked.end,
    busy: true,
    ..EventDraft::default()
  };
  demo.handle(Command::SaveEvent(Box::new(draft(EditScope::This))));
  let Some(Event::CalendarEvents { events: held, .. }) = drain(&events).into_iter().last() else {
    panic!("no events");
  };
  assert_eq!(held.iter().filter(|event| event.title == "Standup, retitled").count(), 1);
  demo.handle(Command::SaveEvent(Box::new(draft(EditScope::All))));
  let Some(Event::CalendarEvents { events: held, .. }) = drain(&events).into_iter().last() else {
    panic!("no events");
  };
  assert_eq!(held.iter().filter(|event| event.title == "Daily standup").count(), 0);
  assert!(held.iter().filter(|event| event.title == "Standup, retitled").count() > 10);
}

#[test]
fn answering_an_invitation_updates_the_users_response() {
  use crate::model::{EditScope, Rsvp};
  let (mut demo, events) = demo();
  let day = 24 * 3600 * 1_000_000_i64;
  let now = 1_800_000_000_000_000;
  demo.handle(Command::LoadEvents { start: now, end: now + 14 * day });
  let Some(Event::CalendarEvents { events: held, .. }) = drain(&events).pop() else {
    panic!("no events");
  };
  let waiting = held
    .iter()
    .find(|event| event.my_rsvp == Some(Rsvp::NeedsAction))
    .expect("a coming invitation waits for an answer");
  demo.handle(Command::RespondEvent {
    calendar_id: waiting.calendar_id.clone(),
    event_id: waiting.id.clone(),
    scope: EditScope::This,
    rsvp: Rsvp::Declined,
  });
  let Some(Event::CalendarEvents { events: held, .. }) = drain(&events).into_iter().last() else {
    panic!("no events");
  };
  let answered = held.iter().find(|event| event.id == waiting.id).expect("still listed");
  assert_eq!(answered.my_rsvp, Some(Rsvp::Declined));
  assert!(answered.guests.iter().any(|guest| guest.me && guest.rsvp == Rsvp::Declined));
}

#[test]
fn a_write_to_a_gone_event_says_so() {
  use crate::model::EditScope;
  let (mut demo, events) = demo();
  demo.handle(Command::DeleteEvent {
    calendar_id: "amorgan@example.com".into(),
    event_id: "no-such-event".into(),
    scope: EditScope::This,
    notify: false,
  });
  assert!(matches!(drain(&events).first(), Some(Event::CalendarWritten(Err(_)))));
}
