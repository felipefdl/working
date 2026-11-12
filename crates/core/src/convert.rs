//! Wire messages to the UI model: names resolved, UTF-16 offsets turned into byte ranges, URLs made absolute.
#![allow(
  clippy::as_conversions,
  clippy::cast_sign_loss,
  clippy::cast_possible_truncation,
  clippy::string_slice,
  clippy::too_many_lines,
  clippy::expect_used,
  clippy::indexing_slicing,
  clippy::option_if_let_else,
  clippy::unnecessary_wraps,
  reason = "protobuf i32 enums, UTF-16 offsets, and static regexes"
)]

use std::collections::HashMap;

use crate::model::{
  Attachment, AttachmentKind, Card, Group, GroupId, GroupKind, Message, Notify, Placement, Quote, Reaction, Snippet,
  Span, SpanKind, User,
};
use crate::proto;

pub type Users = HashMap<String, User>;

pub fn group_id(id: &proto::GroupId) -> Option<GroupId> {
  if let Some(dm) = id.dm_id.as_ref().and_then(|dm| dm.dm_id.clone()) {
    return Some(GroupId::Dm(dm));
  }
  id.space_id
    .as_ref()
    .and_then(|space| space.space_id.clone())
    .map(GroupId::Space)
}

pub fn proto_group_id(id: &GroupId) -> proto::GroupId {
  match id {
    GroupId::Dm(dm) => proto::GroupId {
      dm_id: Some(proto::DmId { dm_id: Some(dm.clone()) }),
      ..Default::default()
    },
    GroupId::Space(space) => proto::GroupId {
      space_id: Some(proto::SpaceId { space_id: Some(space.clone()) }),
      ..Default::default()
    },
  }
}

fn absolute(url: &str) -> Option<String> {
  match url {
    "" => None,
    url if url.starts_with("//") => Some(format!("https:{url}")),
    url => Some(url.to_string()),
  }
}

