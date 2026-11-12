//! Small pure helpers: time formatting, initials, avatar colors, fuzzy matching.

use chrono::{DateTime, Datelike, Duration, Local, TimeZone};
use gchat_core::Micros;
use gpui_kit::{Hsla, hsla};

pub fn local(t: Micros) -> DateTime<Local> {
  Local.timestamp_micros(t).single().unwrap_or_else(Local::now)
}

/// `now`, `4 min`, `11:22 AM`, `Yesterday`, `Nov 3`.
pub fn rel_time(t: Micros) -> String {
  if t <= 0 {
    return String::new();
  }
  let now = Local::now();
  let then = local(t);
  let secs = (now - then).num_seconds();
  if secs < 60 {
    return "now".into();
  }
  if secs < 3600 {
    return format!("{} min", secs / 60);
  }
  if then.date_naive() == now.date_naive() {
    return then.format("%-I:%M %p").to_string();
  }
  if then.date_naive() == (now - Duration::days(1)).date_naive() {
    return "Yesterday".into();
  }
  if then.year() == now.year() {
    then.format("%b %-d").to_string()
  } else {
    then.format("%b %-d, %Y").to_string()
  }
}

/// Time shown in a message header: minutes while recent, the clock time otherwise.
pub fn message_time(t: Micros) -> String {
  let now = Local::now();
  let then = local(t);
  let secs = (now - then).num_seconds();
  if secs < 60 {
    "now".into()
  } else if secs < 3600 {
    format!("{} min", secs / 60)
  } else {
    then.format("%-I:%M %p").to_string()
  }
}

pub fn day_key(t: Micros) -> i32 {
  local(t).date_naive().num_days_from_ce()
}

pub fn date_label(t: Micros) -> String {
  let now = Local::now();
  let then = local(t);
  if then.date_naive() == now.date_naive() {
    "Today".into()
  } else if then.date_naive() == (now - Duration::days(1)).date_naive() {
    "Yesterday".into()
  } else if then.year() == now.year() {
    then.format("%A, %b %-d").to_string()
  } else {
    then.format("%A, %b %-d, %Y").to_string()
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
