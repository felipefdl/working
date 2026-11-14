//! Where read receipts land among the messages of a list, and how a group of them is told, as plain code. No GPUI
//! here: `message.rs` draws what this decides.

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

#[cfg(test)]
mod tests {
  use super::{MAX_SHOWN, place, read_by, shown};

  /// The places `place` answers, as `(message, readers)` pairs.
  fn placed(times: &[i64], read_times: &[i64]) -> Vec<(usize, Vec<usize>)> {
    place(times, read_times).into_iter().collect()
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
