//! Self-update. Release builds check GitHub for a newer version, get it ready, and show a restart card until the
//! user restarts. macOS installs the new bundle over the running one, Linux the new `AppImage` over the running one
//! (only an `AppImage` updates itself: a package manager owns any other install), and Windows keeps the verified NSIS
//! installer for the restart, which runs it. There is no setting: every release build updates itself. Local builds
//! (`just run`) never update, because `WORKING_UPDATES` is set only by the release workflow.
//!
//! Each OS has a file of its own (`macos.rs`, `linux.rs`, `windows.rs`) for what differs: whether this copy can
//! update, how the downloaded update is installed, how the restart finishes it, and what a previous update left.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
use linux as os;
#[cfg(target_os = "macos")]
use macos as os;
#[cfg(target_os = "windows")]
use windows as os;

use std::time::Duration;

use cargo_packager_updater::{Config, UpdaterBuilder, WindowsConfig, semver::Version};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;

use crate::theme::{ACCENT, ACCENT_BG, BORDER, MUTED, POPOVER, TEXT, rgb};

/// Manifest the release workflow attaches to every GitHub release.
const ENDPOINT: &str = "https://github.com/felipefdl/working/releases/latest/download/latest.json";
/// Minisign public key that verifies the downloaded bundle.
const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDAwNzZBMjM5RjZBODM2MDkKUldRSk5xajJPYUoyQUZmRC9QMHYzTDdyalhqNXNsZmF5RThLUGRLbVpGbUJLU0JsTDJhYTgyejQK";

/// After launch, the app settles before the first check.
const FIRST_CHECK: Duration = Duration::from_secs(15);
/// A long-running app keeps checking at this interval.
const INTERVAL: Duration = Duration::from_hours(4);
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(10);

/// An update waiting for a restart.
#[derive(Clone)]
pub struct Ready {
  pub version: String,
  pending: os::Pending,
}

/// Whether an update waits for a restart.
#[derive(Default)]
pub struct Updater(pub Option<Ready>);

impl Global for Updater {}

/// What one check came to.
enum Outcome {
  /// A newer version is installed or kept for the restart.
  Ready(Ready),
  /// Nothing newer is published.
  Current,
  /// This copy cannot be replaced now, so nothing was downloaded; the reason says why.
  Blocked(String),
}

fn enabled() -> bool {
  option_env!("WORKING_UPDATES") == Some("1") && os::installable()
}

/// Starts the background check loop in release builds. It stops once an update is installed.
pub fn start(cx: &mut App) {
  cx.set_global(Updater::default());
  if !enabled() {
    return;
  }
  cx.spawn(async move |cx| {
    cx.background_executor().timer(FIRST_CHECK).await;
    // What the previous update left; whatever ran it has exited by now.
    cx.background_executor().spawn(async { os::clean() }).await;
    let mut told = false;
    loop {
      let result = cx.background_executor().spawn(async { check_and_install() }).await;
      match result {
        Ok(Outcome::Ready(ready)) => {
          cx.update(|cx| {
            cx.set_global(Updater(Some(ready)));
            cx.refresh_windows();
          });
          return;
        },
        Ok(Outcome::Current) => {},
        // Once per run: the reason holds until the user changes something, and the loop keeps trying.
        Ok(Outcome::Blocked(reason)) => {
          if !told {
            tracing::warn!("update skipped: {reason}");
            told = true;
          }
        },
        Err(err) => tracing::warn!("update failed: {err}"),
      }
      cx.background_executor().timer(INTERVAL).await;
    }
  })
  .detach();
}

/// Checks the manifest and, when a newer version exists, downloads and verifies it, and installs it where the running
/// app can be replaced (macOS, Linux).
fn check_and_install() -> Result<Outcome, String> {
  if let Some(reason) = os::blocked() {
    return Ok(Outcome::Blocked(reason));
  }
  let current: Version = env!("CARGO_PKG_VERSION").parse().map_err(|err| format!("{err}"))?;
  let config = Config {
    endpoints: vec![ENDPOINT.parse().map_err(|err| format!("{err}"))?],
    pubkey: PUBKEY.into(),
    // Windows runs the NSIS installer passively (`/P /R`, the default mode), which closes the app, installs, and opens
    // it again. `/NS` keeps it from putting back the Desktop shortcut a user may have deleted; the Start Menu
    // shortcut, which carries the app's notification identity, is written either way.
    windows: Some(WindowsConfig {
      installer_args: Some(vec!["/NS".into()]),
      install_mode: None,
    }),
  };
  let updater = UpdaterBuilder::new(current, config)
    .timeout(CHECK_TIMEOUT)
    .build()
    .map_err(|err| err.to_string())?;
  let Some(mut update) = updater.check().map_err(|err| err.to_string())? else {
    return Ok(Outcome::Current);
  };
  update.timeout = Some(DOWNLOAD_TIMEOUT);
  let version = update.version.clone();
  let pending = os::install(update)?;
  Ok(Outcome::Ready(Ready { version, pending }))
}

/// The persistent card shown once an update is ready; the window root puts it in the bottom-right corner.
pub fn card(cx: &App) -> Option<AnyElement> {
  let ready = cx.try_global::<Updater>()?.0.clone()?;
  let pending = ready.pending.clone();
  Some(
    h_flex()
      .id("update-card")
      .w(px(300.))
      .p_3()
      .gap_3()
      .items_center()
      .rounded(px(12.))
      .border_1()
      .border_color(rgb(BORDER))
      .bg(rgb(POPOVER))
      .shadow_lg()
      .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
      .child(
        v_flex()
          .flex_1()
          .min_w_0()
          .gap_0p5()
          .child(
            div()
              .text_size(px(13.))
              .font_weight(FontWeight::SEMIBOLD)
              .text_color(rgb(TEXT))
              .child(format!("Working {} is ready", ready.version)),
          )
          .child(
            div()
              .text_size(px(12.))
              .text_color(rgb(MUTED))
              .child("Restart to finish updating."),
          ),
      )
      .child(
        div()
          .id("update-restart")
          .h(px(28.))
          .px_3()
          .flex()
          .flex_shrink_0()
          .items_center()
          .rounded(px(8.))
          .bg(rgb(ACCENT_BG))
          .text_size(px(13.))
          .text_color(rgb(ACCENT))
          .cursor_pointer()
          .hover(|style| style.opacity(0.85))
          .on_click(move |_, _, cx| os::restart(&pending, cx))
          .child("Restart"),
      )
      .into_any_element(),
  )
}

#[cfg(test)]
mod tests {
  use super::{ENDPOINT, PUBKEY, Version};
  use base64::Engine;

  #[test]
  fn endpoint_and_key_are_well_formed() {
    assert!(ENDPOINT.starts_with("https://github.com/felipefdl/working/releases/latest/download/"));
    let key = base64::engine::general_purpose::STANDARD.decode(PUBKEY).unwrap_or_default();
    assert!(String::from_utf8_lossy(&key).contains("minisign public key"));
  }

  #[test]
  fn package_version_is_semver() {
    assert!(env!("CARGO_PKG_VERSION").parse::<Version>().is_ok());
  }
}