pub fn user(user: &proto::User) -> Option<User> {
  let id = user.user_id.as_ref()?.id.clone()?;
  let bot = user.bot_info.as_ref();
  let name = user
    .name
    .clone()
    .filter(|name| !name.is_empty())
    .or_else(|| bot.and_then(|bot| bot.bot_name.clone()));
  let avatar = user
    .avatar_url
    .as_deref()
    .and_then(absolute)
    .or_else(|| bot?.bot_avatar_url.as_deref().and_then(absolute));
  Some(User {
    id,
    name: name.unwrap_or_default(),
    first_name: user.first_name.clone().unwrap_or_default(),
    email: user.email.clone().unwrap_or_default(),
    avatar_url: avatar,
    is_bot: bot.is_some() || user.user_id.as_ref().and_then(|id| id.r#type) == Some(proto::UserType::Bot as i32),
    deleted: user.deleted.unwrap_or(false),
  })
}

/// User ids a conversation needs resolved to show its name and avatar.
pub fn member_ids(item: &proto::WorldItemLite) -> Vec<String> {
  let dm = item.dm_members.iter().flat_map(|members| &members.members);
  let named = item.name_users.iter().flat_map(|users| &users.name_user_ids);
  dm.chain(named).filter_map(|id| id.id.clone()).collect()
}

/// Whether the conversation belongs in the list at all (joined, not hidden, not blocked).
pub fn is_listed(item: &proto::WorldItemLite) -> bool {
  let Some(state) = &item.read_state else {
    return true;
  };
  let joined = state
    .membership_state
    .is_none_or(|value| value == proto::MembershipState::MemberJoined as i32);
  joined && !state.blocked.unwrap_or(false) && state.hide_timestamp.unwrap_or(0) == 0
}

pub fn group(
  item: &proto::WorldItemLite,
  me: &str,
  users: &Users,
  sections: &HashMap<String, String>,
) -> Option<Group> {
  use proto::SharedAttributeCheckerGroupType as Type;
  let id = group_id(item.group_id.as_ref()?)?;
  let room_name = item.room_name.clone().unwrap_or_default();
  let kind = match item.attribute_checker_group_type.and_then(|value| Type::try_from(value).ok()) {
    Some(Type::OneToOneHumanDm | Type::ImmutableMembershipHumanDm) => GroupKind::Dm,
    Some(Type::OneToOneBotDm) => GroupKind::App,
    Some(Type::ImmutableMembershipGroupDm) => GroupKind::GroupDm,
    _ if matches!(id, GroupId::Dm(_)) => GroupKind::Dm,
    _ if room_name.is_empty() => GroupKind::GroupDm,
    _ => GroupKind::Space,
  };
  let member_ids: Vec<String> = member_ids(item).into_iter().filter(|id| id != me).collect();
  let members: Vec<&User> = member_ids.iter().filter_map(|id| users.get(id)).collect();
  // The web app does not list a one-to-one chat whose other account was deleted.
  if kind == GroupKind::Dm && members.first().is_some_and(|user| user.deleted) {
    return None;
  }
  let given_name = item
    .name_users
    .as_ref()
    .and_then(|users| users.group_name.clone())
    .unwrap_or_default();
  let name = if !room_name.is_empty() {
    room_name
  } else if !given_name.is_empty() {
    given_name
  } else if members.len() == 1 && kind == GroupKind::GroupDm {
    // A group chat left with one other person, labeled as the web app does.
    format!("Group: {}", members.first().map(|user| user.name.as_str()).unwrap_or_default())
  } else if members.len() == 1 {
    members.first().map(|user| user.name.clone()).unwrap_or_default()
  } else {
    let first_names: Vec<&str> = members
      .iter()
      .map(|user| {
        if user.first_name.is_empty() {
          user.name.as_str()
        } else {
          user.first_name.as_str()
        }
      })
      .collect();
    first_names.join(", ")
  };
  // Every participant is unknown (deleted accounts): still give the row something to show and sort by.
  let name = if name.trim().is_empty() {
    "Unknown".to_string()
  } else {
    name
  };
  let avatar_url = match kind {
    GroupKind::Dm | GroupKind::App => members.first().and_then(|user| user.avatar_url.clone()),
    // Unset avatars arrive as a generic colored tile; the UI draws something better.
    _ => item
      .avatar_url
      .as_deref()
      .and_then(absolute)
      .filter(|url| !url.contains("/dynamite/images/room_avatars/")),
  };
  let member_avatars = match kind {
    GroupKind::GroupDm => members.iter().filter_map(|user| user.avatar_url.clone()).take(2).collect(),
    _ => Vec::new(),
  };
  let emoji = item
    .avatar_info
    .as_ref()
    .and_then(|info| info.emoji.as_ref())
    .and_then(|emoji| emoji.unicode.clone());
  let state = item.read_state.as_ref();
  let last_read_time = state.and_then(|state| state.last_read_time).unwrap_or(0);
  let sort_time = item.sort_timestamp.unwrap_or(0);
  let snippet = item.message.as_ref().and_then(|message| snippet(message, users));
  let marked_unread = state.and_then(|state| state.mark_as_unread_timestamp_usec).unwrap_or(0) > 0;
  let counted_unread = state.and_then(|state| state.unread_message_count).unwrap_or(0) > 0;
  let newer = match &snippet {
    Some(snippet) => snippet.time > last_read_time && snippet.sender_id != me,
    None => sort_time > last_read_time,
  };
  let settings = state.and_then(|state| state.notification_settings.as_ref());
  let muted = settings.and_then(|settings| settings.state)
    == Some(proto::group_notification_settings::GroupNotificationState::Muted as i32);
  let notify = {
    use proto::group_notification_settings::RoomNotificationState as Room;
    match settings
      .and_then(|settings| settings.room_state)
      .and_then(|value| Room::try_from(value).ok())
    {
      _ if muted => Notify::Off,
      Some(Room::NotifyLessWithNewThreads) => Notify::MainConversations,
      Some(Room::NotifyLess) => Notify::Mentions,
      Some(Room::NotifyNever) => Notify::Off,
      Some(Room::NotifyAlways) | None => Notify::All,
    }
  };
  let placement = state
    .into_iter()
    .flat_map(|state| &state.group_label_id)
    .filter(|label| label.group_label_type == Some(proto::group_label_id::GroupLabelType::CustomSection as i32))
    .filter_map(|label| label.label_secondary_key.as_deref())
    .map(|key| match key {
      "predefined_meeting_chat" => Placement::Meeting,
      id => Placement::Custom {
        id: id.to_string(),
        name: sections.get(id).cloned().unwrap_or_else(|| "Section".into()),
      },
    })
    .next()
    .unwrap_or_default();
  Some(Group {
    id,
    kind,
    name,
    avatar_url,
    emoji: emoji.filter(|emoji| !emoji.is_empty()),
    member_ids,
    member_avatars,
    sort_time,
    last_read_time,
    unread: marked_unread || counted_unread || newer,
    pinned: state.and_then(|state| state.starred).unwrap_or(false),
    muted,
    notify,
    snippet,
    placement,
  })
}

/// Names of the user's own sidebar sections, read from the data embedded in the web app's page. Each section
/// there starts with `[[13,"<id>"` and ends with `["<name>",n,null,[..],n,null,n],[]]`.
pub fn section_names(page: &str) -> HashMap<String, String> {
  let head = regex::Regex::new(r#"\[\[13,"(-?\d+)""#).expect("valid pattern");
  let tail = regex::Regex::new(r#"\["((?:[^"\\]|\\.)*)",\d+,null,\[[\d,]*\],\d+,null,\d+\],\[[^\]]*\]\]"#)
    .expect("valid pattern");
  let mut names = HashMap::new();
  for section in head.captures_iter(page) {
    let start = section.get(0).map_or(0, |found| found.end());
    let Some(name) = tail.captures(&page[start..]).and_then(|found| found.get(1)) else {
      continue;
    };
    let Ok(name) = serde_json::from_str::<String>(&format!("\"{}\"", name.as_str())) else {
      continue;
    };
    if !name.is_empty() {
      names.entry(section[1].to_string()).or_insert(name);
    }
  }
  names
}

fn sender(message: &proto::Message, users: &Users) -> (String, String, Option<String>) {
  let creator = message.creator.as_ref();
  let id = creator
    .and_then(|creator| creator.user_id.as_ref())
    .and_then(|id| id.id.clone())
    .unwrap_or_default();
  let known = users.get(&id);
  let converted = creator.and_then(user);
  let name = converted
    .as_ref()
    .map(|user| user.name.clone())
    .filter(|name| !name.is_empty())
    .or_else(|| known.map(|user| user.name.clone()))
    .unwrap_or_default();
  let avatar = converted
    .and_then(|user| user.avatar_url)
    .or_else(|| known.and_then(|user| user.avatar_url.clone()));
  (id, name, avatar)
}

pub fn snippet(message: &proto::Message, users: &Users) -> Option<Snippet> {
  let (sender_id, sender_name, _) = sender(message, users);
  let text = match message.text_body.as_deref() {
    Some(text) if !text.trim().is_empty() => text.split_whitespace().collect::<Vec<_>>().join(" "),
    _ => "Sent an attachment".to_string(),
  };
  Some(Snippet {
    sender_id,
    sender_name,
    text,
    time: message.create_time.unwrap_or(0),
  })
}

/// Byte offset of each UTF-16 code unit boundary in `text`.
struct Utf16Index<'a> {
  text: &'a str,
}

impl Utf16Index<'_> {
  fn byte(&self, units: usize) -> usize {
    let mut seen = 0;
    for (index, character) in self.text.char_indices() {
      if seen >= units {
        return index;
      }
      seen += character.len_utf16();
    }
    self.text.len()
  }

  fn range(&self, start: i32, length: i32) -> std::ops::Range<usize> {
    let start = start.max(0) as usize;
    self.byte(start)..self.byte(start + length.max(0) as usize)
  }
}

fn attachment_url(url_type: &str, upload: &proto::UploadMetadata, size: &str) -> String {
  format!(
    "https://chat.google.com/api/get_attachment_url?url_type={url_type}&content_type={}&attachment_token={}{size}",
    urlencoding::encode(upload.content_type.as_deref().unwrap_or_default()),
    urlencoding::encode(upload.attachment_token.as_deref().unwrap_or_default()),
  )
}

fn positive(value: Option<i32>) -> Option<u32> {
  value.filter(|value| *value > 0).map(|value| value as u32)
}

fn annotations(text: &str, annotations: &[proto::Annotation]) -> (Vec<Span>, Vec<Attachment>) {
  use proto::AnnotationType as Type;
  use proto::format_metadata::FormatType as Format;
  let index = Utf16Index { text };
  let mut spans = Vec::new();
  let mut attachments = Vec::new();
  for annotation in annotations {
    let range = index.range(annotation.start_index.unwrap_or(0), annotation.length.unwrap_or(0));
    let mut span = |kind: SpanKind| {
      if !range.is_empty() {
        spans.push(Span { range: range.clone(), kind });
      }
    };
    match annotation.r#type.and_then(|value| Type::try_from(value).ok()) {
      Some(Type::FormatData) => {
        let format = annotation.format_metadata.as_ref().and_then(|format| format.format_type);
        match format.and_then(|value| Format::try_from(value).ok()) {
          Some(Format::Bold) => span(SpanKind::Bold),
          Some(Format::Italic) => span(SpanKind::Italic),
          Some(Format::Strike) => span(SpanKind::Strike),
          Some(Format::Underline) => span(SpanKind::Underline),
          Some(Format::Monospace | Format::SourceCode) => span(SpanKind::Code),
          Some(Format::MonospaceBlock) => span(SpanKind::CodeBlock),
          _ => {},
        }
      },
      Some(Type::UserMention) => {
        let Some(mention) = &annotation.user_mention_metadata else {
          continue;
        };
        if mention.r#type == Some(proto::user_mention_metadata::Type::MentionAll as i32) {
          span(SpanKind::MentionAll);
        } else if let Some(id) = mention.id.as_ref().and_then(|id| id.id.clone()) {
          span(SpanKind::Mention(id));
        }
      },
      Some(Type::Url) => {
        let Some(meta) = &annotation.url_metadata else {
          continue;
        };
        let Some(url) = meta.url.as_ref().and_then(|url| url.url.clone()) else {
          continue;
        };
        span(SpanKind::Link(url.clone()));
        let title = meta.title.clone().unwrap_or_default();
        let image = meta.image_url.as_deref().and_then(absolute);
        if !meta.should_not_render.unwrap_or(false) && (!title.is_empty() || image.is_some()) {
          attachments.push(Attachment {
            kind: AttachmentKind::LinkPreview,
            name: if title.is_empty() {
              meta.domain.clone().unwrap_or_default()
            } else {
              title
            },
            mime: meta.mime_type.clone().unwrap_or_default(),
            url,
            thumb_url: image,
            width: positive(meta.int_image_width),
            height: positive(meta.int_image_height),
          });
        }
      },
      Some(Type::UploadMetadata) => {
        let Some(upload) = &annotation.upload_metadata else {
          continue;
        };
        let mime = upload.content_type.clone().unwrap_or_default();
        let kind = match mime.split('/').next() {
          Some("image") => AttachmentKind::Image,
          Some("video") => AttachmentKind::Video,
          _ => AttachmentKind::File,
        };
        let dimension = upload.original_dimension.as_ref();
        attachments.push(Attachment {
          kind,
          name: upload.content_name.clone().unwrap_or_default(),
          url: attachment_url("DOWNLOAD_URL", upload, ""),
          thumb_url: (kind == AttachmentKind::Image).then(|| attachment_url("FIFE_URL", upload, "&sz=w1024")),
          width: positive(dimension.and_then(|dimension| dimension.width)),
          height: positive(dimension.and_then(|dimension| dimension.height)),
          mime,
        });
      },
      Some(Type::DriveFile | Type::DriveDoc | Type::DriveSheet | Type::DriveSlide | Type::DriveForm) => {
        let Some(drive) = &annotation.drive_metadata else {
          continue;
        };
        let Some(id) = &drive.id else { continue };
        attachments.push(Attachment {
          kind: AttachmentKind::Drive,
          name: drive.title.clone().unwrap_or_default(),
          mime: drive.mimetype.clone().unwrap_or_default(),
          url: format!("https://drive.google.com/open?id={}", urlencoding::encode(id)),
          thumb_url: drive.thumbnail_url.as_deref().and_then(absolute),
          width: positive(drive.thumbnail_width),
          height: positive(drive.thumbnail_height),
        });
      },
      Some(Type::VideoCall) => {
        let space = annotation
          .video_call_metadata
          .as_ref()
          .and_then(|call| call.meeting_space.as_ref());
        let Some(url) = space.and_then(|space| space.meeting_url.clone()) else {
          continue;
        };
        attachments.push(Attachment {
          kind: AttachmentKind::Meet,
          name: "Google Meet".into(),
          mime: String::new(),
          url,
          thumb_url: None,
          width: None,
          height: None,
        });
      },
      _ => {},
    }
  }
  spans.sort_by_key(|span| (span.range.start, span.range.end));
  (spans, attachments)
}

