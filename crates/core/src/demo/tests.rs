use std::time::Duration;

use super::data::{APPROVE, BUILD_BOT, ME};
use super::{Action, Demo, Window};
use crate::mail::SignaturePlace;
use crate::model::{
  AttachmentKind, Chip, ChipKind, Command, Event, GroupId, MailAddress, MailCompose, MailReply, MailThread, Mailbox,
  Message, ReadReceipt, Shortcut, SpanKind,
};

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

/// A demo made at the real clock's now, so what the user sends during a test is the newest message of a conversation.
/// (`demo` is dated after the clock, where a message sent now would sort before the whole workspace.)
fn live_demo() -> (Demo, flume::Receiver<Event>) {
  let (events, received) = flume::unbounded();
  (Demo::new(events, super::now_micros()), received)
}

/// The receipts the last `ReadReceipts` event of `group` carried.
fn receipts_of(events: &[Event], group: &GroupId) -> Option<Vec<ReadReceipt>> {
  events.iter().rev().find_map(|event| match event {
    Event::ReadReceipts { group: of, receipts, .. } if of == group => Some(receipts.to_vec()),
    _ => None,
  })
}

/// Where `who` has read up to, among `receipts`.
fn read_to(receipts: &[ReadReceipt], who: &str) -> Option<i64> {
  receipts
    .iter()
    .find(|receipt| receipt.user_id == who)
    .map(|receipt| receipt.read_time)
}

#[test]
fn opening_a_conversation_says_how_far_its_other_members_have_read() {
  let (mut demo, events) = demo();
  let group = GroupId::Dm("demo-riley-dm".into());
  demo.handle(Command::Open(group.clone()));
  let events = drain(&events);
  let receipts_at = events
    .iter()
    .position(|event| matches!(event, Event::ReadReceipts { group: of, enabled: true, .. } if *of == group))
    .expect("the receipts are sent");
  let history_at = events
    .iter()
    .position(|event| matches!(event, Event::History { .. }))
    .expect("so is the history");
  assert!(receipts_at < history_at, "they arrive before the messages they sit beside");
  let receipts = receipts_of(&events, &group).unwrap();
  assert_eq!(receipts.len(), 1, "Riley is the only other member");
  let riley = receipts.first().unwrap();
  assert_eq!((riley.user_id.as_str(), riley.name.as_str()), ("demo-riley", "Riley Brooks"));
  // The user wrote the last message of the conversation, and Riley has read it.
  let last = demo.messages(&group).last().unwrap();
  assert_eq!(last.sender_id, ME);
  assert_eq!(riley.read_time, last.time);
}

#[test]
fn members_of_a_group_have_read_to_different_recent_messages_and_an_app_has_no_readers() {
  let (mut demo, events) = demo();
  let trio = GroupId::Dm("demo-trio".into());
  demo.handle(Command::Open(trio.clone()));
  let receipts = receipts_of(&drain(&events), &trio).unwrap();
  assert_eq!(receipts.len(), 3);
  assert!(receipts.iter().all(|receipt| receipt.user_id != ME), "never the signed-in user");
  assert!(
    receipts.windows(2).all(|pair| pair[0].read_time <= pair[1].read_time),
    "earliest reader first"
  );
  // Sam wrote the newest message and Jordan has read it as well; Riley stopped at an earlier one.
  let newest = demo.messages(&trio).last().unwrap().time;
  assert_eq!(read_to(&receipts, "demo-sam"), Some(newest));
  assert_eq!(read_to(&receipts, "demo-jordan"), Some(newest));
  assert!(read_to(&receipts, "demo-riley").is_some_and(|time| time < newest));
  let app = GroupId::Dm("demo-build-bot-dm".into());
  demo.handle(Command::Open(app.clone()));
  let events = drain(&events);
  assert_eq!(receipts_of(&events, &app), Some(Vec::new()));
  assert!(
    events
      .iter()
      .any(|event| matches!(event, Event::ReadReceipts { group, enabled: false, .. } if *group == app)),
    "an app's conversation has receipts off"
  );
}

#[test]
fn the_person_who_replies_has_read_what_was_sent_and_the_others_stay() {
  let (mut demo, events) = live_demo();
  let group = GroupId::Dm("demo-trio".into());
  demo.handle(Command::Open(group.clone()));
  let opened = receipts_of(&drain(&events), &group).unwrap();
  demo.handle(Command::Send {
    group: group.clone(),
    topic_id: None,
    text: "Hello".into(),
    quote: None,
    files: Vec::new(),
    chips: Vec::new(),
  });
  run_all(&mut demo);
  let events = drain(&events);
  let sent = events
    .iter()
    .find_map(|event| match event {
      Event::MessageUpsert { message, .. } if message.sender_id == ME && !message.pending => Some(message.time),
      _ => None,
    })
    .expect("the message is confirmed");
  let after = receipts_of(&events, &group).expect("the receipts move on");
  // Jordan answers a message to the group, so he has read it.
  assert!(read_to(&opened, "demo-jordan") < Some(sent));
  assert_eq!(read_to(&after, "demo-jordan"), Some(sent));
  assert_eq!(read_to(&after, "demo-riley"), read_to(&opened, "demo-riley"));
  assert_eq!(read_to(&after, "demo-sam"), read_to(&opened, "demo-sam"));
}

