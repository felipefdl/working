//! What the Meet view accepts and lists, as plain code: the link a Join field's text names, the meetings of a day with
//! their times, and how a date reads. No GPUI here.

use chrono::NaiveDate;
use gchat_core::{CalendarEvent, Micros, Rsvp};

use crate::calendar_layout::{Mode, add_days, days, event_days, local, range, sub_days};

/// The host of every Meet link.
const HOST: &str = "meet.google.com";

/// `text` without `prefix`, whatever the case of the letters it starts with.
fn strip_prefix_nocase<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
  let head = text.get(..prefix.len())?;
  if head.eq_ignore_ascii_case(prefix) {
    text.get(prefix.len()..)
  } else {
    None
  }
}

/// What follows the host of a Meet link, with or without its scheme: `abc-defg-hij` of
/// `https://meet.google.com/abc-defg-hij`.
fn after_host(text: &str) -> Option<&str> {
  let rest = strip_prefix_nocase(text, "https://")
    .or_else(|| strip_prefix_nocase(text, "http://"))
    .unwrap_or(text);
  strip_prefix_nocase(rest, HOST)?.strip_prefix('/')
}

/// Whether `name` can be a meeting code or a nickname: letters, digits, hyphens, and underscores, with a letter or a
/// digit among them.
fn is_name(name: &str) -> bool {
  let allowed = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
  name.chars().all(allowed) && name.chars().any(|c| c.is_ascii_alphanumeric())
}

/// A code of ten letters written without dashes gets them: `abc-defg-hij`. Anything else stays as it is.
fn dashed(name: &str) -> String {
  if name.len() != 10 || !name.chars().all(|c| c.is_ascii_lowercase()) {
    return name.to_string();
  }
  let mut code = String::with_capacity(12);
  for (ix, letter) in name.chars().enumerate() {
    if ix == 3 || ix == 7 {
      code.push('-');
    }
    code.push(letter);
  }
  code
}

/// The path of the Meet link of `named`: a code, a nickname, or `lookup/` and an id.
fn meeting_path(named: &str) -> Option<String> {
  if let Some(id) = named.strip_prefix("lookup/") {
    return is_name(id).then(|| format!("lookup/{id}"));
  }
  is_name(named).then(|| dashed(named))
}

/// The Meet link that the text of a Join field names: a Meet link, a meeting code with or without dashes
/// (`abc-defg-hij`, `abcdefghij`), or a nickname. `None` for anything else. Ten letters in a row are taken for a code,
/// so a nickname of exactly ten letters gets dashes too.
pub fn meeting_link(text: &str) -> Option<String> {
  let text = text.trim();
  let named = after_host(text).unwrap_or(text);
  // A pasted link may carry a query, a fragment, or a closing slash: none of them is part of the meeting.
  let named = named.split(['?', '#']).next()?.trim_end_matches('/').to_ascii_lowercase();
  meeting_path(&named).map(|path| format!("https://{HOST}/{path}"))
}

/// One meeting of a day, as its row shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meeting {
  pub title: String,
  /// `1:45 PM – 2:45 PM`, or `All day`.
  pub when: String,
  /// The Google Meet link it opens.
  pub url: String,
}

/// `1:45 PM`: a clock time as Meet writes it.
fn clock(time: Micros) -> String {
  local(time).format("%-I:%M %p").to_string()
}

fn time_text(event: &CalendarEvent) -> String {
  if event.all_day {
    "All day".into()
  } else {
    format!("{} – {}", clock(event.start), clock(event.end))
  }
}

/// Whether `event` is a meeting to list on `day`: it is on that day, the user did not decline it, and its calendar is
/// not one of `hidden`.
fn is_listed(event: &CalendarEvent, day: NaiveDate, hidden: &[String]) -> bool {
  let (first, last) = event_days(event);
  let on_day = first <= day && day <= last;
  on_day && event.my_rsvp != Some(Rsvp::Declined) && !hidden.contains(&event.calendar_id)
}

