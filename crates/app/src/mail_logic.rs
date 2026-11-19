//! The Mail view's plain code: what its mailboxes are called, how a thread's people and an email's recipients read on
//! one line, file sizes, when the reading pane fits beside the list, moving the selection, the labels popover's tree
//! and filter, and a new email's addresses (parse, valid `local@domain`, suggestion rank).

use std::collections::HashSet;

use gchat_core::fuzzy::Fuzzy;
use gchat_core::{Attachment, AttachmentKind, MailAddress, MailAttachment, MailLabel, Mailbox};

/// Width of the thread list while the reading pane is beside it.
pub const LIST_WIDTH: f32 = 380.0;
/// The narrowest the reading pane gets beside the list. A narrower panel shows one of them at a time.
const READER_MIN_WIDTH: f32 = 420.0;

/// The mailboxes of the switcher, in its order.
pub const fn mailboxes() -> [Mailbox; 4] {
  [Mailbox::Inbox, Mailbox::Starred, Mailbox::Sent, Mailbox::All]
}

/// The switcher's name for a mailbox.
pub fn mailbox_label(mailbox: &Mailbox) -> String {
  match mailbox {
    Mailbox::Inbox => "Inbox".into(),
    Mailbox::Starred => "Starred".into(),
    Mailbox::Sent => "Sent".into(),
    Mailbox::All => "All mail".into(),
    Mailbox::Label(_) => "Label".into(),
    Mailbox::Search(query) => format!("Results for \"{query}\""),
  }
}

/// What an empty mailbox says.
pub fn empty_text(mailbox: &Mailbox) -> String {
  match mailbox {
    Mailbox::Inbox => "Your inbox is empty".into(),
    Mailbox::Starred => "No starred mail".into(),
    Mailbox::Sent => "No sent mail".into(),
    Mailbox::All => "No mail".into(),
    Mailbox::Label(_) => "No mail has this label".into(),
    Mailbox::Search(query) => format!("No mail matches \"{query}\""),
  }
}

/// How a person reads: their name, or their address when the email carries no name.
pub fn person(address: &MailAddress) -> &str {
  let name = address.name.trim();
  if name.is_empty() {
    address.email.as_str()
  } else {
    name
  }
}

/// The first word of a person's name, or their address without a name.
fn first_name(address: &MailAddress) -> &str {
  let name = address.name.trim();
  if name.is_empty() {
    return address.email.as_str();
  }
  name.split_whitespace().next().unwrap_or(name)
}

/// Whether `address` is the signed-in user's, whose address is `me` (empty while unknown).
pub fn is_me(address: &MailAddress, me: &str) -> bool {
  !me.is_empty() && address.email.eq_ignore_ascii_case(me)
}

/// A thread's people on one line, as Gmail lists them: one sender by full name, several by first names, the
/// signed-in user (`me`, their address) as `You`, then the number of emails when there is more than one.
pub fn senders_line(senders: &[MailAddress], messages: u32, me: &str) -> String {
  let names = match senders {
    [] => String::new(),
    [one] if is_me(one, me) => "You".into(),
    [one] => person(one).to_string(),
    many => many
      .iter()
      .map(|sender| {
        if is_me(sender, me) {
          "You"
        } else {
          first_name(sender)
        }
      })
      .collect::<Vec<_>>()
      .join(", "),
  };
  match (names.is_empty(), messages > 1) {
    (true, _) => "(no sender)".into(),
    (false, true) => format!("{names} ({messages})"),
    (false, false) => names,
  }
}

/// One row of the labels popover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelRow {
  /// What `Mailbox::Label` takes.
  pub id: String,
  /// What the row says: the label's own name in the tree, its whole path in a filtered list.
  pub name: String,
  /// Its whole name, parents included.
  pub path: String,
  /// How many of its parents are above it.
  pub depth: usize,
  pub color: Option<u32>,
  pub unread: u32,
  /// Labels sit under it: its toggle folds them.
  pub parent: bool,
  /// It is folded: the labels under it are not listed.
  pub folded: bool,
}

