//! The menu a composer opens to put a chip in its text: `@` lists the people and apps of the conversation, and
//! `/` at the start of the message lists the commands of its apps. Chips are the input's inline tokens; on send
//! they become the message's mentions and slash command.

use std::ops::Range;
use std::rc::Rc;

use gchat_core::{Chip, ChipKind, SlashCommand, User};
use gpui_kit::component::input::{InlineToken, InlineTokenSpan, InputContent};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::images::Images;
use crate::theme::{BORDER, MUTED, POPOVER, ROW_HOVER, ROW_SELECTED, TEXT, rgb};
use crate::ui::avatar;

/// Rows the menu shows at most.
const MAX_ITEMS: usize = 8;
/// Longest `@` query: past it the `@` was not a mention.
const MAX_QUERY: usize = 40;

/// What the text before the cursor asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
  /// `@`: someone to mention.
  Mention,
  /// `/` starting the message: an app command.
  Command,
}

/// One row of the menu.
#[derive(Clone, Debug)]
pub enum Item {
  Person(User),
  /// Everyone in the space.
  All,
  Command(SlashCommand),
}

impl Item {
  /// The chip the row puts in the text. Its id says what it stands for; `chips` reads it back.
  pub fn token(&self) -> InlineToken {
    match self {
      Self::Person(user) => {
        let kind = if user.is_bot { "app" } else { "user" };
        InlineToken::new(format!("{kind}:{}", user.id), format!("@{}", user.name))
      },
      Self::All => InlineToken::new("all", "@all"),
      Self::Command(command) => {
        InlineToken::new(format!("cmd:{}:{}", command.bot_id, command.id), command.name.clone())
      },
    }
  }
}

/// The open menu of one composer.
pub struct Menu {
  /// Which composer: the thread's or the main one.
  pub in_thread: bool,
  /// The trigger character and the query after it, which the picked chip replaces.
  pub range: Range<usize>,
  pub items: Vec<Item>,
  pub selected: usize,
  /// Takes the arrow keys, Return, Tab, and Escape from the composer while the menu is open.
  pub _keys: Subscription,
}

/// The trigger the cursor sits after and the byte range from it to the cursor. An `@` inside a chip, or right
/// after a letter or digit (an email address), starts nothing.
pub fn find(text: &str, cursor: usize, tokens: &[InlineTokenSpan]) -> Option<(Trigger, Range<usize>)> {
  let before = text.get(..cursor)?;
  let in_token = |at: usize| tokens.iter().any(|span| span.range().contains(&at));
  if before.starts_with('/') && !before.contains(char::is_whitespace) && !in_token(0) {
    return Some((Trigger::Command, 0..cursor));
  }
  let at = before.rfind('@')?;
  let query = before.get(at + 1..)?;
  let starts_word = before
    .get(..at)?
    .chars()
    .next_back()
    .is_none_or(|previous| !previous.is_alphanumeric());
  let plausible = !query.starts_with(' ') && !query.contains("  ") && !query.contains('\n');
  (starts_word && plausible && query.chars().count() <= MAX_QUERY && !in_token(at))
    .then_some((Trigger::Mention, at..cursor))
}

/// The rows for `query` (the text after the trigger). `all` offers @all, which only spaces have.
pub fn items(trigger: Trigger, query: &str, members: &[User], all: bool, commands: &[SlashCommand]) -> Vec<Item> {
  let query = query.to_lowercase();
  let mut items: Vec<Item> = match trigger {
    Trigger::Mention => {
      let matches = |user: &User| {
        let name = user.name.to_lowercase();
        name.starts_with(&query)
          || name.split_whitespace().any(|word| word.starts_with(&query))
          || user.email.to_lowercase().starts_with(&query)
      };
      let everyone = all && ["all", "everyone"].iter().any(|word| word.starts_with(&query));
      everyone
        .then_some(Item::All)
        .into_iter()
        .chain(members.iter().filter(|user| matches(user)).cloned().map(Item::Person))
        .collect()
    },
    Trigger::Command => commands
      .iter()
      .filter(|command| {
        command.name.trim_start_matches('/').to_lowercase().starts_with(&query)
          || command.bot_name.to_lowercase().contains(&query)
      })
      .cloned()
      .map(Item::Command)
      .collect(),
  };
  items.truncate(MAX_ITEMS);
  items
}

