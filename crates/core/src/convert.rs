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
use std::ops::Range;

use crate::markup::Parsed;
use crate::model::{
  Attachment, AttachmentKind, Card, CardAction, CardButton, CardSection, Chip, ChipKind, Group, GroupId, GroupKind,
  Message, Notify, Placement, Quote, Reaction, ReadReceipt, SlashCommand, Snippet, Span, SpanKind, User,
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

/// A person's id, name, and avatar: the profile the wire carries, else the one `users` already knows.
fn person(profile: Option<&proto::User>, users: &Users) -> (String, String, Option<String>) {
  let id = profile
    .and_then(|profile| profile.user_id.as_ref())
    .and_then(|id| id.id.clone())
    .unwrap_or_default();
  let known = users.get(&id);
  let converted = profile.and_then(user);
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

fn sender(message: &proto::Message, users: &Users) -> (String, String, Option<String>) {
  person(message.creator.as_ref(), users)
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

/// Earliest reader first; people who read at the same time by id.
pub fn sort_receipts(receipts: &mut [ReadReceipt]) {
  receipts.sort_by(|a, b| a.read_time.cmp(&b.read_time).then_with(|| a.user_id.cmp(&b.user_id)));
}

/// `incoming` applied to the receipts `held`: each person keeps the later of the two read times and the name and
/// avatar the newest answer has (an event may carry only an id), and anyone not held yet is added. Earliest reader
/// first.
pub fn merge_receipts(mut held: Vec<ReadReceipt>, incoming: Vec<ReadReceipt>) -> Vec<ReadReceipt> {
  for receipt in incoming {
    let Some(slot) = held.iter_mut().find(|slot| slot.user_id == receipt.user_id) else {
      held.push(receipt);
      continue;
    };
    slot.read_time = slot.read_time.max(receipt.read_time);
    if !receipt.name.is_empty() {
      slot.name = receipt.name;
    }
    if receipt.avatar_url.is_some() {
      slot.avatar_url = receipt.avatar_url;
    }
  }
  sort_receipts(&mut held);
  held
}

/// Who has read a conversation, from the answer to loading its messages or from an event of the stream: everyone but
/// `me`, each resolved like the sender of a message, earliest reader first. `None` when read receipts are off. A set
/// that lists readers is on even when it leaves the flag out; people without a read time are left out.
pub fn read_receipts(set: &proto::ReadReceiptSet, me: &str, users: &Users) -> Option<Vec<ReadReceipt>> {
  if !set.enabled.unwrap_or(!set.read_receipts.is_empty()) {
    return None;
  }
  let readers = set.read_receipts.iter().filter_map(|receipt| {
    let (user_id, name, avatar_url) = person(receipt.user.as_ref(), users);
    let read_time = receipt.read_time_micros.filter(|time| *time > 0)?;
    (!user_id.is_empty() && user_id != me).then_some(ReadReceipt { user_id, name, avatar_url, read_time })
  });
  Some(merge_receipts(Vec::new(), readers.collect()))
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

/// UTF-16 code units in `text`: what an annotation's offsets count.
fn utf16_units(text: &str) -> i32 {
  i32::try_from(text.encode_utf16().count()).unwrap_or(i32::MAX)
}

/// The annotations that mark `chips` in an outgoing message: mentions and the slash command, at UTF-16 offsets.
pub fn chip_annotations(text: &str, chips: &[Chip]) -> Vec<proto::Annotation> {
  chips
    .iter()
    .filter_map(|chip| {
      let covered = text.get(chip.range.clone()).filter(|covered| !covered.is_empty())?;
      let start = utf16_units(text.get(..chip.range.start)?);
      let user_id = |id: &str, bot: bool| proto::UserId {
        id: Some(id.to_string()),
        r#type: Some(if bot {
          proto::UserType::Bot
        } else {
          proto::UserType::Human
        } as i32),
        ..Default::default()
      };
      let mut annotation = proto::Annotation {
        start_index: Some(start),
        length: Some(utf16_units(covered)),
        ..Default::default()
      };
      match &chip.kind {
        ChipKind::Mention { user_id: id, bot } => {
          annotation.r#type = Some(proto::AnnotationType::UserMention as i32);
          annotation.user_mention_metadata = Some(proto::UserMentionMetadata {
            id: Some(user_id(id, *bot)),
            r#type: Some(proto::user_mention_metadata::Type::Mention as i32),
            ..Default::default()
          });
        },
        ChipKind::MentionAll => {
          annotation.r#type = Some(proto::AnnotationType::UserMention as i32);
          annotation.user_mention_metadata = Some(proto::UserMentionMetadata {
            r#type: Some(proto::user_mention_metadata::Type::MentionAll as i32),
            ..Default::default()
          });
        },
        ChipKind::Command { bot_id, command_id, name } => {
          annotation.r#type = Some(proto::AnnotationType::SlashCommand as i32);
          annotation.slash_command_metadata = Some(proto::SlashCommandMetadata {
            id: Some(user_id(bot_id, true)),
            r#type: Some(proto::slash_command_metadata::Type::Invoke as i32),
            command_name: Some(name.clone()),
            command_id: Some(*command_id),
            triggers_dialog: Some(false),
            ..Default::default()
          });
        },
      }
      Some(annotation)
    })
    .collect()
}

/// The annotations that carry the formatting of an outgoing message, at UTF-16 offsets: the mirror of `annotations`,
/// which reads them. Links and mentions are left out: an address is sent as plain text, and `chip_annotations` marks
/// the mentions. Items on neighboring lines also get the list that wraps them, as on a message that was received.
pub fn format_annotations(text: &str, spans: &[Span]) -> Vec<proto::Annotation> {
  use proto::format_metadata::FormatType as Format;
  let lists = list_ranges(text, spans).into_iter().map(|range| (range, Format::BulletedList));
  let formats = spans.iter().filter_map(|span| {
    let format = match span.kind {
      SpanKind::Bold => Format::Bold,
      SpanKind::Italic => Format::Italic,
      SpanKind::Strike => Format::Strike,
      SpanKind::Underline => Format::Underline,
      SpanKind::Code => Format::Monospace,
      SpanKind::CodeBlock => Format::MonospaceBlock,
      SpanKind::ListItem => Format::BulletedListItem,
      SpanKind::Link(_) | SpanKind::Mention(_) | SpanKind::MentionAll => return None,
    };
    Some((span.range.clone(), format))
  });
  lists
    .chain(formats)
    .filter_map(|(range, format)| {
      let covered = text.get(range.clone()).filter(|covered| !covered.is_empty())?;
      Some(proto::Annotation {
        r#type: Some(proto::AnnotationType::FormatData as i32),
        start_index: Some(utf16_units(text.get(..range.start)?)),
        length: Some(utf16_units(covered)),
        format_metadata: Some(proto::FormatMetadata {
          format_type: Some(format as i32),
          ..Default::default()
        }),
        ..Default::default()
      })
    })
    .collect()
}

/// The ranges of the lists that the list items in `spans` make: items on neighboring lines are one list.
fn list_ranges(text: &str, spans: &[Span]) -> Vec<Range<usize>> {
  let mut items: Vec<&Range<usize>> = spans
    .iter()
    .filter(|span| span.kind == SpanKind::ListItem)
    .map(|span| &span.range)
    .collect();
  items.sort_by_key(|item| item.start);
  let mut lists: Vec<Range<usize>> = Vec::new();
  for item in items {
    match lists.last_mut() {
      Some(list) if text.get(list.end..item.start) == Some("\n") => list.end = item.end,
      _ => lists.push(item.clone()),
    }
  }
  lists
}

/// How a typed message shows while it is pending: its mentions and formatting, and a link over each address.
pub fn pending_spans(typed: &Parsed) -> Vec<Span> {
  let mentions = typed.chips.iter().filter_map(|chip| {
    let kind = match &chip.kind {
      ChipKind::Mention { user_id, .. } => SpanKind::Mention(user_id.clone()),
      ChipKind::MentionAll => SpanKind::MentionAll,
      ChipKind::Command { .. } => return None,
    };
    Some(Span { range: chip.range.clone(), kind })
  });
  with_links(&typed.text, mentions.chain(typed.spans.iter().cloned()).collect())
}

/// The commands of the apps in the conversation, by name, without the ones that open a dialog.
pub fn slash_commands(response: &proto::AutocompleteSlashCommandsResponse) -> Vec<SlashCommand> {
  let mut commands: Vec<SlashCommand> = response
    .bots_in_group
    .iter()
    .filter_map(|bot| {
      let app = bot.bot.as_ref().and_then(user)?;
      Some(bot.slash_commands.iter().filter_map(move |command| {
        let name = command.name.clone().filter(|name| !name.is_empty())?;
        if command.triggers_dialog.unwrap_or(false) {
          return None;
        }
        Some(SlashCommand {
          bot_id: app.id.clone(),
          bot_name: app.name.clone(),
          bot_avatar: app.avatar_url.clone(),
          id: command.command_id?,
          name: if name.starts_with('/') {
            name
          } else {
            format!("/{name}")
          },
          description: command.description.clone().unwrap_or_default(),
        })
      }))
    })
    .flatten()
    .collect();
  commands.sort_by_cached_key(|command| command.name.to_lowercase());
  commands
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
          // The list itself (BULLETED_LIST) only wraps its items.
          Some(Format::BulletedListItem) => span(SpanKind::ListItem),
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
  (with_links(text, spans), attachments)
}

/// `spans` in text order, with a link over each web address in `text` that has none. Google Chat marks a link only
/// when the sender's client did, and a message typed here goes out as plain text. Addresses inside code stay text.
pub fn with_links(text: &str, mut spans: Vec<Span>) -> Vec<Span> {
  let mut finder = linkify::LinkFinder::new();
  finder.kinds(&[linkify::LinkKind::Url]);
  let found: Vec<Span> = finder
    .links(text)
    .map(|link| link.start()..link.end())
    .filter(|range| {
      !spans.iter().any(|span| {
        matches!(span.kind, SpanKind::Link(_) | SpanKind::Code | SpanKind::CodeBlock)
          && span.range.start < range.end
          && range.start < span.range.end
      })
    })
    .map(|range| Span {
      kind: SpanKind::Link(text[range.clone()].to_string()),
      range,
    })
    .collect();
  spans.extend(found);
  spans.sort_by_key(|span| (span.range.start, span.range.end));
  spans
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

/// A button's form action, kept whole so the click sends back what the card carried.
fn card_action(action: &proto::JAddOnsFormAction) -> Option<CardAction> {
  let method = action.action_method_name.clone().filter(|method| !method.is_empty())?;
  Some(CardAction {
    method,
    parameters: action
      .parameters
      .iter()
      .map(|parameter| {
        (
          parameter.key.clone().unwrap_or_default(),
          parameter.value.clone().unwrap_or_default(),
        )
      })
      .collect(),
    load_indicator: action.load_indicator.unwrap_or(0),
    persist_values: action.persist_values.unwrap_or(false),
    interaction: action.interaction.unwrap_or(0),
    all_widgets_are_required: action.all_widgets_are_required.unwrap_or(false),
  })
}

fn card(item: &proto::JAddOnsCardItem, id: &str) -> Card {
  let mut card = Card { id: id.to_string(), ..Card::default() };
  if let Some(header) = &item.header {
    card.title = formatted(header.title.as_ref());
    card.subtitle = formatted(header.subtitle.as_ref());
  }
  for section in &item.sections {
    let mut out = CardSection {
      header: formatted(section.header.as_ref()),
      ..CardSection::default()
    };
    let mut line = |text: String| {
      if !text.is_empty() {
        out.lines.push(text);
      }
    };
    let mut buttons = Vec::new();
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
          buttons.push(CardButton {
            label: if content.is_empty() {
              "Open".to_string()
            } else {
              content
            },
            url: Some(url),
            action: None,
          });
        }
      }
      for button in &widget.buttons {
        let Some(button) = &button.text_button else {
          continue;
        };
        let label = formatted(button.text.as_ref());
        if label.is_empty() || button.disabled.unwrap_or(false) {
          continue;
        }
        let url = click_url(button.on_click.as_deref());
        let action = if url.is_none() {
          button
            .on_click
            .as_ref()
            .and_then(|click| click.action.as_ref())
            .and_then(card_action)
        } else {
          None
        };
        buttons.push(CardButton { label, url, action });
      }
    }
    out.buttons = buttons;
    if !out.header.is_empty() || !out.lines.is_empty() || !out.buttons.is_empty() {
      card.sections.push(out);
    }
  }
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
    .filter_map(|attachment| {
      let item = attachment.card_add_on_data.as_ref()?;
      Some(card(item, attachment.attachment_id.as_deref().unwrap_or_default()))
    })
    .filter(|card| !card.is_empty())
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
  use super::{
    Users, absolute, annotations, emoji_text, format_annotations, group_id, merge_receipts, pending_spans,
    proto_group_id, read_receipts, strip_tags, user, with_links,
  };
  use crate::markup::parse;
  use crate::model::{Chip, ChipKind, GroupId, ReadReceipt, Span, SpanKind, User};
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

  fn wire_receipt(id: &str, name: Option<&str>, avatar: Option<&str>, read_time: Option<i64>) -> proto::ReadReceipt {
    proto::ReadReceipt {
      read_time_micros: read_time,
      user: Some(proto::User {
        user_id: Some(proto::UserId {
          id: Some(id.into()),
          ..Default::default()
        }),
        name: name.map(Into::into),
        avatar_url: avatar.map(Into::into),
        ..Default::default()
      }),
    }
  }

  fn reader(id: &str, read_time: i64) -> ReadReceipt {
    ReadReceipt {
      user_id: id.into(),
      name: String::new(),
      avatar_url: None,
      read_time,
    }
  }

  #[test]
  fn read_receipts_skip_the_signed_in_user_and_resolve_people_like_a_sender() {
    let mut users = Users::new();
    users.insert(
      "riley".into(),
      User {
        id: "riley".into(),
        name: "Riley Brooks".into(),
        avatar_url: Some("https://lh3.googleusercontent.com/riley".into()),
        ..User::default()
      },
    );
    let set = proto::ReadReceiptSet {
      enabled: Some(true),
      read_receipts: vec![
        // The profile on the wire, with a protocol-relative avatar.
        wire_receipt(
          "jordan",
          Some("Jordan Park"),
          Some("//lh3.googleusercontent.com/jordan"),
          Some(300),
        ),
        // The signed-in user.
        wire_receipt("me", Some("Avery Morgan"), None, Some(500)),
        // Only an id: the users already known answer for it.
        wire_receipt("riley", None, None, Some(200)),
        // Nobody knows this one, but where they have read up to still counts.
        wire_receipt("sam", None, None, Some(100)),
        // Never read anything, and no one at all: nothing to place.
        wire_receipt("noah", Some("Noah Fischer"), None, None),
        proto::ReadReceipt { read_time_micros: Some(50), user: None },
      ],
    };
    let people = read_receipts(&set, "me", &users).expect("receipts are on");
    let shown: Vec<(&str, &str, Option<&str>, i64)> = people
      .iter()
      .map(|person| {
        (
          person.user_id.as_str(),
          person.name.as_str(),
          person.avatar_url.as_deref(),
          person.read_time,
        )
      })
      .collect();
    assert_eq!(
      shown,
      [
        ("sam", "", None, 100),
        ("riley", "Riley Brooks", Some("https://lh3.googleusercontent.com/riley"), 200),
        ("jordan", "Jordan Park", Some("https://lh3.googleusercontent.com/jordan"), 300),
      ]
    );
  }

  #[test]
  fn a_set_with_receipts_off_has_none_and_one_that_lists_readers_is_on() {
    let jordan = || vec![wire_receipt("jordan", Some("Jordan Park"), None, Some(10))];
    let set = |enabled, read_receipts| proto::ReadReceiptSet { enabled, read_receipts };
    let people = |set: &proto::ReadReceiptSet| read_receipts(set, "me", &Users::new());
    assert_eq!(people(&set(Some(false), jordan())), None);
    assert_eq!(people(&set(Some(true), Vec::new())), Some(Vec::new()));
    assert_eq!(people(&set(None, Vec::new())), None);
    assert_eq!(people(&set(None, jordan())).as_ref().map(Vec::len), Some(1));
  }

  #[test]
  fn merging_receipts_moves_people_on_but_never_back() {
    let held = vec![reader("jordan", 100), reader("riley", 200)];
    let merged = merge_receipts(held, vec![reader("jordan", 300), reader("riley", 150), reader("sam", 250)]);
    let order: Vec<(&str, i64)> = merged
      .iter()
      .map(|person| (person.user_id.as_str(), person.read_time))
      .collect();
    assert_eq!(order, [("riley", 200), ("sam", 250), ("jordan", 300)]);
    // A person named twice in one answer counts once, at the later time.
    let twice = merge_receipts(
      Vec::new(),
      vec![reader("jordan", 10), reader("jordan", 30), reader("jordan", 20)],
    );
    assert_eq!(twice, [reader("jordan", 30)]);
    // An event may carry only an id: what is known of the person stays, and a name that comes later is kept.
    let named = ReadReceipt {
      name: "Jordan Park".into(),
      avatar_url: Some("https://lh3.googleusercontent.com/jordan".into()),
      ..reader("jordan", 100)
    };
    let bare = merge_receipts(vec![named.clone()], vec![reader("jordan", 200)]);
    assert_eq!(bare, [ReadReceipt { read_time: 200, ..named.clone() }]);
    let late_name = merge_receipts(vec![reader("jordan", 300)], vec![named.clone()]);
    assert_eq!(late_name, [ReadReceipt { read_time: 300, ..named }]);
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
  fn bulleted_list_items_become_list_spans() {
    use proto::format_metadata::FormatType as Format;
    let format = |format_type: proto::format_metadata::FormatType, start: i32, length: i32| proto::Annotation {
      r#type: Some(proto::AnnotationType::FormatData as i32),
      start_index: Some(start),
      length: Some(length),
      format_metadata: Some(proto::FormatMetadata {
        format_type: Some(format_type as i32),
        ..Default::default()
      }),
      ..Default::default()
    };
    let text = "New\none\ntwo";
    let (spans, _) = annotations(
      text,
      &[
        format(Format::BulletedList, 4, 7),
        format(Format::BulletedListItem, 4, 3),
        format(Format::BulletedListItem, 8, 3),
      ],
    );
    let items: Vec<&str> = spans
      .iter()
      .filter(|span| span.kind == SpanKind::ListItem)
      .map(|span| &text[span.range.clone()])
      .collect();
    assert_eq!(items, ["one", "two"]);
    assert_eq!(spans.len(), 2);
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

  #[test]
  fn a_bare_address_becomes_a_link_unless_marked_or_in_code() {
    let text = "see https://example.com/a, `https://example.org` and https://example.net.";
    let code = text.find('`').expect("code");
    let marked = text.find("https://example.net").expect("marked");
    let held = vec![
      Span {
        range: code..code + 21,
        kind: SpanKind::Code,
      },
      Span {
        range: marked..marked + 19,
        kind: SpanKind::Link("https://example.net/".into()),
      },
    ];
    let links: Vec<(&str, String)> = with_links(text, held)
      .into_iter()
      .filter_map(|span| match span.kind {
        SpanKind::Link(url) => Some((&text[span.range], url)),
        _ => None,
      })
      .collect();
    assert_eq!(
      links,
      [
        ("https://example.com/a", "https://example.com/a".to_string()),
        ("https://example.net", "https://example.net/".to_string()),
      ]
    );
  }

  /// What a format annotation says: its format type, then its UTF-16 start and length.
  fn shape(annotation: &proto::Annotation) -> (i32, i32, i32) {
    assert_eq!(annotation.r#type, Some(proto::AnnotationType::FormatData as i32));
    let format = annotation.format_metadata.as_ref().and_then(|format| format.format_type);
    (
      format.unwrap_or_default(),
      annotation.start_index.unwrap_or(-1),
      annotation.length.unwrap_or(-1),
    )
  }

  #[test]
  fn typed_formatting_reads_back_as_it_was_sent() {
    let typed = parse("*bold* _it_ ~gone~ `code` café 😀 *x*\n- one\n- two\n```\nblock\n```", &[]);
    assert_eq!(typed.spans.len(), 8);
    let wire = format_annotations(&typed.text, &typed.spans);
    let (read, attachments) = annotations(&typed.text, &wire);
    assert_eq!(read, typed.spans);
    assert_eq!(attachments, Vec::new());
  }

  #[test]
  fn format_annotations_count_in_utf16_units() {
    use proto::format_metadata::FormatType as Format;
    // The emoji is four bytes and two UTF-16 units.
    let wire = format_annotations("😀 hi", &[Span { range: 5..7, kind: SpanKind::Bold }]);
    assert_eq!(wire.iter().map(shape).collect::<Vec<_>>(), [(Format::Bold as i32, 3, 2)]);
  }

  #[test]
  fn each_kind_of_formatting_has_its_format_and_the_rest_has_none() {
    use proto::format_metadata::FormatType as Format;
    let text = "abc";
    let sent = |kind: SpanKind| format_annotations(text, &[Span { range: 0..3, kind }]);
    for (kind, format) in [
      (SpanKind::Bold, Format::Bold),
      (SpanKind::Italic, Format::Italic),
      (SpanKind::Strike, Format::Strike),
      (SpanKind::Underline, Format::Underline),
      (SpanKind::Code, Format::Monospace),
      (SpanKind::CodeBlock, Format::MonospaceBlock),
    ] {
      assert_eq!(
        sent(kind.clone()).iter().map(shape).collect::<Vec<_>>(),
        [(format as i32, 0, 3)],
        "{kind:?}"
      );
    }
    for kind in [
      SpanKind::Link("https://example.com/".into()),
      SpanKind::Mention("riley".into()),
      SpanKind::MentionAll,
    ] {
      assert_eq!(sent(kind), Vec::new());
    }
    let empty = Span { range: 1..1, kind: SpanKind::Bold };
    let outside = Span { range: 2..9, kind: SpanKind::Bold };
    assert_eq!(format_annotations(text, &[empty, outside]), Vec::new());
  }

  #[test]
  fn items_on_neighboring_lines_share_a_list() {
    use proto::format_metadata::FormatType as Format;
    let item = |range| Span { range, kind: SpanKind::ListItem };
    let wire = format_annotations("a\nb\n\nc", &[item(0..1), item(2..3), item(5..6)]);
    let (list, line) = (Format::BulletedList as i32, Format::BulletedListItem as i32);
    assert_eq!(
      wire.iter().map(shape).collect::<Vec<_>>(),
      [(list, 0, 3), (list, 5, 1), (line, 0, 1), (line, 2, 1), (line, 5, 1)]
    );
  }

  #[test]
  fn a_pending_message_shows_mentions_formatting_and_links() {
    let text = "*hi* @Riley Brooks, see https://example.com/a";
    let start = text.find('@').expect("mention");
    let mention = Chip {
      range: start..start + 13,
      kind: ChipKind::Mention { user_id: "riley".into(), bot: false },
    };
    let typed = parse(text, &[mention]);
    let shown: Vec<(&str, SpanKind)> = pending_spans(&typed)
      .into_iter()
      .map(|span| (&typed.text[span.range], span.kind))
      .collect();
    assert_eq!(
      shown,
      [
        ("hi", SpanKind::Bold),
        ("@Riley Brooks", SpanKind::Mention("riley".into())),
        ("https://example.com/a", SpanKind::Link("https://example.com/a".into())),
      ]
    );
    let command = Chip {
      range: 0..7,
      kind: ChipKind::Command {
        bot_id: "build-bot".into(),
        command_id: 2,
        name: "/deploy".into(),
      },
    };
    let typed = parse("/deploy *now*", &[command]);
    let kinds: Vec<SpanKind> = pending_spans(&typed).into_iter().map(|span| span.kind).collect();
    assert_eq!(kinds, [SpanKind::Bold]);
  }
}