/// Removes the HTML subset cards use (`<b>`, `<a href>`, `<br>`, `<font>`), keeping the text.
fn strip_tags(html: &str) -> String {
  let mut text = String::with_capacity(html.len());
  let mut rest = html;
  while let Some(open) = rest.find('<') {
    text.push_str(&rest[..open]);
    let Some(close) = rest[open..].find('>') else {
      rest = &rest[open..];
      break;
    };
    let tag = rest[open + 1..open + close].trim().to_ascii_lowercase();
    if tag.starts_with("br") {
      text.push('\n');
    }
    rest = &rest[open + close + 1..];
  }
  text.push_str(rest);
  text
    .replace("&amp;", "&")
    .replace("&lt;", "<")
    .replace("&gt;", ">")
    .replace("&quot;", "\"")
    .replace("&#39;", "'")
    .replace("&nbsp;", " ")
}

fn formatted(text: Option<&proto::JAddOnsFormattedText>) -> String {
  use proto::j_add_ons_formatted_text::formatted_text_element::styled_text::Style;
  let Some(text) = text else {
    return String::new();
  };
  if text.formatted_text_elements.is_empty() {
    return strip_tags(text.original_text.as_deref().unwrap_or_default()).trim().to_string();
  }
  let mut out = String::new();
  for element in &text.formatted_text_elements {
    if let Some(styled) = &element.styled_text {
      if styled.styles.contains(&(Style::Br as i32)) {
        out.push('\n');
      }
      out.push_str(styled.text.as_deref().unwrap_or_default());
    }
    if let Some(link) = &element.hyperlink {
      out.push_str(link.text.as_deref().or(link.link.as_deref()).unwrap_or_default());
    }
  }
  out.trim().to_string()
}