/// The message `content` holds, trimmed, and the chips in it with ranges into that trimmed text.
pub fn message(content: &InputContent) -> (String, Vec<Chip>) {
  let text = content.text().as_ref();
  let trimmed = text.trim();
  let start = text.len() - text.trim_start().len();
  let chips = content
    .tokens()
    .iter()
    .filter_map(|span| {
      let range = span.range();
      let kind = chip_kind(span.token())?;
      let range = range.start.checked_sub(start)?..range.end.checked_sub(start)?;
      (range.end <= trimmed.len()).then_some(Chip { range, kind })
    })
    .collect();
  (trimmed.to_string(), chips)
}

fn chip_kind(token: &InlineToken) -> Option<ChipKind> {
  let id = token.id().as_ref();
  if id == "all" {
    return Some(ChipKind::MentionAll);
  }
  let (kind, rest) = id.split_once(':')?;
  match kind {
    "user" | "app" => Some(ChipKind::Mention {
      user_id: rest.to_string(),
      bot: kind == "app",
    }),
    "cmd" => {
      let (bot_id, command_id) = rest.rsplit_once(':')?;
      Some(ChipKind::Command {
        bot_id: bot_id.to_string(),
        command_id: command_id.parse().ok()?,
        name: token.text().to_string(),
      })
    },
    _ => None,
  }
}

/// Where the chips of a draft land in its rewrite: each at the first place after the previous one where its text
/// still appears. Chips whose text the rewrite changed are dropped.
pub fn reattach(text: &str, tokens: &[InlineTokenSpan]) -> Vec<(Range<usize>, InlineToken)> {
  let mut from = 0;
  tokens
    .iter()
    .filter_map(|span| {
      let token = span.token();
      let start = from + text.get(from..)?.find(token.text().as_ref())?;
      let range = start..start + token.text().len();
      from = range.end;
      Some((range, token.clone()))
    })
    .collect()
}

impl Menu {
  /// The menu, to sit above its composer. `pick` gets the row index.
  pub fn render(&self, images: &Images, pick: Rc<dyn Fn(usize, &mut Window, &mut App)>, cx: &App) -> Div {
    let mut list = v_flex()
      .w_full()
      .max_w(px(420.))
      .p_1()
      .rounded(px(12.))
      .bg(rgb(POPOVER))
      .border_1()
      .border_color(rgb(BORDER))
      .shadow_lg()
      .occlude();
    for (ix, item) in self.items.iter().enumerate() {
      let (picture, title, detail) = match item {
        Item::Person(user) => (
          avatar(images, cx, user.avatar_url.as_deref(), None, &user.name, 28.),
          user.name.clone(),
          user.email.clone(),
        ),
        Item::All => (
          avatar(images, cx, None, None, "all", 28.),
          "all".to_string(),
          "Notify everyone in this space".to_string(),
        ),
        Item::Command(command) => (
          avatar(images, cx, command.bot_avatar.as_deref(), None, &command.bot_name, 28.),
          command.name.clone(),
          if command.description.is_empty() {
            command.bot_name.clone()
          } else {
            format!("{} · {}", command.bot_name, command.description)
          },
        ),
      };
      let pick = pick.clone();
      list = list.child(
        h_flex()
          .id(("completion", ix))
          .w_full()
          .px_2()
          .py_1p5()
          .gap_3()
          .items_center()
          .rounded(px(8.))
          .cursor_pointer()
          .when(ix == self.selected, |row| row.bg(rgb(ROW_SELECTED)))
          .when(ix != self.selected, |row| row.hover(|row| row.bg(rgb(ROW_HOVER))))
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .on_click(move |_, window, cx| pick(ix, window, cx))
          .child(picture)
          .child(
            v_flex()
              .flex_1()
              .min_w_0()
              .child(div().text_size(px(13.)).text_color(rgb(TEXT)).truncate().child(title))
              .when(!detail.is_empty(), |text| {
                text.child(div().text_size(px(12.)).text_color(rgb(MUTED)).truncate().child(detail))
              }),
          ),
      );
    }
    list
  }
}

#[cfg(test)]
mod tests {
  use gchat_core::{Chip, ChipKind, SlashCommand, User};
  use gpui_kit::component::input::{InlineToken, InputContent};

  use super::{Item, Trigger, find, items, message, reattach};

  fn user(id: &str, name: &str, email: &str) -> User {
    User {
      id: id.into(),
      name: name.into(),
      first_name: String::new(),
      email: email.into(),
      avatar_url: None,
      is_bot: false,
      deleted: false,
    }
  }

