//! When the user shows as online, and when notifications stay quiet: the activity status the settings ask
//! for, turned into what the core tells Google Chat, and the do-not-disturb schedules Google Chat keeps.
#![allow(unsafe_code, reason = "CoreGraphics idle-time FFI")]

use std::time::Duration;

use chrono::{Datelike, Local, Timelike};
use gchat_core::{Command, Core, DndSchedules, Presence};
use gpui_kit::*;

use crate::settings::{ActivityStatus, Settings};

/// Without a key or pointer event for this long, the user is not at the Mac (as Slack counts it).
const IDLE_AFTER_SECS: f64 = 600.;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
  fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
}

fn idle() -> bool {
  // SAFETY: a plain C call. 0 is the combined session state, all bits set is "any input event".
  unsafe { CGEventSourceSecondsSinceLastEventType(0, u32::MAX) >= IDLE_AFTER_SECS }
}

fn now_minute() -> u16 {
  let now = Local::now();
  (now.hour() * 60 + now.minute()) as u16
}

/// Whether `minute` of the day is inside `start..end`. A range that ends at or before its start runs past
/// midnight.
const fn within(start: u16, end: u16, minute: u16) -> bool {
  if start < end {
    start <= minute && minute < end
  } else {
    minute >= start || minute < end
  }
}

pub fn presence(settings: &Settings) -> Presence {
  let at_mac = || {
    if idle() {
      Presence::Idle
    } else {
      Presence::Active
    }
  };
  let in_hours = within(settings.hours_start, settings.hours_end, now_minute());
  match settings.status {
    ActivityStatus::Automatic => at_mac(),
    ActivityStatus::AutomaticInHours if in_hours => at_mac(),
    ActivityStatus::OnlineInHours if in_hours => Presence::Active,
    ActivityStatus::AutomaticInHours | ActivityStatus::OnlineInHours => Presence::Away,
    ActivityStatus::AlwaysOnline => Presence::Active,
    ActivityStatus::AlwaysOffline => Presence::Away,
  }
}

/// Keeps the core told how the user should show, as the settings, the clock and the keyboard change it.
pub fn keep_presence(core: Core, cx: &mut App) {
  cx.spawn(async move |cx| {
    // The core passes on only what changed.
    loop {
      let presence = cx.update(|cx| {
        if cx.global::<Settings>().away {
          Presence::Away
        } else {
          presence(cx.global::<Settings>())
        }
      });
      core.send(Command::SetPresence(presence));
      cx.background_executor().timer(Duration::from_secs(5)).await;
    }
  })
  .detach();
}

/// Tells the core how the user should show right now, including a "set as away" from the status button.
pub fn apply(core: &Core, cx: &App) {
  let presence = if cx.global::<Settings>().away {
    Presence::Away
  } else {
    presence(cx.global::<Settings>())
  };
  core.send(Command::SetPresence(presence));
}

/// The do-not-disturb schedules of the account, once Google Chat answered with them.
#[derive(Default)]
pub struct DndState(pub Option<DndSchedules>);

impl Global for DndState {}

/// How the signed-in user shows right now, once Google Chat has answered.
#[derive(Default)]
pub struct Mine(pub gchat_core::MyStatus);

impl Global for Mine {}

/// Whether a schedule that is turned on covers this moment.
pub fn muted_now(schedules: &DndSchedules) -> bool {
  let now = Local::now();
  let today = now.weekday().number_from_monday() as u8;
  let yesterday = if today == 1 { 7 } else { today - 1 };
  let minute = now_minute();
  schedules.schedules.iter().filter(|schedule| schedule.enabled).any(|schedule| {
    if schedule.start < schedule.end {
      schedule.days.contains(&today) && schedule.start <= minute && minute < schedule.end
    } else {
      // Past midnight it belongs to the day it started on.
      (schedule.days.contains(&today) && minute >= schedule.start)
        || (schedule.days.contains(&yesterday) && minute < schedule.end)
    }
  })
}

/// A minute of the day as a clock shows it: "7:00 PM".
pub fn clock(minute: u16) -> String {
  let (hour, minute) = (minute / 60 % 24, minute % 60);
  let half = if hour < 12 { "AM" } else { "PM" };
  format!("{}:{minute:02} {half}", if hour % 12 == 0 { 12 } else { hour % 12 })
}

/// Days of a schedule in the order Google Chat keeps them: "Sun, Mon, Tue".
pub fn days_text(days: &[u8]) -> String {
  let names: Vec<&str> = days
    .iter()
    .filter_map(|day| WEEK.iter().find(|(number, _)| number == day))
    .map(|(_, name)| *name)
    .collect();
  names.join(", ")
}

/// The week as Google Chat orders it, with each day's number in a schedule.
pub const WEEK: [(u8, &str); 7] = [
  (7, "Sun"),
  (1, "Mon"),
  (2, "Tue"),
  (3, "Wed"),
  (4, "Thu"),
  (5, "Fri"),
  (6, "Sat"),
];

#[cfg(test)]
mod tests {
  use super::{clock, days_text, presence, within};
  use crate::settings::{ActivityStatus, Settings};
  use gchat_core::Presence;

  #[test]
  fn within_handles_midnight() {
    assert!(within(9 * 60, 17 * 60, 12 * 60));
    assert!(!within(9 * 60, 17 * 60, 8 * 60));
    assert!(within(22 * 60, 6 * 60, 23 * 60));
    assert!(within(22 * 60, 6 * 60, 2 * 60));
    assert!(!within(22 * 60, 6 * 60, 12 * 60));
  }

  #[test]
  fn clock_is_12_hour() {
    assert_eq!(clock(0), "12:00 AM");
    assert_eq!(clock(12 * 60 + 5), "12:05 PM");
    assert_eq!(clock(19 * 60), "7:00 PM");
  }

  #[test]
  fn days_text_names_in_input_order() {
    assert_eq!(days_text(&[7, 1, 2]), "Sun, Mon, Tue");
    assert_eq!(days_text(&[1, 7]), "Mon, Sun");
    assert_eq!(days_text(&[]), "");
  }

  #[test]
  fn presence_fixed_modes() {
    let online = Settings {
      status: ActivityStatus::AlwaysOnline,
      ..Settings::default()
    };
    let away = Settings {
      status: ActivityStatus::AlwaysOffline,
      ..Settings::default()
    };
    assert_eq!(presence(&online), Presence::Active);
    assert_eq!(presence(&away), Presence::Away);
  }
}