/// What each span of a message covers.
fn marked(message: &Message) -> Vec<(&str, SpanKind)> {
  message
    .spans
    .iter()
    .map(|span| (message.text.get(span.range.clone()).unwrap(), span.kind.clone()))
    .collect()
}

#[test]
fn sent_markers_become_formatting() {
  let (mut demo, events) = demo();
  let group = GroupId::Dm("demo-riley-dm".into());
  demo.handle(Command::Open(group.clone()));
  drain(&events);
  demo.handle(Command::Send {
    group,
    topic_id: None,
    text: "*hi* _there_".into(),
    quote: None,
    files: Vec::new(),
    chips: Vec::new(),
  });
  run_all(&mut demo);
  let mine: Vec<_> = drain(&events)
    .into_iter()
    .filter_map(|event| match event {
      Event::MessageUpsert { message, .. } if message.sender_id == ME => Some(message),
      _ => None,
    })
    .collect();
  assert!(mine.iter().any(|message| message.pending), "it shows before it is confirmed");
  assert!(mine.iter().any(|message| !message.pending), "and after");
  for message in &mine {
    assert_eq!(message.text, "hi there");
    assert_eq!(marked(message), [("hi", SpanKind::Bold), ("there", SpanKind::Italic)]);
  }
}

#[test]
fn markers_around_a_mention_move_it() {
  let (mut demo, events) = demo();
  let group = GroupId::Space("demo-release".into());
  demo.handle(Command::Send {
    group,
    topic_id: None,
    text: "*ship it* @Riley Brooks".into(),
    quote: None,
    files: Vec::new(),
    chips: vec![Chip {
      range: 10..23,
      kind: ChipKind::Mention { user_id: "demo-riley".into(), bot: false },
    }],
  });
  let pending = drain(&events)
    .into_iter()
    .find_map(|event| match event {
      Event::MessageUpsert { message, .. } if message.pending => Some(message),
      _ => None,
    })
    .expect("the message shows at once");
  assert_eq!(pending.text, "ship it @Riley Brooks");
  assert_eq!(
    marked(&pending),
    [
      ("ship it", SpanKind::Bold),
      ("@Riley Brooks", SpanKind::Mention("demo-riley".into()))
    ]
  );
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
fn edited_markers_become_formatting() {
  let (mut demo, _events) = demo();
  let group = GroupId::Dm("demo-jordan-dm".into());
  let first = demo.messages(&group).first().unwrap().clone();
  demo.handle(Command::Edit {
    group: group.clone(),
    message_id: first.id.clone(),
    text: "~old~ and `new`\n- one\n- two".into(),
  });
  let held = demo.held(&group, &first.id).unwrap();
  assert_eq!(held.text, "old and new\none\ntwo");
  assert_eq!(
    marked(&held),
    [
      ("old", SpanKind::Strike),
      ("new", SpanKind::Code),
      ("one", SpanKind::ListItem),
      ("two", SpanKind::ListItem)
    ]
  );
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

#[test]
fn conversation_actions_change_the_list() {
  let (mut demo, events) = demo();
  let group = release();
  demo.handle(Command::SetPinned { group: group.clone(), pinned: true });
  demo.handle(Command::SetMuted { group: group.clone(), muted: true });
  demo.handle(Command::MarkUnread(group.clone()));
  let held = demo.groups.get(&group).unwrap();
  assert!(held.pinned && held.muted && held.unread);
  assert!(drain(&events).iter().any(|event| matches!(
    event,
    Event::GroupUpsert(listed) if listed.id == group && listed.pinned && listed.muted && listed.unread
  )));
  demo.handle(Command::MarkRead(group.clone()));
  assert!(!demo.groups.get(&group).unwrap().unread);
  drain(&events);
  demo.handle(Command::MarkUnread(group.clone()));
  demo.handle(Command::SetHidden { group: group.clone(), hidden: true });
  let held = demo.groups.get(&group).expect("a hidden conversation stays in the world");
  assert!(held.hidden && !held.unread);
  assert!(drain(&events).iter().any(|event| matches!(
    event,
    Event::GroupUpsert(listed) if listed.id == group && listed.hidden && !listed.unread
  )));
  demo.handle(Command::SetHidden { group: group.clone(), hidden: false });
  assert!(!demo.groups.get(&group).unwrap().hidden);
  demo.handle(Command::SetHidden { group: group.clone(), hidden: true });
  drain(&events);
  demo.handle(Command::Send {
    group: group.clone(),
    topic_id: None,
    text: "Back again".into(),
    quote: None,
    files: Vec::new(),
    chips: Vec::new(),
  });
  assert!(!demo.groups.get(&group).unwrap().hidden);
  assert!(
    drain(&events)
      .iter()
      .any(|event| matches!(event, Event::GroupUpsert(listed) if listed.id == group && !listed.hidden))
  );
}

#[test]
fn leaving_or_deleting_takes_the_conversation_out_of_the_world() {
  let (mut demo, events) = demo();
  let trio = GroupId::Dm("demo-trio".into());
  demo.handle(Command::Leave(trio.clone()));
  assert!(!demo.groups.contains_key(&trio));
  assert!(
    drain(&events)
      .iter()
      .any(|event| matches!(event, Event::World(groups) if groups.iter().all(|listed| listed.id != trio)))
  );
  let coffee = GroupId::Space("demo-coffee".into());
  assert!(demo.groups.get(&coffee).unwrap().can_delete);
  demo.handle(Command::DeleteConversation(coffee.clone()));
  assert!(!demo.groups.contains_key(&coffee));
  // A one-to-one chat cannot be left: it stays, and the answer says why.
  let dm = demo
    .groups
    .values()
    .find(|group| group.kind == crate::model::GroupKind::Dm)
    .map(|group| group.id.clone())
    .unwrap();
  drain(&events);
  demo.handle(Command::Leave(dm.clone()));
  assert!(demo.groups.contains_key(&dm));
  assert!(drain(&events).iter().any(|event| matches!(event, Event::Error(_))));
}

/// Width and height from a JPEG's start-of-frame segment, walking the segments after the SOI marker.
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
  let byte = |at: usize| bytes.get(at).copied();
  let word = |at: usize| Some(u16::from_be_bytes([byte(at)?, byte(at + 1)?]));
  if word(0)? != 0xFFD8 {
    return None;
  }
  let mut at = 2;
  while byte(at)? == 0xFF {
    let marker = byte(at + 1)?;
    // SOF0 to SOF15, except DHT (C4), JPG (C8), and DAC (CC). The frame header is length, precision, height, width.
    if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
      return Some((u32::from(word(at + 7)?), u32::from(word(at + 5)?)));
    }
    at += 2 + usize::from(word(at + 2)?);
  }
  None
}

