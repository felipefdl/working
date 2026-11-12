//! The made-up workspace demo mode starts with: people, apps, conversations, and their messages.
//!
//! Every person here is invented. Times are relative to the moment the demo starts, so the workspace always
//! looks current.

use std::collections::HashMap;
use std::sync::Arc;

use crate::model::{
  Attachment, AttachmentKind, Card, CardAction, CardButton, CardSection, Group, GroupId, GroupKind, Message, Micros,
  Notify, Placement, Quote, Reaction, SlashCommand, Snippet, Span, SpanKind, User,
};

const MINUTE: Micros = 60 * 1_000_000;
const HOUR: Micros = 60 * MINUTE;
const DAY: Micros = 24 * HOUR;

/// The signed-in user's id.
pub const ME: &str = "demo-me";
/// The build app's id.
pub const BUILD_BOT: &str = "demo-build-bot";
/// Card button method that approves a deploy.
pub const APPROVE: &str = "demo.approve";
/// Card button method that cancels a deploy.
pub const CANCEL: &str = "demo.cancel";

/// The workspace the demo serves.
pub struct World {
  pub me: User,
  pub users: Vec<User>,
  pub groups: Vec<Group>,
  /// Messages per conversation, oldest first, a topic's replies after its first message.
  pub histories: HashMap<GroupId, Vec<Arc<Message>>>,
  /// Who is in each conversation, without the signed-in user.
  pub members: HashMap<GroupId, Vec<String>>,
}

/// A message's sidebar line: its text on one line, else its card's title.
pub fn snippet_text(message: &Message) -> String {
  let text = message.text.split_whitespace().collect::<Vec<_>>().join(" ");
  if !text.is_empty() {
    return text;
  }
  message
    .cards
    .first()
    .map_or_else(|| "Sent an attachment".into(), |card| card.title.clone())
}

fn person(id: &str, name: &str, email: &str) -> User {
  User {
    id: id.into(),
    name: name.into(),
    first_name: name.split_whitespace().next().unwrap_or(name).into(),
    email: email.into(),
    avatar_url: None,
    is_bot: false,
    deleted: false,
  }
}

/// The people and apps of the workspace, the signed-in user first.
pub fn users() -> Vec<User> {
  vec![
    person(ME, "Avery Morgan", "amorgan@example.com"),
    person("demo-jordan", "Jordan Park", "jpark@example.com"),
    person("demo-riley", "Riley Brooks", "rbrooks@example.com"),
    person("demo-sam", "Sam Okafor", "sokafor@example.com"),
    person("demo-priya", "Priya Raman", "praman@example.com"),
    person("demo-diego", "Diego Alvarez", "dalvarez@example.com"),
    person("demo-mei", "Mei Tanaka", "mtanaka@example.com"),
    person("demo-noah", "Noah Fischer", "nfischer@example.com"),
    User {
      is_bot: true,
      ..person(BUILD_BOT, "Build Bot", "")
    },
  ]
}

/// An image the demo draws itself: `Core::fetch` answers this URL with a gradient of that size.
pub fn image(hue: u16, width: u32, height: u32, name: &str) -> Attachment {
  let url = format!("demo:image/{hue}/{width}/{height}");
  Attachment {
    kind: AttachmentKind::Image,
    name: name.into(),
    mime: "image/bmp".into(),
    url: url.clone(),
    thumb_url: Some(url),
    width: Some(width),
    height: Some(height),
  }
}

/// A text file whose bytes the demo serves.
fn file(name: &str) -> Attachment {
  Attachment {
    kind: AttachmentKind::File,
    name: name.into(),
    mime: mime_guess::from_path(name).first_or_text_plain().essence_str().to_string(),
    url: format!("demo:file/{name}"),
    thumb_url: None,
    width: None,
    height: None,
  }
}

/// What a demo file holds.
pub fn file_body(name: &str) -> String {
  format!(
    "{name}\n\nThis file is part of the Working demo. Every name, number, and date in it is made up.\n\n\
     - Launch checklist reviewed\n- Pricing page copy final\n- Support macros updated\n"
  )
}

fn link(url: &str, title: &str, hue: u16) -> Attachment {
  Attachment {
    kind: AttachmentKind::LinkPreview,
    name: title.into(),
    mime: String::new(),
    url: url.into(),
    thumb_url: Some(format!("demo:image/{hue}/800/420")),
    width: Some(800),
    height: Some(420),
  }
}