/// A label's own name: the last part of its path.
pub fn leaf_name(path: &str) -> &str {
  path.rsplit('/').next().unwrap_or(path)
}

/// The rows of the labels popover. Without a filter, the tree A to Z, each label under its parents and the labels
/// under a parent in `folded` (by id) left out; with one, the labels that match it (`Fuzzy`), best first, each by its
/// whole path.
pub fn label_rows(labels: &[MailLabel], filter: &str, folded: &HashSet<String>) -> Vec<LabelRow> {
  let row = |label: &MailLabel, name: String, depth: usize, parent: bool, folded: bool| LabelRow {
    id: label.id.clone(),
    name,
    path: label.name.clone(),
    depth,
    color: label.color,
    unread: label.unread,
    parent,
    folded,
  };
  let filter = filter.trim();
  if !filter.is_empty() {
    let mut fuzzy = Fuzzy::new(filter);
    return fuzzy
      .rank(labels.iter(), |label| label.name.as_str())
      .into_iter()
      .map(|(_, label)| row(label, label.name.clone(), 0, false, false))
      .collect();
  }
  let names: HashSet<&str> = labels.iter().map(|label| label.name.as_str()).collect();
  let key = |label: &&MailLabel| label.name.split('/').map(str::to_lowercase).collect::<Vec<_>>();
  let mut sorted: Vec<&MailLabel> = labels.iter().collect();
  sorted.sort_by_key(key);
  let mut rows = Vec::with_capacity(sorted.len());
  // The path, with its `/`, of the folded parent whose labels are being left out.
  let mut hiding: Option<String> = None;
  for label in sorted {
    if let Some(prefix) = &hiding {
      if label.name.starts_with(prefix.as_str()) {
        continue;
      }
      hiding = None;
    }
    // The parents that are labels of their own: the row sits under the deepest one and says the rest of the path.
    let mut depth = 0;
    let mut start = 0;
    for (at, _) in label.name.match_indices('/') {
      if names.contains(&label.name[..at]) {
        depth += 1;
        start = at + 1;
      }
    }
    let under = format!("{}/", label.name);
    let parent = names.iter().any(|name| name.starts_with(under.as_str()));
    let is_folded = parent && folded.contains(&label.id);
    if is_folded {
      hiding = Some(under);
    }
    rows.push(row(label, label.name[start..].to_string(), depth, parent, is_folded));
  }
  rows
}

/// An email's recipients on one line: `to Jordan Park, Riley Brooks` and, when it has any, `cc …` after them.
pub fn recipients_line(to: &[MailAddress], cc: &[MailAddress]) -> String {
  let join = |people: &[MailAddress]| people.iter().map(person).collect::<Vec<_>>().join(", ");
  match (to.is_empty(), cc.is_empty()) {
    (true, true) => String::new(),
    (false, true) => format!("to {}", join(to)),
    (true, false) => format!("cc {}", join(cc)),
    (false, false) => format!("to {}, cc {}", join(to), join(cc)),
  }
}