#[test]
fn every_picture_fetches_as_a_jpeg_of_its_declared_size() {
  let world = super::data::world(1_800_000_000_000_000);
  let mut served = Vec::new();
  for attachment in world.histories.values().flatten().flat_map(|message| &message.attachments) {
    let pictures = match attachment.kind {
      AttachmentKind::Image => vec![&attachment.url, attachment.thumb_url.as_ref().unwrap()],
      AttachmentKind::LinkPreview => attachment.thumb_url.iter().collect(),
      _ => continue,
    };
    for url in pictures {
      let bytes = super::fetch_bytes(url).unwrap();
      assert_eq!(
        jpeg_size(&bytes),
        Some((attachment.width.unwrap(), attachment.height.unwrap())),
        "{url} is not a JPEG of the size {} declares",
        attachment.name
      );
      served.push(url.clone());
    }
  }
  for photo in super::image::PHOTOS {
    assert!(served.contains(&photo.url()), "no message shows {}", photo.file);
  }
  assert!(super::fetch_bytes("demo:image/missing.jpg").is_err());
}

/// The threads and `more` a `LoadMail` answers.
fn load_mail(
  demo: &mut Demo,
  events: &flume::Receiver<Event>,
  mailbox: &Mailbox,
  more: bool,
) -> (Vec<MailThread>, bool) {
  demo.handle(Command::LoadMail { mailbox: mailbox.clone(), more });
  let answer = drain(events).into_iter().find_map(|event| match event {
    Event::MailThreads { mailbox: answered, threads, more } if answered == *mailbox => Some(((*threads).clone(), more)),
    _ => None,
  });
  answer.unwrap()
}

fn unread_of(events: &[Event]) -> Option<u32> {
  events.iter().rev().find_map(|event| match event {
    Event::MailUnread(count) => Some(*count),
    _ => None,
  })
}

#[test]
fn mailboxes_page_by_25_newest_first() {
  let (mut demo, events) = demo();
  let (first, more) = load_mail(&mut demo, &events, &Mailbox::Inbox, false);
  assert_eq!(first.len(), super::mail::PAGE);
  assert!(more);
  assert!(first.windows(2).all(|pair| pair[0].time >= pair[1].time));
  let (all_inbox, more) = load_mail(&mut demo, &events, &Mailbox::Inbox, true);
  assert_eq!(all_inbox.len(), 28);
  assert!(!more);
  assert_eq!(all_inbox.get(..25), Some(first.as_slice()));
  let (all, more) = load_mail(&mut demo, &events, &Mailbox::All, false);
  assert_eq!(all.len(), 25);
  assert!(more);
  let (all, more) = load_mail(&mut demo, &events, &Mailbox::All, true);
  assert_eq!(all.len(), 40);
  assert!(!more);
  let (starred, more) = load_mail(&mut demo, &events, &Mailbox::Starred, false);
  assert!(!more);
  assert!(!starred.is_empty() && starred.iter().all(|thread| thread.starred));
  let (sent, _) = load_mail(&mut demo, &events, &Mailbox::Sent, false);
  let me = "amorgan@example.com";
  assert!(sent.iter().all(|thread| thread.senders.iter().any(|sender| sender.email == me)));
  assert!(sent.iter().any(|thread| thread.messages > 1));
  // A reload starts from the newest page again.
  let (again, more) = load_mail(&mut demo, &events, &Mailbox::Inbox, false);
  assert_eq!(again.len(), super::mail::PAGE);
  assert!(more);
}