/// A Meet link as a message carries it.
pub fn meet(code: &str) -> Attachment {
  Attachment {
    kind: AttachmentKind::Meet,
    name: "Video meeting".into(),
    mime: String::new(),
    url: format!("https://meet.google.com/{code}"),
    thumb_url: None,
    width: None,
    height: None,
  }
}

/// Formatting over the first occurrence of each needle.
pub fn marks(text: &str, marks: &[(&str, SpanKind)]) -> Vec<Span> {
  marks
    .iter()
    .filter_map(|(needle, kind)| {
      let start = text.find(needle)?;
      Some(Span {
        range: start..start + needle.len(),
        kind: kind.clone(),
      })
    })
    .collect()
}

/// One list item per line that starts with `- `, the bullet left out of the text.
fn bullets(intro: &str, items: &[&str]) -> (String, Vec<Span>) {
  let mut text = intro.to_string();
  let mut spans = Vec::new();
  for item in items {
    text.push('\n');
    let start = text.len();
    text.push_str(item);
    spans.push(Span {
      range: start..text.len(),
      kind: SpanKind::ListItem,
    });
  }
  (text, spans)
}

fn react(emoji: &str, count: u32, mine: bool) -> Reaction {
  Reaction { emoji: emoji.into(), count, mine }
}

/// The build app's deploy card, with its buttons while the deploy waits.
pub fn deploy_card(id: &str, build: &str, waiting: bool, outcome: &str) -> Card {
  let action = |method: &str| CardAction {
    method: method.into(),
    parameters: vec![("build".into(), build.into())],
    load_indicator: 0,
    persist_values: false,
    interaction: 0,
    all_widgets_are_required: false,
  };
  let buttons = if waiting {
    vec![
      CardButton {
        label: "Approve".into(),
        url: None,
        action: Some(action(APPROVE)),
      },
      CardButton {
        label: "Cancel".into(),
        url: None,
        action: Some(action(CANCEL)),
      },
    ]
  } else {
    Vec::new()
  };
  let mut lines = vec![format!("Build {build} passed 412 tests."), "Target: production".into()];
  if !outcome.is_empty() {
    lines.push(outcome.into());
  }
  Card {
    id: id.into(),
    title: if waiting {
      "Deploy waiting for approval"
    } else {
      "Deploy"
    }
    .into(),
    subtitle: format!("web-app · build {build}"),
    sections: vec![
      CardSection { header: "Checks".into(), lines, buttons },
      CardSection {
        header: String::new(),
        lines: Vec::new(),
        buttons: vec![CardButton {
          label: "Open build log".into(),
          url: Some(format!("https://ci.example.com/builds/{build}")),
          action: None,
        }],
      },
    ],
  }
}

/// The build app's status card.
pub fn status_card(id: &str) -> Card {
  Card {
    id: id.into(),
    title: "Build status".into(),
    subtitle: "web-app · main".into(),
    sections: vec![CardSection {
      header: "Last 3 builds".into(),
      lines: vec![
        "#1842 passed in 6m 12s".into(),
        "#1841 passed in 6m 40s".into(),
        "#1840 failed: 2 flaky tests".into(),
      ],
      buttons: vec![CardButton {
        label: "Open dashboard".into(),
        url: Some("https://ci.example.com/web-app".into()),
        action: None,
      }],
    }],
  }
}

/// The build app's slash commands.
pub fn commands() -> Vec<SlashCommand> {
  let command = |id: i64, name: &str, description: &str| SlashCommand {
    bot_id: BUILD_BOT.into(),
    bot_name: "Build Bot".into(),
    bot_avatar: None,
    id,
    name: name.into(),
    description: description.into(),
  };
  vec![
    command(1, "/status", "Show the latest builds"),
    command(2, "/deploy", "Ask to deploy the latest build"),
    command(3, "/help", "List what Build Bot can do"),
  ]
}

/// A blank message from `sender` in `group`.
pub fn message(group: &GroupId, id: &str, topic: &str, sender: &User, time: Micros, text: &str) -> Message {
  Message {
    id: id.into(),
    topic_id: topic.into(),
    group: group.clone(),
    sender_id: sender.id.clone(),
    sender_name: sender.name.clone(),
    sender_avatar: sender.avatar_url.clone(),
    text: text.into(),
    spans: Vec::new(),
    time,
    update_time: time,
    edited: false,
    system: false,
    reactions: Vec::new(),
    attachments: Vec::new(),
    cards: Vec::new(),
    quote: None,
    starred: false,
    pending: false,
    failed: false,
  }
}