fn click_url(click: Option<&proto::JAddOnsOnClick>) -> Option<String> {
  let click = click?;
  click
    .open_link
    .as_ref()
    .and_then(|open| open.url.clone())
    .or_else(|| click.link.clone())
    .filter(|url| !url.is_empty())
}

fn card(item: &proto::JAddOnsCardItem) -> Card {
  let mut card = Card::default();
  if let Some(header) = &item.header {
    card.title = formatted(header.title.as_ref());
    card.subtitle = formatted(header.subtitle.as_ref());
  }
  let mut line = |text: String| {
    if !text.is_empty() {
      card.lines.push(text);
    }
  };
  let mut links = Vec::new();
  for section in &item.sections {
    line(formatted(section.header.as_ref()));
    for widget in &section.widgets {
      if let Some(paragraph) = &widget.text_paragraph {
        line(formatted(paragraph.text.as_ref()));
      }
      if let Some(pair) = &widget.text_key_value {
        let key = formatted(pair.key.as_ref());
        let value = formatted(pair.text.as_ref());
        line(if key.is_empty() {
          value
        } else {
          format!("{key}: {value}")
        });
      }
      if let Some(pair) = &widget.image_key_value {
        line(formatted(pair.text.as_ref()));
      }
      if let Some(pair) = &widget.key_value {
        let label = formatted(pair.top_label.as_ref());
        let content = formatted(pair.content.as_ref());
        line(if label.is_empty() {
          content.clone()
        } else {
          format!("{label}: {content}")
        });
        line(formatted(pair.bottom_label.as_ref()));
        if let Some(url) = click_url(pair.on_click.as_ref()) {
          links.push((
            if content.is_empty() {
              "Open".to_string()
            } else {
              content
            },
            url,
          ));
        }
      }
      for button in &widget.buttons {
        let Some(button) = &button.text_button else {
          continue;
        };
        if let Some(url) = click_url(button.on_click.as_deref()) {
          links.push((formatted(button.text.as_ref()), url));
        }
      }
    }
  }
  card.links = links;
  card
}