#[test]
fn the_first_inbox_load_says_how_many_threads_are_unread() {
  let (mut demo, events) = demo();
  load_mail(&mut demo, &events, &Mailbox::Starred, false);
  assert!(demo.timeline.is_empty(), "only the inbox starts the watch");
  demo.handle(Command::LoadMail { mailbox: Mailbox::Inbox, more: false });
  assert_eq!(unread_of(&drain(&events)), Some(5));
  demo.handle(Command::LoadMail { mailbox: Mailbox::Inbox, more: false });
  assert_eq!(unread_of(&drain(&events)), None);
}

#[test]
fn mail_search_matches_subject_sender_and_text_case_insensitively() {
  let (mut demo, events) = demo();
  let search = |demo: &mut Demo, query: &str| {
    load_mail(demo, &events, &Mailbox::Search(query.into()), false)
      .0
      .into_iter()
      .map(|thread| thread.subject)
      .collect::<Vec<_>>()
  };
  assert_eq!(search(&mut demo, "FIELDNOTES weekly").len(), 3);
  let by_sender = search(&mut demo, "riley brooks");
  assert!(by_sender.contains(&"Lunch Thursday?".to_string()));
  assert!(by_sender.contains(&"Offsite agenda".to_string()));
  let by_address = search(&mut demo, "nfischer@example.com");
  assert!(by_address.contains(&"Q4 budget draft".to_string()));
  assert_eq!(search(&mut demo, "bridges"), vec!["Book club pick for December".to_string()]);
  assert_eq!(search(&mut demo, "no such words anywhere"), Vec::<String>::new());
  let (_, more) = load_mail(&mut demo, &events, &Mailbox::Search("example".into()), false);
  assert!(more, "a broad search pages too");
}

#[test]
fn a_new_email_arrives_after_the_first_inbox_load() {
  let (mut demo, events) = demo();
  load_mail(&mut demo, &events, &Mailbox::Inbox, false);
  assert_eq!(demo.timeline.len(), 1);
  run_all(&mut demo);
  let after = drain(&events);
  let Some(Event::MailIncoming(incoming)) = after.iter().find(|event| matches!(event, Event::MailIncoming(_))) else {
    panic!("no incoming email");
  };
  assert!(incoming.unread);
  let inbox = after.iter().find_map(|event| match event {
    Event::MailThreads { mailbox: Mailbox::Inbox, threads, more } => Some((threads.clone(), *more)),
    _ => None,
  });
  let (threads, more) = inbox.unwrap();
  assert_eq!(threads.first().map(|thread| &thread.id), Some(&incoming.id));
  assert_eq!(threads.len(), super::mail::PAGE + 1);
  assert!(more);
  assert_eq!(unread_of(&after), Some(6));
  // It arrives once, even after the watch starts again.
  demo.handle(Command::StopMail);
  load_mail(&mut demo, &events, &Mailbox::Inbox, false);
  run_all(&mut demo);
  assert!(!drain(&events).iter().any(|event| matches!(event, Event::MailIncoming(_))));
}

#[test]
fn stopping_mail_cancels_the_new_email_and_starts_fresh() {
  let (mut demo, events) = demo();
  load_mail(&mut demo, &events, &Mailbox::Inbox, false);
  load_mail(&mut demo, &events, &Mailbox::Inbox, true);
  demo.handle(Command::StopMail);
  assert!(demo.timeline.is_empty());
  assert!(drain(&events).is_empty(), "nothing answers StopMail");
  demo.handle(Command::LoadMail { mailbox: Mailbox::Inbox, more: true });
  let fresh = drain(&events);
  assert_eq!(unread_of(&fresh), Some(5));
  let held = fresh.iter().find_map(|event| match event {
    Event::MailThreads { threads, .. } => Some(threads.len()),
    _ => None,
  });
  assert_eq!(held, Some(super::mail::PAGE), "the held pages were forgotten");
  assert_eq!(demo.timeline.len(), 1);
}

#[test]
fn opening_a_thread_answers_its_emails_and_their_files_fetch() {
  let (mut demo, events) = demo();
  load_mail(&mut demo, &events, &Mailbox::All, false);
  let (all, _) = load_mail(&mut demo, &events, &Mailbox::All, true);
  let mut files = 0;
  let mut inline = 0;
  for thread in &all {
    demo.handle(Command::OpenMail(thread.id.clone()));
    let Some(Event::MailMessages { thread: id, messages }) = drain(&events).pop() else {
      panic!("no emails for {}", thread.subject);
    };
    assert_eq!(id, thread.id);
    assert_eq!(messages.len(), usize::try_from(thread.messages).unwrap());
    assert!(messages.windows(2).all(|pair| pair[0].time <= pair[1].time));
    assert_eq!(messages.iter().any(|message| message.unread), thread.unread);
    assert!(messages.iter().all(|mail| !mail.html.is_empty() && !mail.text.is_empty()));
    for attachment in messages.iter().flat_map(|message| &message.attachments) {
      let bytes = super::fetch_bytes(&attachment.url).unwrap();
      assert_eq!(u64::try_from(bytes.len()).unwrap(), attachment.size, "{}", attachment.name);
      files += 1;
    }
    for message in messages.iter() {
      for source in message.html.split("src=\"").skip(1) {
        let url = source.split('"').next().unwrap();
        assert!(super::fetch_bytes(url).is_ok(), "{url} does not fetch");
        inline += 1;
      }
    }
  }
  assert!(files >= 5);
  assert_eq!(inline, 1);
  let ids: std::collections::HashSet<_> = all.iter().map(|thread| &thread.id).collect();
  assert_eq!(ids.len(), all.len());
  demo.handle(Command::OpenMail("demo-mail-missing".into()));
  assert!(matches!(drain(&events).pop(), Some(Event::MailFailed(_))));
}