/// Writes one conversation's messages in order.
struct Chat {
  group: GroupId,
  key: String,
  now: Micros,
  next: u32,
  messages: Vec<Arc<Message>>,
}

impl Chat {
  fn new(group: GroupId, now: Micros) -> Self {
    Self {
      key: group.key(),
      group,
      now,
      next: 0,
      messages: Vec::new(),
    }
  }

  fn id(&mut self) -> String {
    self.next += 1;
    format!("{}-{}", self.key, self.next)
  }

  /// A message that starts a topic, `ago` before now. Add it with `add`.
  fn post(&mut self, sender: &User, ago: Micros, text: &str) -> Message {
    let id = self.id();
    message(&self.group, &id, &id, sender, self.now - ago, text)
  }

  /// A reply in `topic`.
  fn reply(&mut self, topic: &str, sender: &User, ago: Micros, text: &str) -> Message {
    let id = self.id();
    message(&self.group, &id, topic, sender, self.now - ago, text)
  }

  fn add(&mut self, message: Message) -> String {
    let id = message.id.clone();
    self.messages.push(Arc::new(message));
    id
  }

  fn answer(&mut self, topic: &str, sender: &User, ago: Micros, text: &str) -> String {
    let message = self.reply(topic, sender, ago, text);
    self.add(message)
  }

  fn say(&mut self, sender: &User, ago: Micros, text: &str) -> String {
    let message = self.post(sender, ago, text);
    self.add(message)
  }
}

struct Builder {
  now: Micros,
  users: HashMap<String, User>,
  world: World,
}

impl Builder {
  fn user(&self, id: &str) -> User {
    self.users.get(id).cloned().unwrap_or_default()
  }

  /// Adds a conversation; `read_back` is how many of its newest messages are unread.
  fn group(&mut self, mut group: Group, members: &[&str], chat: Chat, read_back: usize) {
    let messages = chat.messages;
    if let Some(last) = messages.iter().rev().find(|message| !message.system) {
      group.sort_time = last.time;
      group.snippet = Some(Snippet {
        sender_id: last.sender_id.clone(),
        sender_name: last.sender_name.clone(),
        text: snippet_text(last),
        time: last.time,
      });
    }
    let unread_from = messages.len().saturating_sub(read_back);
    let first_unread = messages.iter().skip(unread_from).map(|message| message.time).min();
    group.unread = first_unread.is_some();
    group.last_read_time = first_unread.map_or(self.now, |time| time - 1);
    self
      .world
      .members
      .insert(group.id.clone(), members.iter().map(|id| (*id).to_string()).collect());
    self.world.histories.insert(group.id.clone(), messages);
    self.world.groups.push(group);
  }

  fn blank(id: GroupId, kind: GroupKind, name: &str) -> Group {
    Group {
      id,
      kind,
      name: name.into(),
      avatar_url: None,
      emoji: None,
      member_ids: Vec::new(),
      member_avatars: Vec::new(),
      sort_time: 0,
      last_read_time: 0,
      unread: false,
      pinned: false,
      muted: false,
      notify: Notify::All,
      snippet: None,
      placement: Placement::ByKind,
    }
  }

  fn dm(&self, user: &str) -> (Group, Chat) {
    let id = GroupId::Dm(format!("{user}-dm"));
    let mut group = Self::blank(id.clone(), GroupKind::Dm, &self.user(user).name);
    group.member_ids = vec![user.into()];
    (group, Chat::new(id, self.now))
  }

  fn space(&self, id: &str, name: &str, emoji: &str) -> (Group, Chat) {
    let id = GroupId::Space(id.into());
    let mut group = Self::blank(id.clone(), GroupKind::Space, name);
    group.emoji = Some(emoji.into());
    (group, Chat::new(id, self.now))
  }
}

/// The workspace as it stands at `now`.
pub fn world(now: Micros) -> World {
  let all = users();
  let me = all.first().cloned().unwrap_or_default();
  let mut b = Builder {
    now,
    users: all.iter().map(|user| (user.id.clone(), user.clone())).collect(),
    world: World {
      me,
      users: all,
      groups: Vec::new(),
      histories: HashMap::new(),
      members: HashMap::new(),
    },
  };
  jordan(&mut b);
  riley(&mut b);
  priya(&mut b);
  group_dm(&mut b);
  build_bot(&mut b);
  launch(&mut b);
  release_train(&mut b);
  coffee(&mut b);
  weekly_sync(&mut b);
  b.world
}