pub fn message(message: &proto::Message, users: &Users) -> Option<Message> {
  let id = message.id.as_ref()?;
  let topic = id.parent_id.as_ref()?.topic_id.as_ref()?;
  let group = group_id(topic.group_id.as_ref()?)?;
  let (sender_id, sender_name, sender_avatar) = sender(message, users);
  let text = message.text_body.clone().unwrap_or_default();
  let (spans, attachments) = annotations(&text, &message.annotations);
  let reactions = message
    .reactions
    .iter()
    .filter_map(|reaction| {
      Some(Reaction {
        emoji: emoji_text(reaction.emoji.as_ref()?)?,
        count: reaction.count.unwrap_or(0).max(0) as u32,
        mine: reaction.current_user_participated.unwrap_or(false),
      })
    })
    .collect();
  let cards: Vec<Card> = message
    .attachments
    .iter()
    .filter_map(|attachment| attachment.card_add_on_data.as_ref())
    .map(card)
    .filter(|card| !card.title.is_empty() || !card.lines.is_empty() || !card.links.is_empty())
    .collect();
  let quote = message.quoted_message_metadata.as_ref().map(|quoted| Quote {
    sender_name: quoted
      .creator_profile
      .as_ref()
      .and_then(|profile| profile.name.clone())
      .or_else(|| {
        let id = quoted.creator_id.as_ref()?.id.as_ref()?;
        users.get(id).map(|user| user.name.clone())
      })
      .unwrap_or_default(),
    text: quoted.text_body.clone().unwrap_or_default(),
  });
  Some(Message {
    id: id.message_id.clone()?,
    topic_id: topic.topic_id.clone().unwrap_or_default(),
    group,
    sender_id,
    sender_name,
    sender_avatar,
    text,
    spans,
    time: message.create_time.unwrap_or(0),
    update_time: message.last_update_time.unwrap_or(0),
    edited: message.last_edit_time.unwrap_or(0) > 0,
    system: message.message_type == Some(proto::message::MessageType::SystemMessage as i32),
    reactions,
    attachments,
    cards,
    quote,
    starred: message
      .message_labels
      .iter()
      .any(|label| label.label_type == Some(STARRED_LABEL)),
    pending: false,
    failed: false,
  })
}

