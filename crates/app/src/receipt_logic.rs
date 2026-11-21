//! Where read receipts and the New line land among the messages of a list, and how a group of receipts is told, as
//! plain code. No GPUI here: `message.rs` draws what this decides.

use std::collections::BTreeMap;

use gchat_core::Micros;

/// The most avatars drawn beside one message; the people before them fold into a count.
pub const MAX_SHOWN: usize = 4;

/// The most names a tooltip lists before it counts the rest.
const MAX_NAMED: usize = 10;

/// Which message each reader's avatar sits on.
///
/// `times` are the times of the messages a reader can have read, in the order the list shows them. That is not
/// always the order of their times: a conversation of threads lists its topics by activity. A reader's avatar goes on
/// the newest message sent at or before `read_times[i]`, the later one in the list when two share a time, and nowhere
/// when none was. Answers, for each message index with readers, their indices in `read_times`, in order.
pub fn place(times: &[Micros], read_times: &[Micros]) -> BTreeMap<usize, Vec<usize>> {
  // The messages by time, so each read time finds its message by search.
  let mut by_time: Vec<usize> = (0..times.len()).collect();
  by_time.sort_by_key(|&at| (times[at], at));
  let mut placed: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
  for (reader, &read) in read_times.iter().enumerate() {
    let sent = by_time.partition_point(|&at| times[at] <= read);
    if let Some(&at) = sent.checked_sub(1).and_then(|newest| by_time.get(newest)) {
      placed.entry(at).or_default().push(reader);
    }
  }
  placed
}

/// The readers whose avatars are drawn out of `readers`, and how many fold into the count. These are the last
/// `MAX_SHOWN`: with the earliest reader first, the people who read most recently.
pub fn shown<T>(readers: &[T]) -> (&[T], usize) {
  let folded = readers.len().saturating_sub(MAX_SHOWN);
  (readers.get(folded..).unwrap_or_default(), folded)
}

/// What the tooltip over a group of avatars says: `Read by Jordan Park, Riley Brooks`. A reader without a name is
/// `Someone`, and a long list ends with how many more there are.
pub fn read_by(names: &[&str]) -> String {
  let named: Vec<&str> = names
    .iter()
    .take(MAX_NAMED)
    .map(|name| if name.is_empty() { "Someone" } else { name })
    .collect();
  match names.len().saturating_sub(MAX_NAMED) {
    0 => format!("Read by {}", named.join(", ")),
    more => format!("Read by {} and {more} more", named.join(", ")),
  }
}

/// A message as the New line sees it.
pub struct Sent {
  pub time: Micros,
  /// The user sent it.
  pub own: bool,
  /// It mentions the user.
  pub mention: bool,
  /// The row of the list that stands for it: its own or, for a reply in the main list of a space, the row of the
  /// message that started its thread.
  pub row: usize,
}

/// Where the New line goes in a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewLine {
  /// The row it sits above.
  pub row: usize,
  /// A message past it mentions the user.
  pub mention: bool,
}

/// The line between what the user has read and what is new.
///
/// `read` is the conversation's read time as it was when it opened. The user's own messages move it on: writing is
/// having read, so everything up to their newest message counts as read too. The line goes above the first row, in
/// the order the list shows them, that stands for a message of someone else sent after that.
///
/// With no such message, `marked` says the conversation is unread all the same, as one marked unread by hand is: the
/// line then goes above the newest message of someone else, the one a mark is made from. Otherwise there is no line.
pub fn new_line(sent: &[Sent], read: Micros, marked: bool) -> Option<NewLine> {
  let read = sent
    .iter()
    .filter(|message| message.own)
    .map(|message| message.time)
    .fold(read, Micros::max);
  let mut new = sent.iter().filter(|message| !message.own && message.time > read);
  if let Some(row) = new.clone().map(|message| message.row).min() {
    return Some(NewLine {
      row,
      mention: new.any(|message| message.mention),
    });
  }
  let newest = sent
    .iter()
    .filter(|message| marked && !message.own)
    .max_by_key(|message| message.time)?;
  Some(NewLine { row: newest.row, mention: newest.mention })
}

#[cfg(test)]
mod tests {
  use super::{MAX_SHOWN, NewLine, Sent, new_line, place, read_by, shown};

  /// The places `place` answers, as `(message, readers)` pairs.
  fn placed(times: &[i64], read_times: &[i64]) -> Vec<(usize, Vec<usize>)> {
    place(times, read_times).into_iter().collect()
  }

  /// A message of someone else, shown by `row`.
  const fn theirs(time: i64, row: usize) -> Sent {
    Sent { time, own: false, mention: false, row }
  }

  /// A message of the user, shown by `row`.
  const fn mine(time: i64, row: usize) -> Sent {
    Sent { time, own: true, mention: false, row }
  }

  #[test]
  fn the_new_line_sits_above_the_first_message_past_the_read_time() {
    let sent = [theirs(10, 0), theirs(20, 1), theirs(30, 2)];
    assert_eq!(new_line(&sent, 10, false), Some(NewLine { row: 1, mention: false }));
    // Everything is new, and nothing is.
    assert_eq!(new_line(&sent, 0, false), Some(NewLine { row: 0, mention: false }));
    assert_eq!(new_line(&sent, 30, false), None);
    assert_eq!(new_line(&[], 0, true), None);
  }