/// The recipients of a reply on one line, as its box says them: `To Jordan Park, cc Riley Brooks`.
pub fn reply_line(to: &[MailAddress], cc: &[MailAddress]) -> String {
  let line = recipients_line(to, cc);
  let mut chars = line.chars();
  chars
    .next()
    .map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// Whether Reply all reaches someone Reply does not: `all` and `one` are each a reply's `(to, cc)`.
pub fn adds_people(one: (&[MailAddress], &[MailAddress]), all: (&[MailAddress], &[MailAddress])) -> bool {
  let reached: HashSet<String> = one.0.iter().chain(one.1).map(|person| person.email.to_lowercase()).collect();
  all
    .0
    .iter()
    .chain(all.1)
    .any(|person| !reached.contains(&person.email.to_lowercase()))
}

/// The thread to select once thread `gone` leaves a list of `ids` (newest first): the one after it, or at the end the
/// one before it; none when it was alone or is not listed.
pub fn after_removal(ids: &[&str], gone: &str) -> Option<String> {
  let at = ids.iter().position(|id| *id == gone)?;
  ids
    .get(at + 1)
    .or_else(|| at.checked_sub(1).and_then(|before| ids.get(before)))
    .map(|id| (*id).to_string())
}

/// A file size as a person reads it: `512 B`, `14 KB`, `3.2 MB`.
pub fn size_text(bytes: u64) -> String {
  const KB: u64 = 1024;
  const MB: u64 = KB * 1024;
  const GB: u64 = MB * 1024;
  let tenths = |unit: u64| {
    let tenths = (bytes * 10 + unit / 2) / unit;
    if tenths >= 100 {
      format!("{}", tenths / 10)
    } else {
      format!("{}.{}", tenths / 10, tenths % 10)
    }
  };
  match bytes {
    0..KB => format!("{bytes} B"),
    KB..MB => format!("{} KB", tenths(KB)),
    MB..GB => format!("{} MB", tenths(MB)),
    _ => format!("{} GB", tenths(GB)),
  }
}

/// A mail attachment as the media viewer takes it: images open as images, anything else as a file. The viewer
/// reads `url` through `Core::fetch`.
pub fn attachment(file: &MailAttachment) -> Attachment {
  let kind = if file.mime.to_ascii_lowercase().starts_with("image/") {
    AttachmentKind::Image
  } else {
    AttachmentKind::File
  };
  Attachment {
    kind,
    name: file.name.clone(),
    mime: file.mime.clone(),
    url: file.url.clone(),
    thumb_url: None,
    width: None,
    height: None,
  }
}

/// The start of an email's text on one line, for a collapsed email.
pub fn one_line(text: &str) -> String {
  text
    .lines()
    .map(str::trim)
    .find(|line| !line.is_empty())
    .unwrap_or_default()
    .split_whitespace()
    .collect::<Vec<_>>()
    .join(" ")
}

/// Whether the reading pane fits beside the list in a panel `width` wide.
pub const fn splits(width: f32) -> bool {
  width >= LIST_WIDTH + READER_MIN_WIDTH
}

/// Whether an email of a thread shows whole: the newest does, the older ones show one line, and a click on one swaps
/// it (`toggled`).
pub const fn expanded(newest: bool, toggled: bool) -> bool {
  newest != toggled
}

/// The row the selection goes to in a list of `len` rows, one down (`forward`) or up from `current`. With none
/// selected, the first row; at either end it stays.
pub fn step(current: Option<usize>, len: usize, forward: bool) -> Option<usize> {
  let last = len.checked_sub(1)?;
  Some(match (current, forward) {
    (Some(ix), true) => (ix + 1).min(last),
    (Some(ix), false) => ix.saturating_sub(1).min(last),
    (None, _) => 0,
  })
}

/// An address typed, pasted, or picked for To or Cc: `valid` is a plausible `local@domain`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypedAddress {
  /// Display name, empty when the user typed the address.
  pub name: String,
  /// The email.
  pub email: String,
  /// Whether `email` is a plausible `local@domain`.
  pub valid: bool,
}

impl TypedAddress {
  /// An address the user typed or pasted, with no name.
  pub fn typed(email: &str) -> Self {
    let email = email.trim().to_string();
    let valid = address_ok(&email);
    Self { name: String::new(), email, valid }
  }

  /// An address picked from the people directory.
  pub fn person(name: &str, email: &str) -> Self {
    let email = email.trim().to_string();
    let valid = address_ok(&email);
    Self {
      name: name.trim().to_string(),
      email,
      valid,
    }
  }

  /// The address as the core takes it.
  pub fn mail(&self) -> MailAddress {
    MailAddress {
      name: self.name.clone(),
      email: self.email.clone(),
    }
  }
}

/// A person the address field can offer, by name and email.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressHint {
  pub name: String,
  pub email: String,
}

/// Whether `email` is a plausible `local@domain`: one `@`, no spaces, a local part, a domain with a dot.
pub fn address_ok(email: &str) -> bool {
  let email = email.trim();
  let Some((local, domain)) = email.split_once('@') else {
    return false;
  };
  if local.is_empty() || domain.is_empty() {
    return false;
  }
  if local.contains(char::is_whitespace) || domain.contains(char::is_whitespace) {
    return false;
  }
  if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
    return false;
  }
  if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') || domain.contains("..") {
    return false;
  }
  true
}

