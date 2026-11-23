//! Read-only smoke run of the core against the signed-in account: prints counts and shapes, never message text.
//! `cargo run -p gchat-core --example probe [seconds-to-listen]`
//!
//! `just probe` sets `WORKING_READONLY=1`. This binary prints diagnostics on purpose.

#![allow(
  clippy::print_stdout,
  clippy::unwrap_used,
  clippy::expect_used,
  clippy::too_many_lines,
  clippy::indexing_slicing,
  clippy::items_after_statements,
  reason = "probe is a diagnostic CLI"
)]

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use gchat_core::{Command, Core, Event};

fn main() {
  // Only the core traces: the HTTP stack's debug lines print `Set-Cookie` values.
  let _ = tracing_subscriber::fmt()
    .with_env_filter("warn,gchat_core=debug,probe=debug")
    .with_writer(std::io::stderr)
    .try_init();
  let listen = std::env::args().nth(1).and_then(|value| value.parse().ok()).unwrap_or(20u64);
  let started = Instant::now();
  let core = Core::start();
  let deadline = started + Duration::from_secs(listen);
  let mut jumped = false;
  if let Some(query) = std::env::args().nth(2) {
    core.send(Command::Search(query));
  }
  // `WORKING_MCP_PORT=<port>` also serves the MCP server for the run at `/mcp`, with every tool group on (sending,
  // mail changes, and calendar changes are offered, and still refused under `WORKING_READONLY=1`).
  let mut mcp: Option<(u16, String)> = None;
  if let Some(port) = std::env::var("WORKING_MCP_PORT").ok().and_then(|port| port.parse().ok()) {
    let token = gchat_core::mcp::token().expect("the MCP token can be stored");
    mcp = Some((port, token.clone()));
    core.send(Command::SetMcp(Some(gchat_core::McpConfig {
      address: Ipv4Addr::LOCALHOST.into(),
      port,
      token,
      chat: true,
      allow_send: true,
      mail: true,
      mail_write: true,
      calendar: true,
      calendar_write: true,
      meet: true,
    })));
  }
  let mut opened = false;
  let mut asked_older = false;
  // Where the read receipts of each conversation land among its messages. Only times are kept, for the counts printed
  // when the run ends.
  let mut message_times: HashMap<gchat_core::GroupId, Vec<i64>> = HashMap::new();
  let mut read_times: HashMap<gchat_core::GroupId, Vec<i64>> = HashMap::new();
  // The conversations as last told, and those a history arrived for, for the counts printed when the run ends.
  let mut conversations: HashMap<gchat_core::GroupId, gchat_core::Group> = HashMap::new();
  let mut histories: std::collections::HashSet<gchat_core::GroupId> = std::collections::HashSet::new();
  // Google Calendar: the calendars, then five weeks of events around today. `WORKING_PROBE_CALENDAR_WRITE=1`, without
  // `WORKING_READONLY`, also creates one private event without guests, renames it, and deletes it.
  const PROBE_TITLE: &str = "Working probe (safe to delete)";
  const PROBE_RENAMED: &str = "Working probe, renamed (safe to delete)";
  let calendar_write = std::env::var("WORKING_PROBE_CALENDAR_WRITE").is_ok_and(|value| value == "1");
  let mut write_step = 0;
  let now = i64::try_from(
    std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .unwrap()
      .as_micros(),
  )
  .unwrap();
  let day = 24 * 3600 * 1_000_000_i64;
  core.send(Command::LoadCalendars);
  // Google Meet: `WORKING_PROBE_MEET_CREATE=1`, without `WORKING_READONLY`, makes one empty meeting of its own (in no
  // conversation, on no calendar). Only the shape of its link is printed, never the link.
  if std::env::var("WORKING_PROBE_MEET_CREATE").is_ok_and(|value| value == "1") {
    core.send(Command::CreateMeeting);
  }
  // Gmail, read only: the inbox (which starts its watch), then the first thread already read, so nothing changes.
  // Only counts and shapes are printed, never names, addresses, subjects, or bodies.
  core.send(Command::LoadMail {
    mailbox: gchat_core::Mailbox::Inbox,
    more: false,
  });
  let mut mail_opened = false;
  let mut label_listed = false;
  // `WORKING_PROBE_MAIL_WRITE=1` (without `WORKING_READONLY`): on the test thread only, found by its exact subject,
  // mark unread, mark read, archive, and move back to the inbox. With `WORKING_MCP_PORT` too, then through the MCP
  // server's mail tools: star and unstar the test thread, save a reply draft in it and send it (only when it goes to
  // the signed-in address alone), and save a new email to the signed-in address as a draft and send it (subject
  // `MCP_SUBJECT`). Prints step names and results only.
  const MCP_SUBJECT: &str = "Working MCP mail check";
  let mut me_email = String::new();
  const WRITE_SUBJECT: &str = "Working write capture";
  let mail_write = std::env::var("WORKING_PROBE_MAIL_WRITE").is_ok_and(|value| value == "1");
  let mut write_searched = false;
  let mut write_thread: Option<String> = None;
  let mut write_done = 0_usize;
  /// A write step: its name and the command it sends for the test thread.
  type WriteStep = (&'static str, fn(String) -> Command);
  let write_steps: [WriteStep; 4] = [
    ("mark unread", |thread| Command::MarkMail { thread, read: false }),
    ("mark read", |thread| Command::MarkMail { thread, read: true }),
    ("archive", |thread| Command::ArchiveMail { thread, archive: true }),
    ("move to inbox", |thread| Command::ArchiveMail { thread, archive: false }),
  ];
  // Event reminders: the next day's events, read only; a reminder prints its minutes, never the event.
  core.send(Command::WatchEvents(true));
  while let Ok(event) = core.events().recv_deadline(deadline) {
    let at = started.elapsed().as_millis();
    if let Event::History { group, messages, .. } = &event {
      message_times.insert(group.clone(), messages.iter().map(|message| message.time).collect());
      histories.insert(group.clone());
    }
    match event {
      Event::Me(me) => {
        println!("[{at}ms] me: name={} avatar={}", !me.name.is_empty(), me.avatar_url.is_some());
        me_email.clone_from(&me.email);
        if !me.id.is_empty() {
          for shortcut in [gchat_core::Shortcut::Mentions, gchat_core::Shortcut::Starred] {
            core.send(Command::LoadShortcut { shortcut, cursor: None });
          }
        }
      },
      Event::Connection(connection) => println!("[{at}ms] connection: {connection:?}"),
      Event::World(groups) => {
        conversations = groups.iter().map(|group| (group.id.clone(), group.clone())).collect();
        let count = |kind| groups.iter().filter(|group| group.kind == kind).count();
        println!(
          "[{at}ms] world: {} groups (dm {}, app {}, group dm {}, space {}), unread {}, pinned {}, unnamed {}, snippets {}, avatars {}",
          groups.len(),
          count(gchat_core::GroupKind::Dm),
          count(gchat_core::GroupKind::App),
          count(gchat_core::GroupKind::GroupDm),
          count(gchat_core::GroupKind::Space),
          groups.iter().filter(|group| group.unread).count(),
          groups.iter().filter(|group| group.pinned).count(),
          groups.iter().filter(|group| group.name.is_empty()).count(),
          groups.iter().filter(|group| group.snippet.is_some()).count(),
          groups
            .iter()
            .filter(|group| group.avatar_url.is_some() || group.emoji.is_some())
            .count(),
        );
        if !opened
          && let Some(group) = groups
            .iter()
            .filter(|group| group.kind == gchat_core::GroupKind::Dm)
            .max_by_key(|group| group.sort_time)
        {
          opened = true;
          println!("[{at}ms] opening the most recent DM");
          core.send(Command::Open(group.id.clone()));
          // Who and what the composer offers after @ and /, in the most recent space.
          if let Some(space) = groups
            .iter()
            .filter(|group| group.kind == gchat_core::GroupKind::Space)
            .max_by_key(|group| group.sort_time)
          {
            core.send(Command::LoadMembers(space.id.clone()));
            core.send(Command::LoadSlashCommands(space.id.clone()));
          }
        }
      },
      Event::SearchResults { query, hits } => {
        let exact = hits.iter().position(|hit| hit.text == query);
        let named = hits.iter().filter(|hit| !hit.sender_name.is_empty()).count();
        println!(
          "[{at}ms] search: {} hits, {named} with sender names, exact match at {exact:?}",
          hits.len()
        );
        if let Some(hit) = exact.map(|index| &hits[index]) {
          core.send(Command::Jump {
            group: hit.group.clone(),
            message_id: hit.message_id.clone(),
            time: hit.time,
          });
        }
      },
      Event::Shortcut { shortcut, hits, appended, cursor } => {
        let newest_first = hits.iter().zip(hits.iter().skip(1)).all(|(a, b)| a.time >= b.time);
        println!(
          "[{at}ms] {shortcut:?}: {} messages, appended {appended}, more {}, newest first {newest_first}",
          hits.len(),
          cursor.is_some()
        );
      },
      Event::History {
        group,
        messages,
        reached_end,
        focus: Some(focus),
        ..
      } => {
        let index = messages.iter().position(|message| message.id == focus);
        let videos = messages
          .iter()
          .flat_map(|message| &message.attachments)
          .filter(|a| a.kind == gchat_core::AttachmentKind::Video)
          .count();
        println!(
          "[{at}ms] jump: {} messages, focus at {index:?}, reached_end {reached_end}, videos {videos}",
          messages.len()
        );
        core.send(Command::LoadNewer(group));
        jumped = true;
      },
      Event::History { messages, reached_end, .. } if jumped => {
        println!("[{at}ms] newer page: {} messages, reached_end {reached_end}", messages.len());
      },
      Event::History { messages, reached_start, prepended, .. } => {
        let spans: usize = messages.iter().map(|message| message.spans.len()).sum();
        let attachments: usize = messages.iter().map(|message| message.attachments.len()).sum();
        let cards: usize = messages.iter().map(|message| message.cards.len()).sum();
        let reactions: usize = messages.iter().map(|message| message.reactions.len()).sum();
        let unnamed = messages.iter().filter(|message| message.sender_name.is_empty()).count();
        let sorted = messages.windows(2).all(|pair| pair[0].time <= pair[1].time);
        println!(
          "[{at}ms] history: {} messages, prepended {prepended}, reached_start {reached_start}, spans {spans}, attachments {attachments}, cards {cards}, reactions {reactions}, unnamed senders {unnamed}, time-sorted {sorted}",
          messages.len()
        );
        if opened
          && !asked_older
          && let Some(first) = messages.first()
        {
          asked_older = true;
          core.send(Command::LoadOlder(first.group.clone()));
          if let Some(url) = messages
            .iter()
            .flat_map(|message| &message.attachments)
            .find_map(|attachment| attachment.thumb_url.clone())
          {
            let core = core.clone();
            std::thread::spawn(move || {
              let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
              let result = runtime.block_on(core.fetch(url));
              println!(
                "thumbnail fetch: {:?}",
                result.map(|bytes| (bytes.len(), bytes.iter().take(4).copied().collect::<Vec<u8>>()))
              );
            });
          }
        }
      },
      Event::ReadReceipts { group, enabled, receipts } => {
        println!(
          "[{at}ms] read receipts: {} (enabled {enabled}), named {}, with avatar {}",
          receipts.len(),
          receipts.iter().filter(|receipt| !receipt.name.is_empty()).count(),
          receipts.iter().filter(|receipt| receipt.avatar_url.is_some()).count()
        );
        read_times.insert(group, receipts.iter().map(|receipt| receipt.read_time).collect());
      },
      Event::GroupUpsert(group) => {
        println!("[{at}ms] group upsert: kind={:?} unread={}", group.kind, group.unread);
        conversations.insert(group.id.clone(), group);
      },
      Event::MessageUpsert { message, replaces, .. } => {
        println!(
          "[{at}ms] message upsert: chars={} pending={} replaces={}",
          message.text.chars().count(),
          message.pending,
          replaces.is_some()
        );
      },
      Event::MessageDeleted { .. } => println!("[{at}ms] message deleted"),
      Event::Typing { typing, .. } => println!("[{at}ms] typing: {typing}"),
      Event::MeetStarted(_) => println!("[{at}ms] meeting started"),
      Event::MeetingCreated(Ok(link)) => {
        let path = link.strip_prefix("https://meet.google.com/").unwrap_or_default();
        println!(
          "[{at}ms] meeting created: link {} chars, meet.google.com {}, code groups {:?}",
          link.chars().count(),
          link.starts_with("https://meet.google.com/"),
          path.split('-').map(str::len).collect::<Vec<_>>()
        );
      },
      Event::MeetingCreated(Err(error)) => println!("[{at}ms] meeting not created: {error}"),
      Event::SignedOut(reason) => println!("[{at}ms] signed out: {reason}"),
      Event::Mcp(status) => println!("[{at}ms] mcp: {status:?}"),
      Event::Dnd(schedules) => println!("[{at}ms] dnd: {schedules:?}"),
      Event::Mine(status) => println!(
        "[{at}ms] mine: dnd={} emoji={} text={}",
        status.dnd,
        !status.emoji.is_empty(),
        !status.text.is_empty()
      ),
      Event::Statuses(statuses) => {
        let count = |status| statuses.iter().filter(|(_, held)| *held == status).count();
        use gchat_core::PersonStatus::{Active, Away, DoNotDisturb};
        println!(
          "[{at}ms] statuses: {} active, {} away, {} do not disturb",
          count(Active),
          count(Away),
          count(DoNotDisturb)
        );
      },
      Event::People(people) => println!("[{at}ms] people: {}", people.len()),
      Event::Members { users, .. } => println!(
        "[{at}ms] members: {} (apps {})",
        users.len(),
        users.iter().filter(|user| user.is_bot).count()
      ),
      Event::SlashCommands { commands, .. } => println!(
        "[{at}ms] slash commands: {} from {} apps, all named /x: {}",
        commands.len(),
        commands
          .iter()
          .map(|command| &command.bot_id)
          .collect::<std::collections::HashSet<_>>()
          .len(),
        commands.iter().all(|command| command.name.starts_with('/'))
      ),
      Event::OpenChat(id) => println!("[{at}ms] open chat: {id:?}"),
      Event::ChatStarting(group) => println!("[{at}ms] chat starting: {} members", group.member_ids.len()),
      Event::ChatFailed(error) => println!("[{at}ms] chat failed: {error}"),
      Event::Calendars(calendars) => {
        let writable = calendars.iter().filter(|calendar| calendar.access.can_write()).count();
        println!(
          "[{at}ms] calendars: {} ({writable} writable, {} primary, {} selected)",
          calendars.len(),
          calendars.iter().filter(|calendar| calendar.primary).count(),
          calendars.iter().filter(|calendar| calendar.selected).count(),
        );
        core.send(Command::LoadEvents {
          start: now - 7 * day,
          end: now + 28 * day,
        });
      },
      Event::CalendarEvents { events, .. } => {
        let sorted = events.windows(2).all(|pair| pair[0].start <= pair[1].start);
        println!(
          "[{at}ms] calendar events: {} (all-day {}, recurring {}, with Meet {}, with guests {}, to answer {}, editable {}), start-sorted {sorted}",
          events.len(),
          events.iter().filter(|event| event.all_day).count(),
          events.iter().filter(|event| event.series.is_some()).count(),
          events.iter().filter(|event| event.meet_url.is_some()).count(),
          events.iter().filter(|event| !event.guests.is_empty()).count(),
          events
            .iter()
            .filter(|event| event.my_rsvp == Some(gchat_core::Rsvp::NeedsAction))
            .count(),
          events.iter().filter(|event| event.can_edit).count(),
        );
        if !calendar_write {
          continue;
        }
        let find = |title: &str| events.iter().find(|event| event.title == title);
        match write_step {
          0 => {
            write_step = 1;
            core.send(Command::SaveEvent(Box::new(gchat_core::EventDraft {
              calendar_id: "primary".into(),
              title: PROBE_TITLE.into(),
              start: now + day,
              end: now + day + 900_000_000,
              visibility: gchat_core::Visibility::Private,
              reminder: gchat_core::Reminder::None,
              ..gchat_core::EventDraft::default()
            })));
          },
          1 => {
            let created = find(PROBE_TITLE).expect("the created event is listed");
            println!(
              "[{at}ms] calendar write: created, private {}, free {}",
              created.visibility == gchat_core::Visibility::Private,
              !created.busy
            );
            write_step = 2;
            core.send(Command::SaveEvent(Box::new(gchat_core::EventDraft {
              calendar_id: created.calendar_id.clone(),
              editing: Some((created.calendar_id.clone(), created.id.clone())),
              title: PROBE_RENAMED.into(),
              start: created.start + 3_600_000_000,
              end: created.end + 3_600_000_000,
              visibility: created.visibility,
              reminder: created.reminder,
              ..gchat_core::EventDraft::default()
            })));
          },
          2 => {
            let renamed = find(PROBE_RENAMED).expect("the renamed event is listed");
            println!(
              "[{at}ms] calendar write: renamed and moved, old title gone {}",
              find(PROBE_TITLE).is_none()
            );
            write_step = 3;
            core.send(Command::DeleteEvent {
              calendar_id: renamed.calendar_id.clone(),
              event_id: renamed.id.clone(),
              scope: gchat_core::EditScope::This,
              notify: false,
            });
          },
          3 => {
            write_step = 4;
            println!("[{at}ms] calendar write: deleted {}", find(PROBE_RENAMED).is_none());
          },
          _ => {},
        }
      },
      Event::CalendarWritten(result) => println!("[{at}ms] calendar written: {result:?}"),
      Event::CalendarFailed(error) => println!("[{at}ms] calendar failed: {error}"),
      Event::MailThreads { mailbox, threads, more } => {
        let kind = match &mailbox {
          gchat_core::Mailbox::Inbox => "inbox",
          gchat_core::Mailbox::Starred => "starred",
          gchat_core::Mailbox::Sent => "sent",
          gchat_core::Mailbox::All => "all",
          gchat_core::Mailbox::Label(_) => "label",
          gchat_core::Mailbox::Search(_) => "search",
        };
        println!(
          "[{at}ms] mail threads: {kind}, {} threads, {} unread, {} starred, {} with files, {} messages, more={more}",
          threads.len(),
          threads.iter().filter(|thread| thread.unread).count(),
          threads.iter().filter(|thread| thread.starred).count(),
          threads.iter().filter(|thread| thread.attachments).count(),
          threads.iter().map(|thread| thread.messages).sum::<u32>(),
        );
        if !mail_opened && let Some(read) = threads.iter().find(|thread| !thread.unread) {
          mail_opened = true;
          core.send(Command::OpenMail(read.id.clone()));
        }
        if mail_write && !write_searched && mailbox == gchat_core::Mailbox::Inbox {
          write_searched = true;
          core.send(Command::LoadMail {
            mailbox: gchat_core::Mailbox::Search(format!("subject:\"{WRITE_SUBJECT}\"")),
            more: false,
          });
        }
        if write_searched
          && write_thread.is_none()
          && write_done == 0
          && matches!(mailbox, gchat_core::Mailbox::Search(_))
          && let Some(thread) = threads.iter().find(|thread| thread.subject == WRITE_SUBJECT)
        {
          println!("[{at}ms] mail write: test thread found");
          write_thread = Some(thread.id.clone());
          core.send(write_steps[0].1(thread.id.clone()));
        }
      },
      Event::MailWritten(result) => {
        let Some(thread) = write_thread.clone() else {
          println!("[{at}ms] mail written: {result:?}");
          continue;
        };
        println!("[{at}ms] mail write: {}: {result:?}", write_steps[write_done].0);
        write_done += 1;
        if result.is_ok() && write_done < write_steps.len() {
          core.send(write_steps[write_done].1(thread));
        } else {
          write_thread = None;
          if result.is_ok()
            && let Some((port, token)) = &mcp
          {
            mcp_mail_checks(*port, token, &thread, &me_email, MCP_SUBJECT);
          }
        }
      },
      Event::MailMessages { messages, .. } => println!(
        "[{at}ms] mail messages: {}, html bytes {}, text bytes {}, {} recipients, {} files, {} unread, {} drafts",
        messages.len(),
        messages.iter().map(|message| message.html.len()).sum::<usize>(),
        messages.iter().map(|message| message.text.len()).sum::<usize>(),
        messages
          .iter()
          .map(|message| message.to.len() + message.cc.len())
          .sum::<usize>(),
        messages.iter().map(|message| message.attachments.len()).sum::<usize>(),
        messages.iter().filter(|message| message.unread).count(),
        messages.iter().filter(|message| message.draft).count(),
      ),
      Event::MailUnread(count) => println!("[{at}ms] mail unread: {count}"),
      Event::MailIncoming(_) => println!("[{at}ms] mail incoming"),
      Event::MailFailed(error) => println!("[{at}ms] mail failed: {error}"),
      Event::MailLabels(labels) => {
        println!(
          "[{at}ms] mail labels: {}, {} nested, {} with a color, {} unread threads",
          labels.len(),
          labels.iter().filter(|label| label.name.contains('/')).count(),
          labels.iter().filter(|label| label.color.is_some()).count(),
          labels.iter().map(|label| label.unread).sum::<u32>(),
        );
        if !label_listed && let Some(label) = labels.iter().find(|label| label.name.contains('/')) {
          label_listed = true;
          core.send(Command::LoadMail {
            mailbox: gchat_core::Mailbox::Label(label.id.clone()),
            more: false,
          });
        }
      },
      Event::MailSignatures { new, reply } => {
        // Which kinds exist, never the signature.
        println!(
          "[{at}ms] mail signatures: new {}, reply {}, same {}",
          new.is_some(),
          reply.is_some(),
          new.is_some() && new == reply
        );
      },
      Event::EventReminder { minutes, .. } => println!("[{at}ms] event reminder: {minutes} minutes before"),
      Event::Error(error) => println!("[{at}ms] error: {error}"),
    }
  }
  let (mut exact, mut between, mut after, mut before, mut unplaced) = (0_usize, 0_usize, 0_usize, 0_usize, 0_usize);
  for (group, times) in &read_times {
    let held = message_times.get(group).map_or(&[][..], Vec::as_slice);
    let (Some(&newest), Some(&oldest)) = (held.iter().max(), held.iter().min()) else {
      unplaced += times.len();
      continue;
    };
    for time in times {
      if held.contains(time) {
        exact += 1;
      } else if *time > newest {
        after += 1;
      } else if *time < oldest {
        before += 1;
      } else {
        between += 1;
      }
    }
  }
  println!(
    "read receipts summary: {} conversations, {} receipts: {exact} at a message time, {between} between messages, {after} after the newest message, {before} before the oldest, {unplaced} where no history is held",
    read_times.len(),
    read_times.values().map(Vec::len).sum::<usize>()
  );
  let shown: Vec<&gchat_core::Group> = conversations.values().filter(|group| !group.hidden && !group.blocked).collect();
  let unread: Vec<&gchat_core::Group> = shown.iter().copied().filter(|group| group.unread).collect();
  println!(
    "conversations summary: {} listed, {} unread ({} muted, {} mentioned, {} muted and mentioned), {} histories arrived ({} of unread ones)",
    shown.len(),
    unread.len(),
    unread.iter().filter(|group| group.muted).count(),
    unread.iter().filter(|group| group.mentioned).count(),
    unread.iter().filter(|group| group.muted && group.mentioned).count(),
    histories.len(),
    unread.iter().filter(|group| histories.contains(&group.id)).count()
  );
}

/// The MCP server's mail write tools on the test thread `thread` and the signed-in address `me`: star, unstar, a reply
/// draft sent, and a new email's draft sent. Prints each step's name and whether it worked, never an answer's text.
fn mcp_mail_checks(port: u16, token: &str, thread: &str, me: &str, subject: &str) {
  if me.is_empty() {
    println!("mail mcp: the signed-in address is not known, skipped");
    return;
  }
  let step = |name: &str, tool: &str, arguments: serde_json::Value| {
    let (ok, text) = call_mail_tool(port, token, tool, &arguments);
    println!("mail mcp: {name}: {}", if ok { "ok" } else { "failed" });
    ok.then_some(text)
  };
  step("star", "mail_star", serde_json::json!({ "thread": thread, "starred": true }));
  step("unstar", "mail_star", serde_json::json!({ "thread": thread, "starred": false }));
  let reply = step(
    "reply draft",
    "mail_draft",
    serde_json::json!({ "thread": thread, "text": "Working MCP mail check: a reply saved as a draft, then sent." }),
  );
  match reply {
    Some(text) if !only_to(&text, me) => println!("mail mcp: the reply draft goes to others: left unsent in Drafts"),
    Some(text) => {
      if let Some(id) = draft_id(&text) {
        step("reply draft sent", "mail_send_draft", serde_json::json!({ "draft": id }));
      }
    },
    None => {},
  }
  let new = step(
    "new email draft",
    "mail_draft",
    serde_json::json!({ "to": me, "subject": subject, "text": "A new email saved as a draft, then sent." }),
  );
  if let Some(id) = new.as_deref().and_then(draft_id) {
    step("new email draft sent", "mail_send_draft", serde_json::json!({ "draft": id }));
  }
}

/// The draft id in `mail_draft`'s answer (its `Draft: ` line).
fn draft_id(text: &str) -> Option<String> {
  text
    .lines()
    .find_map(|line| line.strip_prefix("Draft: "))
    .map(|id| id.trim().to_string())
}

/// Every address in `mail_draft`'s `To:` and `Cc:` lines is `me`, and there is one.
fn only_to(text: &str, me: &str) -> bool {
  let me = me.to_lowercase();
  let lines: Vec<String> = text
    .lines()
    .filter(|line| line.starts_with("To: ") || line.starts_with("Cc: "))
    .map(str::to_lowercase)
    .collect();
  !lines.is_empty()
    && lines
      .iter()
      .all(|line| line.matches('@').count() == line.matches(me.as_str()).count())
}

/// One `tools/call` of a mail tool on the probe's own MCP server (revision 2025-11-25, which needs no handshake), over
/// plain HTTP/1.1 on loopback: whether it worked, and its answer's text.
fn call_mail_tool(port: u16, token: &str, name: &str, arguments: &serde_json::Value) -> (bool, String) {
  use std::io::{Read, Write};
  let body = serde_json::json!({
    "jsonrpc": "2.0",
    "id": 1,
    "method": "tools/call",
    "params": { "name": name, "arguments": arguments }
  })
  .to_string();
  let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
    return (false, String::new());
  };
  let _ = stream.set_read_timeout(Some(Duration::from_secs(90)));
  let request = format!(
    "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: \
     application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
    body.len()
  );
  if stream.write_all(request.as_bytes()).is_err() {
    return (false, String::new());
  }
  let mut answer = String::new();
  let _ = stream.read_to_string(&mut answer);
  let message = answer
    .split_once("\r\n\r\n")
    .and_then(|(_, body)| serde_json::from_str::<serde_json::Value>(body).ok());
  let Some(result) = message.as_ref().map(|message| &message["result"]) else {
    return (false, String::new());
  };
  let text = result["content"][0]["text"].as_str().unwrap_or_default().to_string();
  (result["isError"] == false, text)
}