/// `MessageLabel::label_type` of the user's star.
pub const STARRED_LABEL: i32 = 1;

pub fn emoji_text(emoji: &proto::Emoji) -> Option<String> {
  if let Some(unicode) = emoji.unicode.clone().filter(|unicode| !unicode.is_empty()) {
    return Some(unicode);
  }
  let custom = emoji.custom_emoji.as_ref()?;
  custom.shortcode.clone().filter(|code| !code.is_empty())
}

#[cfg(test)]
mod tests {
  use super::{absolute, annotations, emoji_text, group_id, proto_group_id, strip_tags, user};
  use crate::model::{GroupId, SpanKind};
  use crate::proto;

  #[test]
  fn group_id_round_trips() {
    let dm = GroupId::Dm("abc".into());
    let space = GroupId::Space("xyz".into());
    assert_eq!(group_id(&proto_group_id(&dm)), Some(dm));
    assert_eq!(group_id(&proto_group_id(&space)), Some(space));
    assert_eq!(group_id(&proto::GroupId::default()), None);
  }

  #[test]
  fn absolute_urls() {
    assert_eq!(absolute(""), None);
    assert_eq!(
      absolute("//lh3.googleusercontent.com/a"),
      Some("https://lh3.googleusercontent.com/a".into())
    );
    assert_eq!(absolute("https://chat.google.com/x"), Some("https://chat.google.com/x".into()));
  }