/// Splits typed or pasted text into addresses. A comma or semicolon always ends one; whitespace ends one only when
/// the token has `@` (so a name query can still contain a space). With `finish` (Enter), a leftover token becomes an
/// address too. Empty pieces are skipped.
pub fn take_addresses(text: &str, finish: bool) -> (Vec<TypedAddress>, String) {
  let mut chips = Vec::new();
  let mut token = String::new();
  for c in text.chars() {
    if c == ',' || c == ';' {
      push_token(&mut token, &mut chips);
      continue;
    }
    if c.is_whitespace() && token.contains('@') {
      push_token(&mut token, &mut chips);
      continue;
    }
    token.push(c);
  }
  if finish {
    push_token(&mut token, &mut chips);
    (chips, String::new())
  } else {
    (chips, token)
  }
}

fn push_token(token: &mut String, chips: &mut Vec<TypedAddress>) {
  let text = token.trim();
  if !text.is_empty() {
    chips.push(TypedAddress::typed(text));
  }
  token.clear();
}

/// Whether `email` is already among `chips` (the address, case-insensitive).
pub fn address_taken(chips: &[TypedAddress], email: &str) -> bool {
  chips.iter().any(|chip| chip.email.eq_ignore_ascii_case(email))
}

/// People to offer for `query`, best first, without those whose email is empty or already in `taken`. With no query,
/// they stay in the order given (the caller puts DMs first).
pub fn rank_addresses<'a>(query: &str, people: &'a [AddressHint], taken: &[&str]) -> Vec<&'a AddressHint> {
  let taken_hit = |email: &str| taken.iter().any(|held| held.eq_ignore_ascii_case(email));
  let offered = people
    .iter()
    .filter(|person| !person.email.is_empty() && !taken_hit(&person.email));
  let query = query.trim();
  if query.is_empty() {
    return offered.collect();
  }
  let mut fuzzy = Fuzzy::new(query);
  let mut hits: Vec<(u32, &AddressHint)> = offered
    .filter_map(|person| {
      let score = fuzzy.score(&person.name).into_iter().chain(fuzzy.score(&person.email)).max();
      score.map(|score| (score, person))
    })
    .collect();
  hits.sort_by(|a, b| b.0.cmp(&a.0));
  hits.into_iter().map(|(_, person)| person).collect()
}

/// What a token still typed in To or Cc becomes when its field loses the keyboard: a chip when it is a valid address,
/// or the name or address of exactly one person; otherwise nothing, and it stays in the field.
pub fn commit_token(token: &str, people: &[AddressHint]) -> Option<TypedAddress> {
  let token = token.trim();
  if token.is_empty() {
    return None;
  }
  if address_ok(token) {
    return Some(TypedAddress::typed(token));
  }
  let mut hits = people.iter().filter(|person| {
    !person.email.is_empty() && (person.name.eq_ignore_ascii_case(token) || person.email.eq_ignore_ascii_case(token))
  });
  let person = hits.next()?;
  if hits.next().is_some() {
    return None;
  }
  Some(TypedAddress::person(&person.name, &person.email))
}

/// One address field of a new email: its chips and the text still typed in it.
pub struct AddressField<'a> {
  pub chips: &'a [TypedAddress],
  pub typed: &'a str,
}

/// Why a new email cannot be sent yet, shown beside Send; `None` when it can. A token still typed that would
/// become a chip counts as a recipient; one that would not blocks Send, as an invalid chip does.
pub fn compose_blocker(
  to: &AddressField<'_>,
  cc: &AddressField<'_>,
  subject: &str,
  body: &str,
  people: &[AddressHint],
) -> Option<&'static str> {
  let check = |field: &AddressField<'_>| {
    let pending = commit_token(field.typed, people);
    let stuck = !field.typed.trim().is_empty() && pending.is_none();
    let bad = stuck || field.chips.iter().any(|chip| !chip.valid);
    (bad, field.chips.len() + usize::from(pending.is_some()))
  };
  let (to_bad, to_count) = check(to);
  let (cc_bad, cc_count) = check(cc);
  if to_bad {
    return Some("Check the address in To");
  }
  if cc_bad {
    return Some("Check the address in Cc");
  }
  if to_count + cc_count == 0 {
    return Some("Add a recipient");
  }
  if subject.trim().is_empty() && body.trim().is_empty() {
    return Some("Write a subject or a message");
  }
  None
}