fn jordan(b: &mut Builder) {
  let (me, jordan) = (b.user(ME), b.user("demo-jordan"));
  let (mut group, mut chat) = b.dm("demo-jordan");
  group.pinned = true;
  chat.say(&jordan, 2 * DAY, "Morning! Did the pricing review get moved?");
  chat.say(&me, 2 * DAY - 5 * MINUTE, "Yes, it's Thursday at 2 now.");
  let mut thanks = chat.post(&jordan, 2 * DAY - 7 * MINUTE, "Perfect, thanks");
  thanks.reactions = vec![react("👍", 1, true)];
  chat.add(thanks);
  let mut notes = chat.post(&me, 3 * HOUR, "Here are my notes from the customer call.");
  notes.attachments = vec![file("customer-call-notes.txt")];
  chat.add(notes);
  let mut text = chat.post(
    &jordan,
    40 * MINUTE,
    "Thanks! Two things stood out: they want SSO on day one, and the export needs to be CSV, not only PDF.",
  );
  text.spans = marks(
    &text.text.clone(),
    &[("SSO on day one", SpanKind::Bold), ("CSV", SpanKind::Code)],
  );
  chat.add(text);
  chat.say(
    &jordan,
    38 * MINUTE,
    "Can you check whether the export work is on the board for this sprint?",
  );
  b.group(group, &["demo-jordan"], chat, 2);
}

fn riley(b: &mut Builder) {
  let (me, riley) = (b.user(ME), b.user("demo-riley"));
  let (group, mut chat) = b.dm("demo-riley");
  chat.say(&riley, 3 * DAY, "Lunch Friday?");
  chat.say(&me, 3 * DAY - 10 * MINUTE, "Sure, the ramen place?");
  let mut photo = chat.post(&riley, 3 * DAY - 12 * MINUTE, "Yes! This is what I'm getting");
  photo.attachments = vec![image(28, 640, 480, "ramen.bmp")];
  photo.reactions = vec![react("🍜", 1, true), react("😋", 1, false)];
  chat.add(photo);
  let mut code = chat.post(
    &riley,
    5 * HOUR,
    "The flaky test was a timezone thing. Fixed with:\nTZ=UTC cargo nextest run --workspace",
  );
  code.spans = marks(
    &code.text.clone(),
    &[("TZ=UTC cargo nextest run --workspace", SpanKind::CodeBlock)],
  );
  chat.add(code);
  chat.say(&me, 4 * HOUR, "Nice catch 🙌");
  b.group(group, &["demo-riley"], chat, 0);
}

fn priya(b: &mut Builder) {
  let (me, priya) = (b.user(ME), b.user("demo-priya"));
  let (mut group, mut chat) = b.dm("demo-priya");
  group.notify = Notify::Mentions;
  chat.say(&me, 6 * DAY, "Do you have the slides from the design review?");
  let mut slides = chat.post(&priya, 6 * DAY - HOUR, "Here you go. The flows are on slide 4.");
  slides.attachments = vec![file("design-review.md")];
  chat.add(slides);
  let mut quoted = chat.post(&me, 26 * HOUR, "Going with option B, then.");
  quoted.quote = Some(Quote {
    sender_name: priya.name.clone(),
    text: "Option B tested better with every group.".into(),
  });
  chat.add(quoted);
  b.group(group, &["demo-priya"], chat, 0);
}

fn group_dm(b: &mut Builder) {
  let (me, jordan, riley, sam) = (b.user(ME), b.user("demo-jordan"), b.user("demo-riley"), b.user("demo-sam"));
  let id = GroupId::Dm("demo-trio".into());
  let mut group = Builder::blank(id.clone(), GroupKind::GroupDm, "Jordan, Riley, Sam");
  group.member_ids = vec!["demo-jordan".into(), "demo-riley".into(), "demo-sam".into()];
  let mut chat = Chat::new(id, b.now);
  chat.say(&sam, 9 * HOUR, "Who's bringing the projector to the offsite?");
  chat.say(&riley, 9 * HOUR - 3 * MINUTE, "I can, if someone grabs the adapters.");
  chat.say(&me, 9 * HOUR - 4 * MINUTE, "I'll get the adapters.");
  let mut done = chat.post(&jordan, 8 * HOUR, "Then we're set 🎉");
  done.reactions = vec![react("🎉", 3, true)];
  chat.add(done);
  chat.say(&sam, 12 * MINUTE, "Room changed to 4B, by the way.");
  b.group(group, &["demo-jordan", "demo-riley", "demo-sam"], chat, 1);
}

