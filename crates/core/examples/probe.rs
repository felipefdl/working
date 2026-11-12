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

use std::time::{Duration, Instant};

use gchat_core::{Command, Core, Event};

fn main() {
  let _ = tracing_subscriber::fmt()
    .with_env_filter("debug")
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
  // `WORKING_MCP_PORT=<port>` also serves the MCP tools for the run (sending stays refused: read-only).
  if let Some(port) = std::env::var("WORKING_MCP_PORT").ok().and_then(|port| port.parse().ok()) {
    let token = gchat_core::mcp::token().expect("the MCP token can be stored");
    core.send(Command::SetMcp(Some(gchat_core::McpConfig { port, token, allow_send: true })));
  }
  let mut opened = false;
  let mut asked_older = false;
  while let Ok(event) = core.events().recv_deadline(deadline) {
    let at = started.elapsed().as_millis();
    match event {
      Event::Me(me) => println!("[{at}ms] me: name={} avatar={}", !me.name.is_empty(), me.avatar_url.is_some()),
      Event::Connection(connection) => println!("[{at}ms] connection: {connection:?}"),
      Event::World(groups) => {
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
      Event::GroupUpsert(group) => println!("[{at}ms] group upsert: kind={:?} unread={}", group.kind, group.unread),
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
      Event::OpenChat(id) => println!("[{at}ms] open chat: {id:?}"),
      Event::Error(error) => println!("[{at}ms] error: {error}"),
    }
  }
}