/// A signature as it shows under an email's body: its plain text, at most `lines` lines, the rest cut with `…`.
pub fn signature_preview(text: &str, lines: usize) -> String {
  let all: Vec<&str> = text.trim().lines().collect();
  let mut shown = all.iter().take(lines).copied().collect::<Vec<_>>().join("\n");
  if all.len() > lines {
    shown.push('…');
  }
  shown
}

#[cfg(test)]
mod tests {
  use super::*;

  fn address(name: &str, email: &str) -> MailAddress {
    MailAddress { name: name.into(), email: email.into() }
  }

  #[test]
  fn a_person_reads_as_their_name_or_else_their_address() {
    assert_eq!(person(&address("Jordan Park", "jpark@example.com")), "Jordan Park");
    assert_eq!(person(&address("  ", "rbrooks@example.com")), "rbrooks@example.com");
  }

  #[test]
  fn senders_read_as_gmail_lists_them() {
    let jordan = address("Jordan Park", "jpark@example.com");
    let riley = address("Riley Brooks", "rbrooks@example.com");
    let bare = address("", "team@example.com");
    assert_eq!(senders_line(std::slice::from_ref(&jordan), 1, ""), "Jordan Park");
    assert_eq!(senders_line(std::slice::from_ref(&jordan), 3, ""), "Jordan Park (3)");
    assert_eq!(
      senders_line(&[jordan, riley, bare], 4, ""),
      "Jordan, Riley, team@example.com (4)"
    );
    assert_eq!(senders_line(&[], 1, ""), "(no sender)");
  }

  #[test]
  fn recipients_read_on_one_line() {
    let jordan = address("Jordan Park", "jpark@example.com");
    let riley = address("", "rbrooks@example.com");
    assert_eq!(recipients_line(std::slice::from_ref(&jordan), &[]), "to Jordan Park");
    assert_eq!(
      recipients_line(std::slice::from_ref(&jordan), std::slice::from_ref(&riley)),
      "to Jordan Park, cc rbrooks@example.com"
    );
    assert_eq!(recipients_line(&[], std::slice::from_ref(&riley)), "cc rbrooks@example.com");
    assert_eq!(recipients_line(&[], &[]), "");
  }

  #[test]
  fn sizes_read_in_the_unit_that_fits() {
    assert_eq!(size_text(0), "0 B");
    assert_eq!(size_text(1023), "1023 B");
    assert_eq!(size_text(1024), "1.0 KB");
    assert_eq!(size_text(1536), "1.5 KB");
    assert_eq!(size_text(14 * 1024), "14 KB");
    assert_eq!(size_text(3 * 1024 * 1024 + 200 * 1024), "3.2 MB");
    assert_eq!(size_text(5 * 1024 * 1024 * 1024), "5.0 GB");
  }

  #[test]
  fn images_open_as_images_and_the_rest_as_files() {
    let file = |name: &str, mime: &str| MailAttachment {
      name: name.into(),
      mime: mime.into(),
      size: 10,
      url: "https://mail.example.com/attachment?id=1".into(),
    };
    let photo = attachment(&file("photo.png", "IMAGE/PNG"));
    assert_eq!(photo.kind, AttachmentKind::Image);
    assert_eq!(photo.url, "https://mail.example.com/attachment?id=1");
    assert_eq!(photo.name, "photo.png");
    assert_eq!(attachment(&file("notes.pdf", "application/pdf")).kind, AttachmentKind::File);
  }

  #[test]
  fn one_line_takes_the_first_words() {
    assert_eq!(one_line("\n\n  Hi Jordan,\n\nThe   notes are attached."), "Hi Jordan,");
    assert_eq!(one_line("   "), "");
  }