  #[test]
  fn user_from_wire() {
    let parsed = user(&proto::User {
      user_id: Some(proto::UserId {
        id: Some("u1".into()),
        ..Default::default()
      }),
      name: Some("Ada".into()),
      first_name: Some("Ada".into()),
      email: Some("ada@x".into()),
      avatar_url: Some("//lh3.googleusercontent.com/a".into()),
      ..Default::default()
    })
    .expect("id is set");
    assert_eq!(parsed.id, "u1");
    assert_eq!(parsed.name, "Ada");
    assert_eq!(parsed.avatar_url.as_deref(), Some("https://lh3.googleusercontent.com/a"));
    assert!(!parsed.is_bot);
    assert_eq!(user(&proto::User::default()), None);
  }

  #[test]
  fn bot_name_and_flag() {
    let parsed = user(&proto::User {
      user_id: Some(proto::UserId {
        id: Some("bot".into()),
        r#type: Some(proto::UserType::Bot as i32),
        ..Default::default()
      }),
      bot_info: Some(proto::BotInfo {
        bot_name: Some("Helper".into()),
        ..Default::default()
      }),
      ..Default::default()
    })
    .expect("id is set");
    assert!(parsed.is_bot);
    assert_eq!(parsed.name, "Helper");
  }

  #[test]
  fn strip_tags_keeps_text_and_breaks() {
    assert_eq!(strip_tags("a<br>b &amp; c"), "a\nb & c");
    assert_eq!(strip_tags("plain"), "plain");
  }

  #[test]
  fn utf16_bold_span_lands_on_char_boundaries() {
    let (spans, _) = annotations(
      "café",
      &[proto::Annotation {
        r#type: Some(proto::AnnotationType::FormatData as i32),
        start_index: Some(0),
        length: Some(4),
        format_metadata: Some(proto::FormatMetadata {
          format_type: Some(proto::format_metadata::FormatType::Bold as i32),
          ..Default::default()
        }),
        ..Default::default()
      }],
    );
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].kind, SpanKind::Bold);
    assert_eq!(&"café"[spans[0].range.clone()], "café");
  }

  #[test]
  fn emoji_prefers_unicode_then_shortcode() {
    assert_eq!(
      emoji_text(&proto::Emoji {
        unicode: Some("🔥".into()),
        ..Default::default()
      })
      .as_deref(),
      Some("🔥")
    );
    assert_eq!(
      emoji_text(&proto::Emoji {
        custom_emoji: Some(proto::CustomEmoji {
          shortcode: Some(":ship:".into()),
          ..Default::default()
        }),
        ..Default::default()
      })
      .as_deref(),
      Some(":ship:")
    );
    assert_eq!(emoji_text(&proto::Emoji::default()), None);
  }
}