fn labels_of(events: &[Event]) -> Option<Vec<crate::model::MailLabel>> {
  events.iter().rev().find_map(|event| match event {
    Event::MailLabels(labels) => Some(labels.to_vec()),
    _ => None,
  })
}

#[test]
fn labels_come_with_the_first_inbox_and_list_their_threads() {
  let (mut demo, events) = demo();
  load_mail(&mut demo, &events, &Mailbox::All, false);
  demo.handle(Command::LoadMail { mailbox: Mailbox::Inbox, more: false });
  let labels = labels_of(&drain(&events)).expect("the first inbox sends the labels");
  assert!(labels.len() >= 12);
  let names: Vec<String> = labels.iter().map(|label| label.name.to_lowercase()).collect();
  assert!(names.windows(2).all(|pair| pair[0] <= pair[1]), "sorted by name");
  assert!(labels.iter().any(|label| label.name == "Clients/Brightwater"));
  assert!(labels.iter().any(|label| label.color.is_some()));
  assert!(labels.iter().any(|label| label.color.is_none()));
  assert!(labels.iter().filter(|label| label.unread > 0).count() >= 3);
  for label in &labels {
    let (threads, more) = load_mail(&mut demo, &events, &Mailbox::Label(label.id.clone()), false);
    assert!((1..=8).contains(&threads.len()), "{} lists {}", label.name, threads.len());
    assert!(!more);
    let unread = threads.iter().filter(|thread| thread.unread).count();
    assert_eq!(u32::try_from(unread).unwrap(), label.unread, "{}", label.name);
  }
  demo.handle(Command::LoadMail { mailbox: Mailbox::Inbox, more: false });
  assert_eq!(labels_of(&drain(&events)), None, "only the first inbox sends them");
  let (threads, more) = load_mail(&mut demo, &events, &Mailbox::Label("Label_missing".into()), false);
  assert!(threads.is_empty() && !more);
  demo.handle(Command::StopMail);
  demo.handle(Command::LoadMail { mailbox: Mailbox::Inbox, more: false });
  assert!(labels_of(&drain(&events)).is_some(), "StopMail starts fresh");
}

#[test]
fn the_new_email_raises_its_labels_unread_count() {
  let (mut demo, events) = demo();
  demo.handle(Command::LoadMail { mailbox: Mailbox::Inbox, more: false });
  let before = labels_of(&drain(&events)).unwrap();
  run_all(&mut demo);
  let after = labels_of(&drain(&events)).expect("the count changed, so the labels come again");
  let changed: Vec<_> = before
    .iter()
    .zip(&after)
    .filter(|(old, new)| old != new)
    .map(|(old, new)| (old.id.clone(), old.unread, new.unread))
    .collect();
  assert_eq!(changed.len(), 1);
  let (id, old, new) = changed.first().unwrap().clone();
  assert_eq!(new, old + 1);
  let (threads, _) = load_mail(&mut demo, &events, &Mailbox::Label(id), false);
  let first = threads.first().unwrap();
  assert!(first.unread && first.subject.contains("launch email"));
}

fn reminders(events: &[Event]) -> Vec<(crate::model::CalendarEvent, u32)> {
  events
    .iter()
    .filter_map(|event| match event {
      Event::EventReminder { event, minutes } => Some((event.clone(), *minutes)),
      _ => None,
    })
    .collect()
}

#[test]
fn watching_events_delivers_one_reminder_for_the_meeting_about_to_start() {
  let (mut demo, events) = demo();
  demo.handle(Command::WatchEvents(true));
  demo.handle(Command::WatchEvents(true));
  assert_eq!(demo.timeline.len(), 1);
  run_all(&mut demo);
  let delivered = reminders(&drain(&events));
  assert_eq!(delivered.len(), 1);
  let (event, minutes) = delivered.first().unwrap();
  assert_eq!(*minutes, 10);
  assert_eq!(event.start, 1_800_000_000_000_000 + 10 * 60 * 1_000_000);
  assert!(event.meet_url.is_some());
  demo.handle(Command::WatchEvents(false));
  demo.handle(Command::WatchEvents(true));
  run_all(&mut demo);
  assert!(reminders(&drain(&events)).is_empty(), "once per run");
}