  #[test]
  fn the_reader_goes_beside_the_list_only_with_room() {
    assert!(!splits(LIST_WIDTH + READER_MIN_WIDTH - 1.));
    assert!(splits(LIST_WIDTH + READER_MIN_WIDTH));
  }

  #[test]
  fn the_newest_email_shows_whole_until_swapped() {
    assert!(expanded(true, false));
    assert!(!expanded(true, true));
    assert!(!expanded(false, false));
    assert!(expanded(false, true));
  }

  #[test]
  fn the_selection_steps_and_stops_at_the_ends() {
    assert_eq!(step(None, 0, true), None);
    assert_eq!(step(None, 3, false), Some(0));
    assert_eq!(step(Some(0), 3, true), Some(1));
    assert_eq!(step(Some(2), 3, true), Some(2));
    assert_eq!(step(Some(0), 3, false), Some(0));
    // A selection past a list that shrank comes back to its end.
    assert_eq!(step(Some(7), 3, false), Some(2));
  }

  #[test]
  fn mailboxes_are_named_for_the_switcher_and_when_empty() {
    assert_eq!(
      mailboxes().map(|mailbox| mailbox_label(&mailbox)),
      ["Inbox", "Starred", "Sent", "All mail"]
    );
    let search = Mailbox::Search("from:jpark@example.com".into());
    assert_eq!(mailbox_label(&search), "Results for \"from:jpark@example.com\"");
    assert_eq!(empty_text(&search), "No mail matches \"from:jpark@example.com\"");
    assert_eq!(empty_text(&Mailbox::Inbox), "Your inbox is empty");
  }
}

#[cfg(test)]
mod label_tests {
  use super::*;

  fn label(id: &str, name: &str, unread: u32) -> MailLabel {
    MailLabel {
      id: id.into(),
      name: name.into(),
      color: None,
      unread,
    }
  }

  fn labels() -> Vec<MailLabel> {
    vec![
      label("3", "Work/Clients", 2),
      label("1", "travel", 0),
      label("2", "Work", 0),
      label("4", "Work/Clients/Acme", 1),
      label("5", "Work-Archive", 0),
      label("6", "Receipts/2026", 0),
    ]
  }

  #[test]
  fn the_tree_nests_by_path_a_to_z() {
    let rows = label_rows(&labels(), "", &HashSet::new());
    let shape: Vec<(&str, usize, bool)> = rows.iter().map(|row| (row.name.as_str(), row.depth, row.parent)).collect();
    assert_eq!(
      shape,
      [
        // A parent that is not a label of its own leaves its child at the top, by its whole path.
        ("Receipts/2026", 0, false),
        ("travel", 0, false),
        ("Work", 0, true),
        ("Clients", 1, true),
        ("Acme", 2, false),
        ("Work-Archive", 0, false),
      ]
    );
    assert_eq!(rows[3].path, "Work/Clients");
    assert_eq!(rows[3].unread, 2);
  }

  #[test]
  fn a_folded_parent_hides_what_is_under_it_only() {
    let folded = HashSet::from(["2".to_string()]);
    let rows = label_rows(&labels(), "", &folded);
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    // "Receipts" is not a label of its own: its child keeps the whole path at the top.
    assert_eq!(names, ["Receipts/2026", "travel", "Work", "Work-Archive"]);
    assert!(rows[2].folded);
  }

  #[test]
  fn a_filter_lists_matches_by_their_whole_path() {
    let rows = label_rows(&labels(), "acme", &HashSet::from(["2".to_string()]));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "Work/Clients/Acme");
    assert_eq!(rows[0].depth, 0);
    assert_eq!(label_rows(&labels(), "zzz", &HashSet::new()).len(), 0);
  }

  #[test]
  fn the_user_reads_as_you() {
    let me = MailAddress {
      name: String::new(),
      email: "jpark@example.com".into(),
    };
    let riley = MailAddress {
      name: "Riley Brooks".into(),
      email: "rbrooks@example.com".into(),
    };
    assert_eq!(senders_line(std::slice::from_ref(&me), 1, "JPark@example.com"), "You");
    assert_eq!(senders_line(&[riley, me.clone()], 3, "jpark@example.com"), "Riley, You (3)");
    assert_eq!(senders_line(std::slice::from_ref(&me), 1, ""), "jpark@example.com");
    assert_eq!(leaf_name("Work/Clients/Acme"), "Acme");
  }
}

