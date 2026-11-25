//! What's New: the changelog baked into the binary from the repository's `CHANGELOG.md`, the dialog that shows it, and
//! the decision to show it once after an update.

use std::sync::Arc;

use gpui_kit::component::input::Escape;
use gpui_kit::component::{FocusTrapElement as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use jiff::civil::Date;

use crate::settings::Settings;
use crate::theme::{BORDER, MUTED, POPOVER, ROW_HOVER, TEXT, is_light, rgb};
use crate::ui::{Icon, icon, primary_button};

/// The changelog as the build found it.
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

/// The version this binary is.
const RUNNING: &str = env!("CARGO_PKG_VERSION");

/// The least and the most the dialog measures as the main window grows.
const SIZE: (Size<f32>, Size<f32>) = (Size { width: 420.0, height: 320.0 }, Size { width: 560.0, height: 720.0 });

/// One change of a release: an optional short title (the bold lead of its line) and the rest of its text.
#[derive(Debug, PartialEq, Eq)]
pub struct Entry<'a> {
  pub title: Option<&'a str>,
  pub text: &'a str,
}

/// One release: its version, the day it came out, and its changes in the file's order.
#[derive(Debug, PartialEq, Eq)]
pub struct Release<'a> {
  pub version: &'a str,
  pub date: Date,
  pub entries: Vec<Entry<'a>>,
}

/// The version and date of a `## <version> - <YYYY-MM-DD>` heading.
fn heading(line: &str) -> Option<(&str, Date)> {
  let (version, date) = line.strip_prefix("## ")?.split_once(" - ")?;
  let version = version.trim();
  if version.is_empty() || version.contains(char::is_whitespace) {
    return None;
  }
  Some((version, Date::strptime("%Y-%m-%d", date.trim()).ok()?))
}

/// The change on a `- ` line: `- **Title.** Text` or `- Text`.
fn entry(line: &str) -> Option<Entry<'_>> {
  let rest = line.strip_prefix("- ")?.trim();
  let (title, text) = match rest.strip_prefix("**").and_then(|bold| bold.split_once("**")) {
    Some((title, text)) if !title.trim().is_empty() => (Some(title.trim()), text.trim()),
    _ => (None, rest),
  };
  (title.is_some() || !text.is_empty()).then_some(Entry { title, text })
}

/// The releases of a changelog, newest first as the file lists them. Lines that do not fit its shape are skipped: text
/// before the first release, a `## ` heading that is not `## <version> - <YYYY-MM-DD>` (with the changes under it),
/// and anything that is not a `- ` change. A release without a change is left out.
pub fn parse(text: &str) -> Vec<Release<'_>> {
  let mut releases: Vec<Release<'_>> = Vec::new();
  // Whether the changes read now belong to the last release: a heading that does not parse ends it.
  let mut open = false;
  for line in text.lines() {
    let line = line.trim_end();
    if line.starts_with("## ") {
      open = match heading(line) {
        Some((version, date)) => {
          releases.push(Release { version, date, entries: Vec::new() });
          true
        },
        None => false,
      };
    } else if open && let (Some(entry), Some(release)) = (entry(line), releases.last_mut()) {
      release.entries.push(entry);
    }
  }
  releases.retain(|release| !release.entries.is_empty());
  releases
}

/// The baked changelog's releases.
pub fn releases() -> Vec<Release<'static>> {
  parse(CHANGELOG)
}

/// A release's day as the dialog writes it: `Nov 24`, with the year when it is not `today`'s.
fn date_label(date: Date, today: Date) -> String {
  if date.year() == today.year() {
    date.strftime("%b %-d").to_string()
  } else {
    date.strftime("%b %-d, %Y").to_string()
  }
}

/// What start-up does about What's New.
#[derive(Debug, PartialEq, Eq)]
pub struct Startup {
  /// Open the dialog once the main window is up.
  pub show: bool,
  /// Store the running version as the one seen (`Settings::changelog_seen`).
  pub record: bool,
}

/// What start-up does, given the version whose changelog was last seen (`None` on a fresh install), the running
/// version, and whether the changelog has a section for it. A fresh install records the running version and shows
/// nothing; an update records it and shows the dialog when there is something to show; the same version does nothing.
pub fn startup(seen: Option<&str>, running: &str, has_section: bool) -> Startup {
  match seen {
    None => Startup { show: false, record: true },
    Some(seen) if seen == running => Startup { show: false, record: false },
    Some(_) => Startup { show: has_section, record: true },
  }
}