  #[test]
  fn the_users_own_messages_are_never_new_and_what_came_before_them_is_read() {
    // Theirs at 20 is past the read time, but the user wrote after it.
    let answered = [theirs(10, 0), theirs(20, 1), mine(30, 2), theirs(40, 3)];
    assert_eq!(new_line(&answered, 10, false), Some(NewLine { row: 3, mention: false }));
    let last_word = [theirs(10, 0), theirs(20, 1), mine(30, 2)];
    assert_eq!(new_line(&last_word, 10, false), None);
  }

  #[test]
  fn a_conversation_marked_unread_has_its_line_above_the_newest_message_of_someone_else() {
    let sent = [theirs(10, 0), theirs(20, 1), mine(30, 2)];
    assert_eq!(new_line(&sent, 30, true), Some(NewLine { row: 1, mention: false }));
    // A message past the read time comes first, marked or not.
    let unanswered = [theirs(10, 0), theirs(20, 1)];
    assert_eq!(new_line(&unanswered, 10, true), new_line(&unanswered, 10, false));
    assert_eq!(new_line(&[mine(30, 0)], 30, true), None);
  }

  #[test]
  fn a_new_reply_puts_the_line_above_its_thread_wherever_the_list_shows_it() {
    // A space of threads: the reply at 50 belongs to the thread row 1 shows, whose first message is old.
    let sent = [theirs(10, 0), theirs(5, 1), theirs(50, 1), theirs(40, 2)];
    assert_eq!(new_line(&sent, 30, false), Some(NewLine { row: 1, mention: false }));
  }

  #[test]
  fn the_line_says_when_a_new_message_mentions_the_user() {
    let mention = |time, row| Sent { time, own: false, mention: true, row };
    // A mention already read does not count; one past the line does, wherever it is under it.
    assert_eq!(
      new_line(&[mention(10, 0), theirs(20, 1), theirs(30, 2)], 10, false),
      Some(NewLine { row: 1, mention: false })
    );
    assert_eq!(
      new_line(&[theirs(10, 0), theirs(20, 1), mention(30, 2)], 10, false),
      Some(NewLine { row: 1, mention: true })
    );
  }

  #[test]
  fn a_reader_lands_on_the_newest_message_at_or_before_their_read_time() {
    let times = [10, 20, 30];
    // On a message, between two, after the newest, and on the oldest.
    assert_eq!(placed(&times, &[20]), [(1, vec![0])]);
    assert_eq!(placed(&times, &[25]), [(1, vec![0])]);
    assert_eq!(placed(&times, &[99]), [(2, vec![0])]);
    assert_eq!(placed(&times, &[10]), [(0, vec![0])]);
    // Before the oldest, there is nothing they have read here.
    assert_eq!(placed(&times, &[9]), Vec::new());
  }

  #[test]
  fn readers_of_one_message_keep_their_order() {
    let times = [10, 20, 30];
    assert_eq!(placed(&times, &[15, 30, 12, 100]), [(0, vec![0, 2]), (2, vec![1, 3])]);
  }

  #[test]
  fn messages_listed_out_of_time_order_are_still_told_by_time() {
    // The first listed is the newest, as a conversation of threads lists a topic that has just had a reply.
    let times = [30, 10, 20];
    assert_eq!(placed(&times, &[25]), [(2, vec![0])]);
    assert_eq!(placed(&times, &[12]), [(1, vec![0])]);
    assert_eq!(placed(&times, &[30]), [(0, vec![0])]);
    assert_eq!(placed(&times, &[5]), Vec::new());
  }

  #[test]
  fn two_messages_at_one_time_take_the_later_one_in_the_list() {
    assert_eq!(placed(&[20, 20, 10], &[20]), [(1, vec![0])]);
    assert_eq!(placed(&[20, 20, 10], &[19]), [(2, vec![0])]);
  }

  #[test]
  fn nothing_to_land_on_or_nobody_to_land_places_nothing() {
    assert_eq!(placed(&[], &[5]), Vec::new());
    assert_eq!(placed(&[5], &[]), Vec::new());
  }

  #[test]
  fn at_most_four_avatars_show_and_the_earliest_readers_fold_into_the_count() {
    assert_eq!(MAX_SHOWN, 4);
    assert_eq!(shown(&[1, 2, 3]), (&[1, 2, 3][..], 0));
    assert_eq!(shown(&[1, 2, 3, 4]), (&[1, 2, 3, 4][..], 0));
    assert_eq!(shown(&[1, 2, 3, 4, 5, 6]), (&[3, 4, 5, 6][..], 2));
    assert_eq!(shown::<u8>(&[]), (&[][..], 0));
  }

  #[test]
  fn the_tooltip_names_the_readers() {
    assert_eq!(read_by(&["Jordan Park"]), "Read by Jordan Park");
    assert_eq!(read_by(&["Jordan Park", "Riley Brooks"]), "Read by Jordan Park, Riley Brooks");
    // A reader nobody has a name for.
    assert_eq!(read_by(&["Jordan Park", ""]), "Read by Jordan Park, Someone");
  }

  #[test]
  fn a_long_tooltip_counts_the_names_it_leaves_out() {
    let names: Vec<String> = (1..=12).map(|number| format!("Person {number}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    assert_eq!(
      read_by(&names),
      "Read by Person 1, Person 2, Person 3, Person 4, Person 5, Person 6, Person 7, Person 8, Person 9, Person 10 \
       and 2 more"
    );
    assert_eq!(read_by(&names[..10]), format!("Read by {}", names[..10].join(", ")));
  }
}