fn build_bot(b: &mut Builder) {
  let (me, bot) = (b.user(ME), b.user(BUILD_BOT));
  let id = GroupId::Dm("demo-build-bot-dm".into());
  let mut group = Builder::blank(id.clone(), GroupKind::App, "Build Bot");
  group.member_ids = vec![BUILD_BOT.into()];
  let mut chat = Chat::new(id, b.now);
  chat.say(&me, 2 * HOUR, "/status");
  let mut status = chat.post(&bot, 2 * HOUR - 2_000_000, "");
  status.cards = vec![status_card("demo-card-status")];
  chat.add(status);
  let mut deploy = chat.post(&bot, 25 * MINUTE, "");
  deploy.cards = vec![deploy_card("demo-card-deploy", "1842", true, "")];
  chat.add(deploy);
  b.group(group, &[BUILD_BOT], chat, 1);
}

fn launch(b: &mut Builder) {
  let [me, jordan, riley, sam, priya, diego, mei] = [
    ME,
    "demo-jordan",
    "demo-riley",
    "demo-sam",
    "demo-priya",
    "demo-diego",
    "demo-mei",
  ]
  .map(|id| b.user(id));
  let (mut group, mut chat) = b.space("demo-launch", "Product launch", "🚀");
  group.pinned = true;

  let mut created = chat.post(&diego, 9 * DAY, "Diego Alvarez created this space.");
  created.system = true;
  chat.add(created);

  let (text, spans) = bullets(
    "Launch plan for next Tuesday:",
    &[
      "Docs freeze Friday",
      "Pricing page goes live at 9:00",
      "Support macros ready by Monday",
    ],
  );
  let mut plan = chat.post(&diego, 3 * DAY, &text);
  plan.spans = spans;
  plan.starred = true;
  plan.reactions = vec![react("✅", 4, true), react("🚀", 2, false)];
  let plan_id = chat.add(plan);
  chat.answer(&plan_id, &priya, 3 * DAY - 20 * MINUTE, "Docs are on track.");
  let mut sure = chat.reply(
    &plan_id,
    &me,
    3 * DAY - 25 * MINUTE,
    "Support macros are drafted, @Sam Okafor can you review?",
  );
  sure.spans = marks(&sure.text.clone(), &[("@Sam Okafor", SpanKind::Mention("demo-sam".into()))]);
  chat.add(sure);
  chat.answer(&plan_id, &sam, 3 * DAY - 40 * MINUTE, "On it this afternoon.");

  let mut preview = chat.post(&mei, DAY + 3 * HOUR, "The new landing page is up on staging:");
  preview.attachments = vec![link(
    "https://staging.example.com/launch",
    "Example Co: meet the new dashboard",
    210,
  )];
  preview.reactions = vec![react("😍", 3, false)];
  chat.add(preview);

  let mut shots = chat.post(&mei, DAY + 2 * HOUR, "And the hero images, light and dark:");
  shots.attachments = vec![image(200, 960, 600, "hero-light.bmp"), image(260, 960, 600, "hero-dark.bmp")];
  chat.add(shots);

  let mut mention = chat.post(&jordan, 5 * HOUR, "@Avery Morgan can you confirm the press embargo time?");
  mention.spans = marks(&mention.text.clone(), &[("@Avery Morgan", SpanKind::Mention(ME.into()))]);
  let mention_id = chat.add(mention);
  let mut answer = chat.reply(&mention_id, &me, 4 * HOUR, "Tuesday 9:00 ET. I edited the doc to say so.");
  answer.edited = true;
  chat.add(answer);

  let mut all = chat.post(&riley, 70 * MINUTE, "@all launch rehearsal moved to 3:30 today, same link.");
  all.spans = marks(&all.text.clone(), &[("@all", SpanKind::MentionAll)]);
  all.attachments = vec![meet("abc-mnop-xyz")];
  chat.add(all);

  let mut link_text = chat.post(&priya, 15 * MINUTE, "Final copy review is in the launch doc, comments welcome.");
  link_text.spans = marks(
    &link_text.text.clone(),
    &[("launch doc", SpanKind::Link("https://docs.example.com/launch".into()))],
  );
  chat.add(link_text);
  b.group(
    group,
    &["demo-jordan", "demo-riley", "demo-sam", "demo-priya", "demo-diego", "demo-mei"],
    chat,
    2,
  );
}