/// Runs the start-up decision on the loaded settings, storing the running version when it says so. Returns whether
/// to open the dialog once the main window is up.
pub fn after_update(cx: &mut App) -> bool {
  let seen = cx.global::<Settings>().changelog_seen.clone();
  let has_section = releases().iter().any(|release| release.version == RUNNING);
  let Startup { show, record } = startup(seen.as_deref(), RUNNING, has_section);
  if record {
    Settings::update(cx, |settings| settings.changelog_seen = Some(RUNNING.into()));
  }
  show
}

pub enum WhatsNewEvent {
  Close,
}

/// The What's New dialog: every release of the changelog, newest first, over the main window.
pub struct WhatsNew {
  focus: FocusHandle,
  /// What had the keyboard before it opened, to give it back on close.
  pub previous_focus: Option<FocusHandle>,
  /// The app's mark, for the dark look and for the light one.
  logo: [Arc<Image>; 2],
  releases: Vec<Release<'static>>,
}

impl EventEmitter<WhatsNewEvent> for WhatsNew {}

impl WhatsNew {
  pub fn new(logo: [Arc<Image>; 2], window: &mut Window, cx: &mut Context<Self>) -> Self {
    let previous_focus = window.focused(cx);
    let focus = cx.focus_handle();
    window.focus(&focus, cx);
    Self {
      focus,
      previous_focus,
      logo,
      releases: releases(),
    }
  }

  /// One release: its version and date, then its changes as a bulleted list. A rule sets it off from the release
  /// above it.
  fn release(release: &Release<'static>, first: bool, today: Date) -> Div {
    let line = h_flex()
      .gap_2()
      .items_baseline()
      .child(
        div()
          .text_size(px(15.))
          .font_weight(FontWeight::SEMIBOLD)
          .text_color(rgb(TEXT))
          .child(release.version),
      )
      .child(
        div()
          .text_size(px(12.))
          .text_color(rgb(MUTED))
          .child(date_label(release.date, today)),
      );
    let changes = release.entries.iter().map(|entry| {
      h_flex()
        .items_start()
        .gap_2()
        .text_size(px(13.))
        .text_color(rgb(TEXT))
        .child(div().flex_shrink_0().text_color(rgb(MUTED)).child("•"))
        .child(
          v_flex()
            .flex_1()
            .min_w_0()
            .children(entry.title.map(|title| div().font_weight(FontWeight::SEMIBOLD).child(title)))
            .when(!entry.text.is_empty(), |change| change.child(div().child(entry.text))),
        )
    });
    v_flex()
      .gap_3()
      .when(!first, |release| release.pt_5().border_t_1().border_color(rgb(BORDER)))
      .child(line)
      .children(changes)
  }
}