/// The meetings of `day` among `events`: those with a Meet link, except the ones the user declined and the ones of a
/// calendar in `hidden`. All-day meetings come first, then the others by start.
pub fn meetings_on(events: &[CalendarEvent], day: NaiveDate, hidden: &[String]) -> Vec<Meeting> {
  let mut listed: Vec<(&CalendarEvent, &str)> = events
    .iter()
    .filter(|event| is_listed(event, day, hidden))
    .filter_map(|event| {
      let url = event.meet_url.as_deref().filter(|url| !url.is_empty())?;
      Some((event, url))
    })
    .collect();
  listed.sort_by_key(|(event, _)| (!event.all_day, event.start, event.end));
  listed
    .into_iter()
    .map(|(event, url)| Meeting {
      title: event.title.clone(),
      when: time_text(event),
      url: url.to_string(),
    })
    .collect()
}

/// `Fri, Nov 13`.
fn date_label(day: NaiveDate) -> String {
  day.format("%a, %b %-d").to_string()
}

/// What the list says when `day` has no meetings: `today` is the day on the calendar now.
pub fn empty_title(day: NaiveDate, today: NaiveDate) -> String {
  if day == today {
    "No meetings scheduled for today".into()
  } else {
    format!("No meetings scheduled for {}", date_label(day))
  }
}

/// The days of the week strip that shows `day`, Sunday first.
pub fn week_of(day: NaiveDate) -> Vec<NaiveDate> {
  days(Mode::Week, day)
}

/// The day the week arrows go to: the same weekday of the next week, or of the one before.
pub fn shift_week(day: NaiveDate, forward: bool) -> NaiveDate {
  if forward {
    add_days(day, 7)
  } else {
    sub_days(day, 7)
  }
}

/// The time to ask the core for to list `day`: from its first moment to the first one of the day after.
pub fn day_range(day: NaiveDate) -> (Micros, Micros) {
  range(Mode::Day, day)
}

#[cfg(test)]
mod tests {
  use gchat_core::{EventKind, Reminder, Visibility};

  use super::*;
  use crate::calendar_layout::{HOUR, MINUTE, day_start};

  fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("a date")
  }

  /// The link of the meeting `path` names.
  fn link(path: &str) -> String {
    format!("https://meet.google.com/{path}")
  }

  /// A timed event of calendar `c` on `day`, from and to `(hour, minute)` after midnight, with a Meet link.
  fn meeting(day: NaiveDate, from: (i64, i64), to: (i64, i64), title: &str) -> CalendarEvent {
    let base = day_start(day);
    CalendarEvent {
      id: format!("e-{title}"),
      calendar_id: "c".into(),
      title: title.into(),
      start: base + from.0 * HOUR + from.1 * MINUTE,
      end: base + to.0 * HOUR + to.1 * MINUTE,
      all_day: false,
      location: String::new(),
      description: String::new(),
      meet_url: Some("https://meet.google.com/abc-defg-hij".into()),
      link: String::new(),
      color: None,
      color_id: None,
      guests: Vec::new(),
      organizer: String::new(),
      series: None,
      kind: EventKind::Event,
      busy: true,
      visibility: Visibility::Default,
      reminder: Reminder::Default,
      can_edit: true,
      can_delete: true,
      my_rsvp: None,
    }
  }

  fn titles(meetings: &[Meeting]) -> Vec<&str> {
    meetings.iter().map(|row| row.title.as_str()).collect()
  }

  #[test]
  fn a_code_becomes_a_link_with_or_without_dashes() {
    assert_eq!(meeting_link("abc-defg-hij"), Some(link("abc-defg-hij")));
    assert_eq!(meeting_link("abcdefghij"), Some(link("abc-defg-hij")));
    assert_eq!(meeting_link("  ABC-DEFG-HIJ \n"), Some(link("abc-defg-hij")));
    assert_eq!(meeting_link("AbcDefgHij"), Some(link("abc-defg-hij")));
  }

  #[test]
  fn a_pasted_link_keeps_only_its_meeting() {
    let code = Some(link("abc-defg-hij"));
    assert_eq!(meeting_link("https://meet.google.com/abc-defg-hij"), code);
    assert_eq!(meeting_link("http://meet.google.com/abc-defg-hij"), code);
    assert_eq!(meeting_link("meet.google.com/abc-defg-hij"), code);
    assert_eq!(meeting_link("HTTPS://Meet.Google.com/ABC-defg-hij"), code);
    assert_eq!(meeting_link("https://meet.google.com/abc-defg-hij?authuser=1&hs=179"), code);
    assert_eq!(meeting_link("https://meet.google.com/abc-defg-hij/#participants"), code);
  }

  #[test]
  fn a_nickname_is_a_path_of_its_own() {
    assert_eq!(meeting_link("Team-Standup"), Some(link("team-standup")));
    assert_eq!(meeting_link("standup_2"), Some(link("standup_2")));
    assert_eq!(
      meeting_link("https://meet.google.com/lookup/xyz789"),
      Some(link("lookup/xyz789"))
    );
  }

  #[test]
  fn anything_else_is_refused() {
    for text in [
      "",
      "   ",
      "---",
      "abc defg hij",
      "abc-defg-hij!",
      "abc–defg–hij",
      "https://example.com/abc-defg-hij",
      "https://meet.google.com/",
      "https://meet.google.com/abc-defg-hij/more",
      "https://meet.google.com.example.com/abc-defg-hij",
      "https://meet.google.com@example.com/abc-defg-hij",
      "javascript:alert(1)",
      "meet.google.com",
    ] {
      assert_eq!(meeting_link(text), None, "{text:?}");
    }
  }

  #[test]
  fn every_link_is_a_meet_link_with_a_plain_path() {
    for text in [
      "abc-defg-hij",
      "abcdefghij",
      "Team-Standup",
      "standup_2",
      "  https://meet.google.com/abc-defg-hij?authuser=1#top  ",
      "http://MEET.GOOGLE.COM/lookup/xyz789/",
    ] {
      let built = meeting_link(text).expect(text);
      let path = built.strip_prefix("https://meet.google.com/").expect(&built);
      assert!(
        path
          .chars()
          .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '/'))
      );
    }
  }

  #[test]
  fn a_day_lists_its_meetings_by_start() {
    let day = date(2026, 11, 13);
    let mut unlinked = meeting(day, (11, 0), (12, 0), "no link");
    unlinked.meet_url = None;
    let mut blank = meeting(day, (11, 30), (12, 0), "blank link");
    blank.meet_url = Some(String::new());
    let events = [
      meeting(day, (14, 0), (15, 0), "afternoon"),
      meeting(day, (9, 30), (10, 0), "morning"),
      unlinked,
      blank,
      meeting(add_days(day, 1), (9, 0), (10, 0), "tomorrow"),
    ];
    let listed = meetings_on(&events, day, &[]);
    assert_eq!(titles(&listed), ["morning", "afternoon"]);
  }

  #[test]
  fn times_read_as_a_range_and_all_day_comes_first() {
    let day = date(2026, 11, 13);
    let mut whole = meeting(day, (0, 0), (0, 0), "offsite");
    whole.all_day = true;
    whole.end = day_start(add_days(day, 1));
    let events = [meeting(day, (13, 45), (14, 45), "sync"), whole];
    let listed = meetings_on(&events, day, &[]);
    assert_eq!(titles(&listed), ["offsite", "sync"]);
    assert_eq!(listed.first().map(|row| row.when.as_str()), Some("All day"));
    assert_eq!(listed.get(1).map(|row| row.when.as_str()), Some("1:45 PM – 2:45 PM"));
    assert_eq!(listed.get(1).map(|row| row.url.clone()), Some(link("abc-defg-hij")));
  }

  #[test]
  fn events_that_run_through_the_day_are_listed_and_the_others_are_not() {
    let day = date(2026, 11, 13);
    // From eleven at night to one in the morning.
    let overnight = meeting(day, (23, 0), (25, 0), "overnight");
    let listed_on = |on_day: NaiveDate| meetings_on(std::slice::from_ref(&overnight), on_day, &[]);
    assert_eq!(titles(&listed_on(day)), ["overnight"]);
    assert_eq!(titles(&listed_on(add_days(day, 1))), ["overnight"]);
    assert_eq!(titles(&listed_on(add_days(day, 2))), [] as [&str; 0]);
    // An all-day event of the day before ends at this day's midnight.
    let mut earlier = meeting(day, (0, 0), (0, 0), "yesterday");
    earlier.all_day = true;
    earlier.start = day_start(sub_days(day, 1));
    earlier.end = day_start(day);
    assert_eq!(titles(&meetings_on(&[earlier], day, &[])), [] as [&str; 0]);
  }

  #[test]
  fn declined_and_hidden_meetings_are_left_out() {
    let day = date(2026, 11, 13);
    let mut declined = meeting(day, (9, 0), (10, 0), "declined");
    declined.my_rsvp = Some(Rsvp::Declined);
    let mut maybe = meeting(day, (10, 0), (11, 0), "maybe");
    maybe.my_rsvp = Some(Rsvp::Tentative);
    let mut elsewhere = meeting(day, (11, 0), (12, 0), "elsewhere");
    elsewhere.calendar_id = "hidden".into();
    let events = [declined, maybe, elsewhere];
    let listed = meetings_on(&events, day, &["hidden".to_string()]);
    assert_eq!(titles(&listed), ["maybe"]);
  }

  #[test]
  fn the_empty_list_names_the_day() {
    let today = date(2026, 11, 13);
    assert_eq!(empty_title(today, today), "No meetings scheduled for today");
    assert_eq!(empty_title(date(2026, 11, 20), today), "No meetings scheduled for Fri, Nov 20");
    assert_eq!(date_label(today), "Fri, Nov 13");
  }

  #[test]
  fn the_week_strip_runs_from_sunday_to_saturday_around_the_day() {
    // Friday, November 13, 2026.
    let day = date(2026, 11, 13);
    let week = week_of(day);
    assert_eq!(week.len(), 7);
    assert_eq!(week.first(), Some(&date(2026, 11, 8)));
    assert_eq!(week.last(), Some(&date(2026, 11, 14)));
    assert!(week.contains(&day));
    // Every day of the week names the same week.
    assert!(week.iter().all(|other| week_of(*other) == week));
  }

  #[test]
  fn the_week_strip_crosses_months_and_years() {
    let week = week_of(date(2026, 12, 31));
    assert_eq!(week.first(), Some(&date(2026, 12, 27)));
    assert_eq!(week.last(), Some(&date(2027, 1, 2)));
    // A Sunday starts its own week, and a Saturday ends it.
    assert_eq!(week_of(date(2026, 11, 8)).first(), Some(&date(2026, 11, 8)));
    assert_eq!(week_of(date(2026, 11, 14)).last(), Some(&date(2026, 11, 14)));
  }

  #[test]
  fn the_week_arrows_keep_the_weekday() {
    let day = date(2026, 11, 13);
    assert_eq!(shift_week(day, true), date(2026, 11, 20));
    assert_eq!(shift_week(day, false), date(2026, 11, 6));
    assert_eq!(shift_week(date(2026, 12, 28), true), date(2027, 1, 4));
    assert_eq!(shift_week(shift_week(day, true), false), day);
    assert_eq!(week_of(shift_week(day, true)).first(), Some(&date(2026, 11, 15)));
  }

  #[test]
  fn a_day_asks_for_its_own_time() {
    let day = date(2026, 11, 13);
    let (start, end) = day_range(day);
    assert_eq!((start, end), (day_start(day), day_start(add_days(day, 1))));
    // The days follow one another without a gap or an overlap.
    assert_eq!(day_range(add_days(day, 1)).0, end);
    assert_eq!(day_range(sub_days(day, 1)).1, start);
  }
}