#[test]
fn turning_event_watching_off_cancels_the_reminder() {
  let (mut demo, events) = demo();
  demo.handle(Command::WatchEvents(true));
  demo.handle(Command::WatchEvents(false));
  assert!(demo.timeline.is_empty());
  run_all(&mut demo);
  assert_eq!(reminders(&drain(&events)), Vec::new());
  demo.handle(Command::WatchEvents(true));
  run_all(&mut demo);
  assert_eq!(reminders(&drain(&events)).len(), 1);
}

fn written(events: &[Event]) -> Option<Result<(), String>> {
  match events.last() {
    Some(Event::MailWritten(result)) => Some(result.clone()),
    _ => None,
  }
}

/// The last list sent for a mailbox.
fn list_of(events: &[Event], wanted: &Mailbox) -> Option<Vec<MailThread>> {
  events.iter().rev().find_map(|event| match event {
    Event::MailThreads { mailbox, threads, .. } if mailbox == wanted => Some(threads.to_vec()),
    _ => None,
  })
}

fn label_unread(events: &[Event], name: &str) -> Option<u32> {
  labels_of(events)?
    .into_iter()
    .find(|label| label.name == name)
    .map(|label| label.unread)
}

fn emails(demo: &mut Demo, events: &flume::Receiver<Event>, thread: &str) -> Vec<crate::model::MailMessage> {
  demo.handle(Command::OpenMail(thread.into()));
  let Some(Event::MailMessages { messages, .. }) = drain(events).pop() else {
    panic!("no emails for {thread}");
  };
  messages.to_vec()
}

#[test]
fn marking_mail_updates_lists_counts_and_labels() {
  let (mut demo, events) = demo();
  load_mail(&mut demo, &events, &Mailbox::Inbox, false);
  let launch = "demo-mail-launch-copy";
  demo.handle(Command::MarkMail { thread: launch.into(), read: true });
  let out = drain(&events);
  assert_eq!(written(&out), Some(Ok(())));
  let inbox = list_of(&out, &Mailbox::Inbox).unwrap();
  assert!(inbox.iter().any(|thread| thread.id == launch && !thread.unread));
  assert_eq!(unread_of(&out), Some(4));
  assert_eq!(label_unread(&out, "Launch"), Some(1));
  assert!(emails(&mut demo, &events, launch).iter().all(|email| !email.unread));

  demo.handle(Command::MarkMail { thread: launch.into(), read: false });
  let out = drain(&events);
  assert_eq!(unread_of(&out), Some(5));
  assert_eq!(label_unread(&out, "Launch"), Some(2));
  let messages = emails(&mut demo, &events, launch);
  assert!(messages.last().unwrap().unread);
  assert!(!messages.first().unwrap().unread);

  // A thread already read changes nothing the app holds.
  demo.handle(Command::MarkMail {
    thread: "demo-mail-interview-loop".into(),
    read: true,
  });
  let out = drain(&events);
  assert_eq!(out.len(), 1);
  assert_eq!(written(&out), Some(Ok(())));

  demo.handle(Command::MarkMail {
    thread: "demo-mail-missing".into(),
    read: true,
  });
  assert_eq!(written(&drain(&events)), Some(Err("This email is no longer there.".into())));
}

#[test]
fn archiving_takes_a_thread_out_of_the_inbox_and_back() {
  let (mut demo, events) = demo();
  load_mail(&mut demo, &events, &Mailbox::Inbox, false);
  load_mail(&mut demo, &events, &Mailbox::Inbox, true);
  let lunch = "demo-mail-lunch-thursday";
  demo.handle(Command::ArchiveMail { thread: lunch.into(), archive: true });
  let out = drain(&events);
  assert_eq!(written(&out), Some(Ok(())));
  let inbox = list_of(&out, &Mailbox::Inbox).unwrap();
  assert_eq!(inbox.len(), 27);
  assert!(!inbox.iter().any(|thread| thread.id == lunch));
  assert_eq!(unread_of(&out), Some(4));
  let (all, _) = load_mail(&mut demo, &events, &Mailbox::All, false);
  assert!(all.iter().any(|thread| thread.id == lunch));
  let (team, _) = load_mail(&mut demo, &events, &Mailbox::Label("Label_1".into()), false);
  assert!(team.iter().any(|thread| thread.id == lunch));

  demo.handle(Command::ArchiveMail { thread: lunch.into(), archive: false });
  let out = drain(&events);
  let inbox = list_of(&out, &Mailbox::Inbox).unwrap();
  assert_eq!(inbox.len(), 28);
  assert!(inbox.iter().any(|thread| thread.id == lunch));
  assert_eq!(unread_of(&out), Some(5));
  assert_eq!(written(&out), Some(Ok(())));

  demo.handle(Command::ArchiveMail {
    thread: "demo-mail-missing".into(),
    archive: true,
  });
  assert!(matches!(written(&drain(&events)), Some(Err(_))));
}

fn reply_to(thread: &str, message: &str, to: &str, text: &str) -> Command {
  Command::ReplyMail(Box::new(MailReply {
    thread: thread.into(),
    message: message.into(),
    to: vec![MailAddress { name: String::new(), email: to.into() }],
    cc: Vec::new(),
    text: text.into(),
    signature: false,
  }))
}

fn answers_scheduled(demo: &Demo) -> usize {
  demo
    .timeline
    .iter()
    .filter(|(_, action)| matches!(action, Action::MailAnswer { .. }))
    .count()
}