impl Render for WhatsNew {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let (width, height) = crate::conversation::modal_size(window.viewport_size(), SIZE, (0.45, 0.8));
    let today = crate::calendar_layout::today();
    div()
      .absolute()
      .top_0()
      .left_0()
      .size_full()
      .flex()
      .items_center()
      .justify_center()
      .bg(rgba(0x0000_0073))
      .occlude()
      .key_context("WhatsNew")
      .on_action(cx.listener(|_, _: &Escape, _, cx| {
        cx.emit(WhatsNewEvent::Close);
        cx.stop_propagation();
      }))
      // A press outside closes, and stays out of what is under it.
      .on_mouse_down(
        MouseButton::Left,
        cx.listener(|_, _, _, cx| {
          cx.emit(WhatsNewEvent::Close);
          cx.stop_propagation();
        }),
      )
      .child(
        v_flex()
          .w(width)
          .h(height)
          .rounded(px(14.))
          .bg(rgb(POPOVER))
          .border_1()
          .border_color(rgb(BORDER))
          .shadow_lg()
          .overflow_hidden()
          .text_color(rgb(TEXT))
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .child(
            h_flex()
              .h(px(56.))
              .pl_4()
              .pr_2()
              .gap_3()
              .flex_shrink_0()
              .items_center()
              .border_b_1()
              .border_color(rgb(BORDER))
              .child(img(self.logo[usize::from(is_light())].clone()).size(px(28.)))
              .child(
                div()
                  .flex_1()
                  .text_size(px(16.))
                  .font_weight(FontWeight::SEMIBOLD)
                  .child("What's new in Working"),
              )
              .child(
                div()
                  .id("whats-new-close")
                  .size(px(30.))
                  .flex()
                  .items_center()
                  .justify_center()
                  .rounded_full()
                  .cursor_pointer()
                  .hover(|style| style.bg(rgb(ROW_HOVER)))
                  .on_click(cx.listener(|_, _, _, cx| cx.emit(WhatsNewEvent::Close)))
                  .child(icon(Icon::X, 16., TEXT)),
              ),
          )
          .child(
            v_flex()
              .id("whats-new-releases")
              .flex_1()
              .min_h_0()
              .p_4()
              .gap_5()
              .overflow_y_scroll()
              .children(
                self
                  .releases
                  .iter()
                  .enumerate()
                  .map(|(index, release)| Self::release(release, index == 0, today)),
              ),
          )
          .child(
            h_flex()
              .h(px(56.))
              .px_4()
              .flex_shrink_0()
              .items_center()
              .justify_end()
              .border_t_1()
              .border_color(rgb(BORDER))
              .child(
                primary_button("whats-new-done", "Got it", true)
                  .on_click(cx.listener(|_, _, _, cx| cx.emit(WhatsNewEvent::Close))),
              ),
          ),
      )
      .focus_trap("whats-new", &self.focus)
  }
}

#[cfg(test)]
mod tests {
  use super::{Entry, Startup, date_label, parse, releases, startup};
  use jiff::civil::date;

  const SAMPLE: &str = "# Changelog

Intro text that is not a release.
- A stray line before any release.

## 0.3.0 - 2026-11-24

- **Mentions work.** Jordan Park sees an @ beside the chat.
- Riley Brooks can pin a space.
-
- **** Empty bold reads as plain text.
not a change
  - indented line

## not a release

- Belongs to the broken heading.

## 0.2.0 - 2025-02-30

- Bad date.

## 0.1.0 - 2025-03-01

- **Only a title.**
";

  #[test]
  fn changelog_parses_releases_and_skips_junk() {
    let releases = parse(SAMPLE);
    let versions: Vec<_> = releases.iter().map(|release| release.version).collect();
    assert_eq!(versions, ["0.3.0", "0.1.0"]);
    let newest = releases.first().unwrap();
    assert_eq!(newest.date, date(2026, 11, 24));
    assert_eq!(
      newest.entries,
      [
        Entry {
          title: Some("Mentions work."),
          text: "Jordan Park sees an @ beside the chat.",
        },
        Entry {
          title: None,
          text: "Riley Brooks can pin a space."
        },
        Entry {
          title: None,
          text: "**** Empty bold reads as plain text."
        },
      ]
    );
    assert_eq!(
      releases.last().unwrap().entries,
      [Entry { title: Some("Only a title."), text: "" }]
    );
  }

  #[test]
  fn changelog_baked_file_parses() {
    let releases = releases();
    assert!(releases.first().is_some_and(|release| !release.entries.is_empty()));
  }

  #[test]
  fn changelog_dates_add_the_year_only_when_it_is_another() {
    let today = date(2026, 11, 24);
    assert_eq!(date_label(date(2026, 11, 24), today), "Nov 24");
    assert_eq!(date_label(date(2026, 3, 5), today), "Mar 5");
    assert_eq!(date_label(date(2025, 12, 31), today), "Dec 31, 2025");
  }

  #[test]
  fn changelog_shows_once_after_an_update() {
    // A fresh install records the version and shows nothing.
    assert_eq!(startup(None, "0.2.0", true), Startup { show: false, record: true });
    // The same version again does nothing.
    assert_eq!(startup(Some("0.2.0"), "0.2.0", true), Startup { show: false, record: false });
    // An update shows the dialog when its version has a section, and records it either way.
    assert_eq!(startup(Some("0.1.0"), "0.2.0", true), Startup { show: true, record: true });
    assert_eq!(startup(Some("0.1.0"), "0.2.0", false), Startup { show: false, record: true });
  }
}
