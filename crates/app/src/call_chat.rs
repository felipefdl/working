//! The chat of a Meet call, as `call.js` reports it from Meet's own chat panel.

use std::ops::Range;

use gpui_kit::SharedString;
use serde::Deserialize;

/// A stretch of a message as the page sends it: plain text, or a link with its address.
#[derive(Deserialize)]
struct Part {
  text: String,
  /// Empty for plain text.
  href: String,
}

/// A message as the page sends it.
#[derive(Deserialize)]
struct Reported {
  sender: SharedString,
  time: SharedString,
  parts: Vec<Part>,
}

/// One message of the call's chat.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(from = "Reported")]
pub struct ChatMessage {
  /// Empty for the user's own messages: Meet names only the others.
  pub sender: SharedString,
  /// The time as Meet writes it ("6:12 PM").
  pub time: SharedString,
  pub text: SharedString,
  /// The links in `text`: where each one is, in bytes, and the address it opens.
  pub links: Vec<(Range<usize>, SharedString)>,
}

impl From<Reported> for ChatMessage {
  fn from(message: Reported) -> Self {
    let mut text = String::new();
    let mut links = Vec::new();
    for part in message.parts {
      let start = text.len();
      text.push_str(&part.text);
      // Only web addresses open: the page decides what a link's address is.
      if part.href.starts_with("https://") || part.href.starts_with("http://") {
        links.push((start..text.len(), part.href.into()));
      }
    }
    text.truncate(text.trim_end().len());
    links.retain(|(range, _)| range.end <= text.len());
    Self {
      sender: message.sender,
      time: message.time,
      text: text.into(),
      links,
    }
  }
}

/// The call's chat, oldest message first.
#[derive(Default, Deserialize)]
pub struct Chat {
  pub messages: Vec<ChatMessage>,
  /// Whether the meeting lets this person write (a host can turn the chat off for everyone else); `None` until
  /// Meet's panel is there to tell.
  writable: Option<bool>,
}

impl Chat {
  /// Parses the JSON of the script's `chat|` report.
  pub fn parse(report: &str) -> Option<Self> {
    serde_json::from_str(report).ok()
  }

  /// The field shows unless the meeting is known to keep this person from writing.
  pub fn writable(&self) -> bool {
    self.writable != Some(false)
  }

  /// How many messages from other people came after the first `seen` ones.
  pub fn unread(&self, seen: usize) -> usize {
    self
      .messages
      .iter()
      .skip(seen)
      .filter(|message| !message.sender.is_empty())
      .count()
  }

  /// The newest message from another person among those after the first `seen` ones.
  pub fn newest_unread(&self, seen: usize) -> Option<usize> {
    (seen..self.messages.len())
      .rev()
      .find(|index| !self.messages[*index].sender.is_empty())
  }
}

#[cfg(test)]
mod tests {
  use super::Chat;

  const REPORT: &str = r#"{"messages":[
    {"sender":"Riley Brooks","time":"6:12 PM","parts":[{"text":"hello","href":""}]},
    {"sender":"","time":"6:12 PM","parts":[{"text":"hi\nthere","href":""}]},
    {"sender":"Riley Brooks","time":"6:13 PM","parts":[
      {"text":"notes: ","href":""},
      {"text":"example.com/é","href":"https://example.com/%C3%A9"},
      {"text":" and ","href":""},
      {"text":"run","href":"javascript:alert(1)"},
      {"text":" \n","href":""}
    ]}
  ],"writable":null}"#;

  #[test]
  fn messages_join_their_parts() {
    let chat = Chat::parse(REPORT).expect("shape");
    assert_eq!(chat.messages[1].text, "hi\nthere");
    let message = &chat.messages[2];
    assert_eq!(message.text, "notes: example.com/é and run");
    // One link: the web address, by its bytes in the text. The script address is plain text.
    assert_eq!(message.links.len(), 1);
    let (range, address) = &message.links[0];
    assert_eq!(&message.text[range.clone()], "example.com/é");
    assert_eq!(address, "https://example.com/%C3%A9");
    assert!(Chat::parse("{}").is_none());
  }

  #[test]
  fn unread_counts_only_the_others() {
    let chat = Chat::parse(REPORT).expect("shape");
    assert_eq!(chat.unread(0), 2);
    assert_eq!(chat.unread(1), 1);
    assert_eq!(chat.unread(3), 0);
    assert_eq!(chat.newest_unread(0), Some(2));
    assert_eq!(chat.newest_unread(3), None);
    assert_eq!(
      Chat::parse(r#"{"messages":[],"writable":null}"#)
        .expect("shape")
        .newest_unread(0),
      None
    );
  }

  #[test]
  fn the_field_hides_only_when_writing_is_off() {
    let writable = |value: &str| {
      Chat::parse(&format!(r#"{{"messages":[],"writable":{value}}}"#))
        .expect("shape")
        .writable()
    };
    assert!(writable("null"));
    assert!(writable("true"));
    assert!(!writable("false"));
  }
}