#[test]
fn a_reply_goes_to_the_top_and_its_person_answers_back_once() {
  let (mut demo, events) = demo();
  load_mail(&mut demo, &events, &Mailbox::Inbox, false);
  load_mail(&mut demo, &events, &Mailbox::Sent, false);
  demo.timeline.retain(|(_, action)| !matches!(action, Action::MailArrives));
  let thread = "demo-mail-interview-loop";
  demo.handle(reply_to(
    thread,
    "demo-mail-interview-loop-0",
    "praman@example.com",
    "Yes, I can take it.\n\nAvery",
  ));
  let out = drain(&events);
  assert_eq!(written(&out), Some(Ok(())));
  assert_eq!(list_of(&out, &Mailbox::Inbox).unwrap().first().unwrap().id, thread);
  assert_eq!(list_of(&out, &Mailbox::Sent).unwrap().first().unwrap().id, thread);
  let messages = out.iter().find_map(|event| match event {
    Event::MailMessages { thread: id, messages } if id == thread => Some(messages.to_vec()),
    _ => None,
  });
  let messages = messages.unwrap();
  assert_eq!(messages.len(), 2);
  let reply = messages.last().unwrap();
  assert_eq!(reply.from.email, "amorgan@example.com");
  assert_eq!(reply.to.first().unwrap().email, "praman@example.com");
  assert!(
    reply
      .html
      .starts_with("<div dir=\"ltr\"><div>Yes, I can take it.</div><div><br></div><div>Avery</div></div>")
      && reply.html.contains("gmail_quote")
  );
  assert!(!reply.unread);
  assert_eq!(answers_scheduled(&demo), 1);

  run_all(&mut demo);
  let out = drain(&events);
  let incoming = out.iter().find_map(|event| match event {
    Event::MailIncoming(incoming) => Some(incoming.clone()),
    _ => None,
  });
  let incoming = incoming.expect("Priya answers back");
  assert_eq!(incoming.id, thread);
  assert!(incoming.unread);
  assert_eq!(list_of(&out, &Mailbox::Inbox).unwrap().first().unwrap().id, thread);
  assert_eq!(unread_of(&out), Some(6));
  let answer = emails(&mut demo, &events, thread).pop().unwrap();
  assert_eq!(answer.from.email, "praman@example.com");
  assert!(answer.unread && answer.text.contains("Priya"));

  // Once per thread.
  demo.handle(reply_to(thread, &answer.id, "praman@example.com", "Great."));
  assert_eq!(answers_scheduled(&demo), 0);
  // An outside sender does not answer.
  demo.handle(reply_to(
    "demo-mail-invoice-november",
    "",
    "billing@acmecloud.example.com",
    "Thanks.",
  ));
  assert_eq!(answers_scheduled(&demo), 0);
  drain(&events);
  demo.handle(reply_to("demo-mail-missing", "", "jpark@example.com", "Hello"));
  assert_eq!(written(&drain(&events)), Some(Err("This email is no longer there.".into())));
}

fn compose(to: &[&str], cc: &[&str], subject: &str) -> Command {
  let addresses = |emails: &[&str]| {
    emails
      .iter()
      .map(|email| MailAddress {
        name: String::new(),
        email: (*email).into(),
      })
      .collect()
  };
  Command::ComposeMail(Box::new(MailCompose {
    to: addresses(to),
    cc: addresses(cc),
    subject: subject.into(),
    text: "Shall we meet at noon?\n\nAvery".into(),
    signature: false,
  }))
}

