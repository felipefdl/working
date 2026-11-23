//! Small pure helpers: time formatting, initials, avatar colors, fuzzy matching.

use gchat_core::Micros;
use gpui_kit::{Hsla, hsla};
use jiff::civil::{Date, Time};
use jiff::{SignedDuration, ZonedDateTime};

pub fn local(t: Micros) -> ZonedDateTime {
  gchat_core::local_time(t).unwrap_or_else(ZonedDateTime::now)
}

/// The local date a day (24 hours) before `now`.
fn day_before(now: &ZonedDateTime) -> Date {
  (now - SignedDuration::from_hours(24)).date()
}

/// `now`, `4 min`, `11:22 AM` (`11:22` on a 24-hour clock), `Yesterday`, `Nov 3`.
pub fn rel_time(t: Micros) -> String {
  if t <= 0 {
    return String::new();
  }
  let now = ZonedDateTime::now();
  let then = local(t);
  let secs = now.duration_since(&then).as_secs();
  if secs < 60 {
    return "now".into();
  }
  if secs < 3600 {
    return format!("{} min", secs / 60);
  }
  if then.date() == now.date() {
    return clock_text(&then, crate::clock::is_24_hour());
  }
  if then.date() == day_before(&now) {
    return "Yesterday".into();
  }
  if then.year() == now.year() {
    then.strftime("%b %-d").to_string()
  } else {
    then.strftime("%b %-d, %Y").to_string()
  }
}

/// Time shown in a message header: minutes while recent, the clock time otherwise.
pub fn message_time(t: Micros) -> String {
  let now = ZonedDateTime::now();
  let then = local(t);
  let secs = now.duration_since(&then).as_secs();
  if secs < 60 {
    "now".into()
  } else if secs < 3600 {
    format!("{} min", secs / 60)
  } else {
    clock_text(&then, crate::clock::is_24_hour())
  }
}

/// `1:50 PM`, or `13:50` when `h24`: a clock time as message headers and lists write it.
pub fn clock_text(at: &ZonedDateTime, h24: bool) -> String {
  if h24 {
    at.strftime("%-H:%M").to_string()
  } else {
    at.strftime("%-I:%M %p").to_string()
  }
}

/// The local day of `t` as a number (days since 0001-01-01), and the span of microseconds of that day, so the times
/// after it on the same day need no time-zone conversion. A day whose midnight cannot be found (a time-zone change at
/// midnight) spans `t` alone.
pub fn day_span(t: Micros) -> (i32, std::ops::Range<Micros>) {
  let date = local(t).date();
  let midnight = |date: Date| gchat_core::local_instant(date.to_datetime(Time::midnight()));
  let span = match (midnight(date), date.tomorrow().ok().and_then(midnight)) {
    (Some(start), Some(end)) if start <= t && t < end => start..end,
    _ => t..t.saturating_add(1),
  };
  let number = date.since(Date::constant(1, 1, 1)).map_or(0, |days| days.get_days());
  (number, span)
}

pub fn date_label(t: Micros) -> String {
  let now = ZonedDateTime::now();
  let then = local(t);
  if then.date() == now.date() {
    "Today".into()
  } else if then.date() == day_before(&now) {
    "Yesterday".into()
  } else if then.year() == now.year() {
    then.strftime("%A, %b %-d").to_string()
  } else {
    then.strftime("%A, %b %-d, %Y").to_string()
  }
}

pub fn initials(name: &str) -> String {
  let mut out = String::new();
  for word in name.split_whitespace() {
    if let Some(c) = word.chars().find(|c| c.is_alphanumeric()) {
      out.extend(c.to_uppercase());
      if out.chars().count() == 2 {
        break;
      }
    }
  }
  if out.is_empty() { "?".into() } else { out }
}

pub fn name_color(name: &str) -> Hsla {
  let mut h: u32 = 2166136261;
  for b in name.bytes() {
    h = (h ^ u32::from(b)).wrapping_mul(16777619);
  }
  hsla((h % 360) as f32 / 360.0, 0.42, 0.40, 1.0)
}

/// Rewrites a `googleusercontent.com` image URL to ask for a square image of `size` pixels.
pub fn sized_avatar(url: &str, size: u32) -> String {
  if !url.contains("googleusercontent.com") {
    return url.to_string();
  }
  let slash = url.rfind('/').unwrap_or(0);
  if let Some(eq) = url[slash..].rfind('=') {
    let eq = slash + eq;
    let tail = &url[eq + 1..];
    let mut chars = tail.chars();
    let sized = matches!(chars.next(), Some('s' | 'w' | 'h')) && chars.next().is_some_and(|c| c.is_ascii_digit());
    return if sized {
      format!("{}=s{}-c", &url[..eq], size)
    } else {
      url.to_string()
    };
  }
  if url.contains('?') {
    return url.to_string();
  }
  format!("{url}=s{size}-c")
}

/// Pixel size to request for an avatar shown at `logical` points.
pub fn avatar_px(logical: f32) -> u32 {
  if logical <= 32.0 { 64 } else { 96 }
}

#[cfg(test)]
mod tests {
  use super::{avatar_px, initials, rel_time, sized_avatar};

  #[test]
  fn initials_take_two_alnum_letters() {
    assert_eq!(initials("Ada Lovelace"), "AL");
    assert_eq!(initials("  ana"), "A");
    assert_eq!(initials("..."), "?");
    assert_eq!(initials(""), "?");
  }

  #[test]
  fn sized_avatar_rewrites_google_urls() {
    assert_eq!(
      sized_avatar("https://lh3.googleusercontent.com/a/A=s96-c", 64),
      "https://lh3.googleusercontent.com/a/A=s64-c"
    );
    assert_eq!(
      sized_avatar("https://lh3.googleusercontent.com/a/A", 64),
      "https://lh3.googleusercontent.com/a/A=s64-c"
    );
    assert_eq!(sized_avatar("https://example.com/a.png", 64), "https://example.com/a.png");
  }

  #[test]
  fn avatar_px_steps() {
    assert_eq!(avatar_px(24.0), 64);
    assert_eq!(avatar_px(32.0), 64);
    assert_eq!(avatar_px(40.0), 96);
  }

  #[test]
  fn rel_time_hides_missing_times() {
    assert_eq!(rel_time(0), "");
    assert_eq!(rel_time(-1), "");
  }
}

#[cfg(test)]
mod day_tests {
  use super::day_span;

  #[test]
  fn day_span_holds_its_time_and_the_rest_of_its_day() {
    let t = 1_700_000_000_000_000;
    let (key, span) = day_span(t);
    assert!(span.contains(&t));
    assert_eq!(day_span(span.start).0, key);
    assert_eq!(day_span(span.end - 1).0, key);
    if span.end - span.start > 1 {
      assert_eq!(day_span(span.end).0, key + 1);
    }
  }
}