/// A long, quiet space: enough messages to page through older history.
fn release_train(b: &mut Builder) {
  let people = ["demo-riley", "demo-noah", "demo-sam", "demo-diego"].map(|id| b.user(id));
  let (mut group, mut chat) = b.space("demo-release", "Release train", "🚂");
  group.muted = true;
  group.notify = Notify::Off;
  let lines = [
    "Branch cut done for",
    "Smoke tests green on",
    "Rolled out to 10% for",
    "Rolled out to 100% for",
  ];
  let mut ago = 60 * DAY;
  for release in 1..=40_usize {
    for (step, line) in lines.iter().enumerate() {
      let index = (release + step) % people.len();
      if let Some(sender) = people.get(index) {
        chat.say(sender, ago, &format!("{line} 2.{release}.0"));
      }
      ago -= 3 * HOUR;
    }
    ago -= 9 * HOUR;
  }
  b.group(group, &["demo-riley", "demo-noah", "demo-sam", "demo-diego"], chat, 0);
}

fn coffee(b: &mut Builder) {
  let (mei, noah) = (b.user("demo-mei"), b.user("demo-noah"));
  let (mut group, mut chat) = b.space("demo-coffee", "Coffee chat", "☕");
  group.placement = Placement::Custom {
    id: "demo-social".into(),
    name: "Social".into(),
  };
  let mut beans = chat.post(&mei, 30 * HOUR, "New beans in the kitchen, a washed Ethiopian. Very bright.");
  beans.attachments = vec![image(35, 600, 800, "beans.bmp")];
  chat.add(beans);
  chat.say(&noah, 29 * HOUR, "Tried it, it tastes like blueberries ☕");
  b.group(group, &["demo-mei", "demo-noah"], chat, 0);
}

fn weekly_sync(b: &mut Builder) {
  let (me, diego) = (b.user(ME), b.user("demo-diego"));
  let (mut group, mut chat) = b.space("demo-weekly", "Weekly sync", "📅");
  group.placement = Placement::Meeting;
  let mut agenda = chat.post(&diego, 4 * DAY, "Agenda: metrics, hiring, launch risks.");
  agenda.attachments = vec![meet("wkl-sync-now")];
  chat.add(agenda);
  chat.say(&me, 4 * DAY - HOUR, "Added the support numbers to the doc.");
  b.group(group, &["demo-diego", "demo-jordan"], chat, 0);
}

#[cfg(test)]
mod tests {
  use std::collections::HashSet;

  use super::world;

  #[test]
  fn spans_fall_on_their_text() {
    let world = world(1_000_000_000_000_000);
    for message in world.histories.values().flatten() {
      for span in &message.spans {
        assert!(
          message.text.get(span.range.clone()).is_some_and(|text| !text.is_empty()),
          "{}: {:?}",
          message.id,
          span
        );
      }
    }
  }

  #[test]
  fn replies_follow_their_topic_and_ids_are_unique() {
    let world = world(1_000_000_000_000_000);
    let mut ids = HashSet::new();
    for messages in world.histories.values() {
      let mut seen = HashSet::new();
      for message in messages {
        assert!(ids.insert(message.id.clone()), "duplicate {}", message.id);
        if message.is_thread_reply() {
          assert!(seen.contains(&message.topic_id), "reply {} before its topic", message.id);
        }
        seen.insert(message.id.clone());
      }
    }
  }

  #[test]
  fn every_group_has_messages_and_members() {
    let world = world(1_000_000_000_000_000);
    for group in &world.groups {
      assert!(
        world.histories.get(&group.id).is_some_and(|list| !list.is_empty()),
        "{}",
        group.name
      );
      assert!(
        world.members.get(&group.id).is_some_and(|list| !list.is_empty()),
        "{}",
        group.name
      );
      assert!(group.snippet.is_some(), "{}", group.name);
    }
  }
}