#[cfg(test)]
mod write_tests {
  use super::*;

  fn address(name: &str, email: &str) -> MailAddress {
    MailAddress { name: name.into(), email: email.into() }
  }

  #[test]
  fn a_reply_names_its_recipients_from_a_capital() {
    let jordan = address("Jordan Park", "jpark@example.com");
    let riley = address("", "rbrooks@example.com");
    assert_eq!(
      reply_line(std::slice::from_ref(&jordan), std::slice::from_ref(&riley)),
      "To Jordan Park, cc rbrooks@example.com"
    );
    assert_eq!(reply_line(&[], &[]), "");
  }

  #[test]
  fn reply_all_counts_only_when_it_reaches_someone_new() {
    let jordan = address("Jordan Park", "jpark@example.com");
    let riley = address("Riley Brooks", "rbrooks@example.com");
    let one = [jordan.clone()];
    assert!(!adds_people((&one, &[]), (&[address("", "JPark@example.com")], &[])));
    assert!(adds_people((&one, &[]), (&one, std::slice::from_ref(&riley))));
    assert!(!adds_people(
      (&[jordan], std::slice::from_ref(&riley)),
      (&[], std::slice::from_ref(&riley))
    ));
  }

  #[test]
  fn an_archive_moves_the_selection_down_or_at_the_end_up() {
    let ids = ["t1", "t2", "t3"];
    assert_eq!(after_removal(&ids, "t1").as_deref(), Some("t2"));
    assert_eq!(after_removal(&ids, "t3").as_deref(), Some("t2"));
    assert_eq!(after_removal(&["t1"], "t1"), None);
    assert_eq!(after_removal(&ids, "t9"), None);
  }
}

#[cfg(test)]
mod compose_tests {
  use super::*;

  fn hint(name: &str, email: &str) -> AddressHint {
    AddressHint { name: name.into(), email: email.into() }
  }

  #[test]
  fn a_plausible_address_is_local_at_domain() {
    assert!(address_ok("jpark@example.com"));
    assert!(address_ok("  Riley.Brooks@mail.example.com  "));
    assert!(!address_ok(""));
    assert!(!address_ok("jordan"));
    assert!(!address_ok("jordan@"));
    assert!(!address_ok("@example.com"));
    assert!(!address_ok("jordan@example"));
    assert!(!address_ok("jordan park@example.com"));
    assert!(!address_ok(".jordan@example.com"));
    assert!(!address_ok("jordan@example..com"));
  }

  #[test]
  fn commas_semicolons_and_spaces_end_an_address() {
    let (chips, rest) = take_addresses("jpark@example.com, rbrooks@example.com; team@example.com,", false);
    assert_eq!(
      chips.iter().map(|chip| chip.email.as_str()).collect::<Vec<_>>(),
      ["jpark@example.com", "rbrooks@example.com", "team@example.com"]
    );
    assert_eq!(rest, "");
    let (chips, rest) = take_addresses("jpark@example.com rbrooks@example.com", false);
    assert_eq!(chips[0].email, "jpark@example.com");
    assert_eq!(rest, "rbrooks@example.com");
  }

  #[test]
  fn a_name_query_keeps_its_spaces() {
    let (chips, rest) = take_addresses("Jordan Park", false);
    assert_eq!(chips, []);
    assert_eq!(rest, "Jordan Park");
  }

  #[test]
  fn enter_takes_the_leftover_token() {
    let (chips, rest) = take_addresses("not-an-email", true);
    assert_eq!(chips.len(), 1);
    assert!(!chips[0].valid);
    assert_eq!(chips[0].email, "not-an-email");
    assert_eq!(rest, "");
  }

  #[test]
  fn empty_pieces_are_skipped() {
    let (chips, rest) = take_addresses(",, ;  ", true);
    assert_eq!(chips, []);
    assert_eq!(rest, "");
  }