#[test]
fn a_new_email_goes_to_sent_and_its_first_person_answers_back_once() {
  let (mut demo, events) = demo();
  load_mail(&mut demo, &events, &Mailbox::Inbox, false);
  load_mail(&mut demo, &events, &Mailbox::Sent, false);
  demo.timeline.retain(|(_, action)| !matches!(action, Action::MailArrives));
  demo.handle(compose(&["praman@example.com"], &["jpark@example.com"], "Lunch on Friday"));
  let out = drain(&events);
  assert_eq!(written(&out), Some(Ok(())));
  assert!(
    list_of(&out, &Mailbox::Inbox).is_none(),
    "not to Avery: the inbox stays as it was"
  );
  let sent = list_of(&out, &Mailbox::Sent).unwrap();
  let new = sent.first().unwrap().clone();
  assert_eq!(new.subject, "Lunch on Friday");
  assert!(!new.unread);
  let emailed = emails(&mut demo, &events, &new.id);
  assert_eq!(emailed.len(), 1);
  assert_eq!(emailed[0].from.email, "amorgan@example.com");
  assert_eq!(emailed[0].cc.first().unwrap().email, "jpark@example.com");
  assert!(
    emailed[0]
      .html
      .starts_with("<div dir=\"ltr\"><div>Shall we meet at noon?</div><div><br></div><div>Avery</div></div>")
  );
  assert_eq!(answers_scheduled(&demo), 1);

  run_all(&mut demo);
  let out = drain(&events);
  let incoming = out.iter().find_map(|event| match event {
    Event::MailIncoming(incoming) => Some(incoming.clone()),
    _ => None,
  });
  let incoming = incoming.expect("Priya answers back");
  assert_eq!(incoming.id, new.id);
  assert_eq!(list_of(&out, &Mailbox::Inbox).unwrap().first().unwrap().id, new.id);
  assert_eq!(
    emails(&mut demo, &events, &new.id).pop().unwrap().from.email,
    "praman@example.com"
  );

  // To Avery too: in the inbox, read, so the unread count stays; an outside first recipient never answers.
  demo.handle(compose(&["billing@acmecloud.example.com"], &["amorgan@example.com"], ""));
  let out = drain(&events);
  assert_eq!(written(&out), Some(Ok(())));
  let top = list_of(&out, &Mailbox::Inbox).unwrap().first().unwrap().clone();
  assert!(top.id != new.id && !top.unread);
  assert_eq!(list_of(&out, &Mailbox::Sent).unwrap().first().unwrap().id, top.id);
  assert_eq!(unread_of(&out), None);
  assert!(!out.iter().any(|event| matches!(event, Event::MailIncoming(_))));
  assert_eq!(answers_scheduled(&demo), 0);

  // Nobody, or no address: an error before anything changes.
  demo.handle(compose(&[], &[], "Nobody"));
  let out = drain(&events);
  assert_eq!(written(&out), Some(Err("Add someone to send the email to.".into())));
  assert!(list_of(&out, &Mailbox::Sent).is_none());
  demo.handle(compose(&["priya"], &[], "No address"));
  let out = drain(&events);
  assert!(matches!(written(&out), Some(Err(_))));
  assert!(list_of(&out, &Mailbox::Sent).is_none());
}

/// The same new email or reply, with the signature.
fn signed(command: Command) -> Command {
  match command {
    Command::ComposeMail(mut compose) => {
      compose.signature = true;
      Command::ComposeMail(compose)
    },
    Command::ReplyMail(mut reply) => {
      reply.signature = true;
      Command::ReplyMail(reply)
    },
    other => other,
  }
}

#[test]
fn signatures_come_with_the_first_inbox_and_sign_what_avery_sends() {
  let (mut demo, events) = demo();
  demo.handle(Command::LoadMail { mailbox: Mailbox::Inbox, more: false });
  let signatures = drain(&events).into_iter().find_map(|event| match event {
    Event::MailSignatures { new, reply } => Some((new.unwrap(), reply.unwrap())),
    _ => None,
  });
  let (new, reply) = signatures.expect("told with the first inbox");
  assert!(new.text.contains("Avery Morgan") && new.text.len() > reply.text.len());
  // Only once.
  demo.handle(Command::LoadMail { mailbox: Mailbox::Inbox, more: false });
  assert!(!drain(&events).iter().any(|event| matches!(event, Event::MailSignatures { .. })));
  load_mail(&mut demo, &events, &Mailbox::Sent, false);
  demo.timeline.retain(|(_, action)| !matches!(action, Action::MailArrives));

  // A new email with the signature for new emails, after the `-- ` line.
  demo.handle(signed(compose(&["billing@acmecloud.example.com"], &[], "Signed")));
  let out = drain(&events);
  let id = list_of(&out, &Mailbox::Sent).unwrap().first().unwrap().id.clone();
  let email = emails(&mut demo, &events, &id).pop().unwrap();
  assert!(email.html.ends_with(&crate::mail::signature_block(
    &new.html,
    SignaturePlace::NewEmail { with_text: true }
  )));
  assert!(email.text.ends_with(&format!("-- \n{}", new.text)));
  // Without it, as before.
  demo.handle(compose(&["billing@acmecloud.example.com"], &[], "Plain"));
  let out = drain(&events);
  let id = list_of(&out, &Mailbox::Sent).unwrap().first().unwrap().id.clone();
  assert!(!emails(&mut demo, &events, &id).pop().unwrap().html.contains("gmail_signature"));

  // A reply with the shorter one, after the quoted email.
  demo.handle(signed(reply_to(
    "demo-mail-invoice-november",
    "demo-mail-invoice-november-0",
    "billing@acmecloud.example.com",
    "Thanks.",
  )));
  drain(&events);
  let sent = emails(&mut demo, &events, "demo-mail-invoice-november").pop().unwrap();
  let quote_ends = sent.html.find("</blockquote>").unwrap();
  let signed_at = sent.html.find("gmail_signature_prefix").unwrap();
  assert!(signed_at > quote_ends);
  assert!(
    sent
      .html
      .ends_with(&crate::mail::signature_block(&reply.html, SignaturePlace::AfterQuote))
  );
}

#[test]
fn a_newsletter_names_where_answers_go() {
  let (mut demo, events) = demo();
  let newsletter = emails(&mut demo, &events, "demo-mail-fieldnotes-112");
  let reply_to = &newsletter.first().unwrap().reply_to;
  assert_eq!(
    reply_to.first().map(|address| address.email.as_str()),
    Some("editors@fieldnotes.example.org")
  );
  let ordinary = emails(&mut demo, &events, "demo-mail-interview-loop");
  assert!(ordinary.iter().all(|email| email.reply_to.is_empty()));
}
