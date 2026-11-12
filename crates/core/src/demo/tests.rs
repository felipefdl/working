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