  #[test]
  fn suggestions_rank_name_and_email_and_skip_taken() {
    let people = [
      hint("Jordan Park", "jpark@example.com"),
      hint("Riley Brooks", "rbrooks@example.com"),
      hint("Sam Okafor", "sam@example.com"),
      hint("No Mail", ""),
    ];
    let taken = ["rbrooks@example.com"];
    let ranked = rank_addresses("jpark", &people, &taken);
    assert_eq!(
      ranked.iter().map(|p| p.email.as_str()).collect::<Vec<_>>(),
      ["jpark@example.com"]
    );
    let ranked = rank_addresses("brook", &people, &taken);
    assert_eq!(ranked, [] as [&AddressHint; 0]);
    let ranked = rank_addresses("", &people, &taken);
    assert_eq!(
      ranked.iter().map(|p| p.email.as_str()).collect::<Vec<_>>(),
      ["jpark@example.com", "sam@example.com"]
    );
  }

  #[test]
  fn send_needs_a_valid_recipient_and_some_text() {
    let jordan = TypedAddress::person("Jordan Park", "jpark@example.com");
    let bad = TypedAddress::typed("not-an-email");
    let one = [jordan.clone()];
    let none: [TypedAddress; 0] = [];
    let field = |chips, typed| AddressField { chips, typed };
    let empty = field(&none, "");
    assert_eq!(compose_blocker(&field(&one, ""), &empty, "Hello", "", &[]), None);
    assert_eq!(compose_blocker(&field(&one, ""), &empty, "", "Hi", &[]), None);
    assert_eq!(
      compose_blocker(&field(&one, ""), &empty, "  ", "\n", &[]),
      Some("Write a subject or a message")
    );
    assert_eq!(compose_blocker(&empty, &empty, "Hello", "Hi", &[]), Some("Add a recipient"));
    let both = [jordan, bad];
    assert_eq!(
      compose_blocker(&field(&both, ""), &empty, "Hello", "Hi", &[]),
      Some("Check the address in To")
    );
  }

  #[test]
  fn a_valid_address_still_typed_counts_as_a_recipient() {
    let none: [TypedAddress; 0] = [];
    let to = AddressField {
      chips: &none,
      typed: "rbrooks@example.com",
    };
    let cc = AddressField { chips: &none, typed: "" };
    assert_eq!(compose_blocker(&to, &cc, "Hello", "", &[]), None);
    let to = AddressField { chips: &none, typed: "rbrooks@" };
    assert_eq!(compose_blocker(&to, &cc, "Hello", "", &[]), Some("Check the address in To"));
    let cc = AddressField { chips: &none, typed: "riley" };
    let to = AddressField { chips: &none, typed: "" };
    assert_eq!(compose_blocker(&to, &cc, "Hello", "", &[]), Some("Check the address in Cc"));
  }

  #[test]
  fn a_token_commits_when_valid_or_exactly_one_person() {
    let people = [
      hint("Jordan Park", "jpark@example.com"),
      hint("Riley Brooks", "rbrooks@example.com"),
      hint("Riley Brooks", "riley.b@example.com"),
    ];
    let chip = commit_token("  sam@example.com ", &people).unwrap();
    assert_eq!((chip.email.as_str(), chip.valid), ("sam@example.com", true));
    let chip = commit_token("jordan park", &people).unwrap();
    assert_eq!((chip.name.as_str(), chip.email.as_str()), ("Jordan Park", "jpark@example.com"));
    assert_eq!(commit_token("Riley Brooks", &people), None);
    assert_eq!(commit_token("jor", &people), None);
    assert_eq!(commit_token("jordan@", &people), None);
    assert_eq!(commit_token("   ", &people), None);
  }

  #[test]
  fn a_signature_shows_four_lines_at_most() {
    assert_eq!(signature_preview("Jordan Park\nExample Co.", 4), "Jordan Park\nExample Co.");
    assert_eq!(signature_preview("\n1\n2\n3\n4\n5\n", 4), "1\n2\n3\n4…");
  }
}