  fn command(name: &str) -> SlashCommand {
    SlashCommand {
      bot_id: "bot".into(),
      bot_name: "Agent".into(),
      bot_avatar: None,
      id: 7,
      name: name.into(),
      description: String::new(),
    }
  }

  #[test]
  fn at_starts_a_mention_only_at_a_word_start() {
    assert_eq!(find("hi @fa", 6, &[]), Some((Trigger::Mention, 3..6)));
    assert_eq!(find("@", 1, &[]), Some((Trigger::Mention, 0..1)));
    assert_eq!(find("hi @jordan pa", 13, &[]).map(|found| found.1), Some(3..13));
    assert_eq!(find("mail a@b", 8, &[]), None);
    assert_eq!(find("hi @ fa", 7, &[]), None);
    assert_eq!(find("hi @fa\nx", 8, &[]), None);
  }

  #[test]
  fn an_at_inside_a_chip_starts_nothing() {
    let content = InputContent::new("@Jordan Park ")
      .with_token(0..12, InlineToken::new("user:1", "@Jordan Park"))
      .unwrap_or_else(|_| InputContent::new(""));
    assert_eq!(find("@Jordan Park ", 13, content.tokens()), None);
  }

  #[test]
  fn slash_starts_a_command_only_at_the_start_and_before_a_space() {
    assert_eq!(find("/he", 3, &[]), Some((Trigger::Command, 0..3)));
    assert_eq!(find("/help me", 8, &[]), None);
    assert_eq!(find("a /he", 5, &[]), None);
  }

  #[test]
  fn mention_rows_match_name_words_and_email() {
    let members = [
      user("1", "Jordan Park", "jpark@example.com"),
      user("2", "Riley Brooks", "riley@example.com"),
    ];
    let names = |query: &str, all: bool| -> Vec<String> {
      items(Trigger::Mention, query, &members, all, &[])
        .into_iter()
        .map(|item| match item {
          Item::Person(user) => user.name,
          Item::All => "all".into(),
          Item::Command(command) => command.name,
        })
        .collect()
    };
    assert_eq!(names("pa", false), ["Jordan Park"]);
    assert_eq!(names("riley@", false), ["Riley Brooks"]);
    assert_eq!(names("r", true), ["Riley Brooks"]);
    assert_eq!(names("a", true), ["all"]);
    assert_eq!(names("a", false), Vec::<String>::new());
    assert_eq!(names("", false).len(), 2);
  }

  #[test]
  fn command_rows_match_the_name_without_its_slash() {
    let commands = [command("/help"), command("/link")];
    let found = items(Trigger::Command, "he", &[], false, &commands);
    assert!(matches!(found.as_slice(), [Item::Command(command)] if command.name == "/help"));
  }

  #[test]
  fn chips_come_back_from_tokens_relative_to_the_trimmed_text() {
    let content = InputContent::new("  /help @all @Avery ")
      .with_token(2..7, Item::Command(command("/help")).token())
      .and_then(|content| content.with_token(8..12, Item::All.token()))
      .and_then(|content| content.with_token(13..19, Item::Person(user("9", "Avery", "")).token()));
    let content = content.unwrap_or_else(|_| InputContent::new(""));
    let (text, chips) = message(&content);
    assert_eq!(text, "/help @all @Avery");
    assert_eq!(
      chips,
      [
        Chip {
          range: 0..5,
          kind: ChipKind::Command {
            bot_id: "bot".into(),
            command_id: 7,
            name: "/help".into()
          }
        },
        Chip { range: 6..10, kind: ChipKind::MentionAll },
        Chip {
          range: 11..17,
          kind: ChipKind::Mention { user_id: "9".into(), bot: false }
        },
      ]
    );
  }

  #[test]
  fn reattach_finds_kept_chips_in_order() {
    let content = InputContent::new("@Avery hi @Avery")
      .with_token(0..6, InlineToken::new("user:9", "@Avery"))
      .and_then(|content| content.with_token(10..16, InlineToken::new("user:9", "@Avery")))
      .unwrap_or_else(|_| InputContent::new(""));
    let placed = reattach("Hello @Avery, and @Avery again", content.tokens());
    let ranges: Vec<_> = placed.into_iter().map(|(range, _)| range).collect();
    assert_eq!(ranges, [6..12, 18..24]);
  }
}
