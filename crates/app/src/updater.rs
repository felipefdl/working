//! Self-update. Release builds check GitHub for a newer version, install it over the running bundle, and show
//! a restart card until the user restarts. There is no setting: every release build updates itself. Local
//! builds (`just run`) never update, because `WORKING_UPDATES` is set only by the release workflow.

use std::path::PathBuf;
use std::time::Duration;

use cargo_packager_updater::{Config, UpdaterBuilder, semver::Version};
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

/// An update installed over the bundle, waiting for a restart.
#[derive(Clone)]
pub struct Ready {
  pub version: String,
  pub bundle: PathBuf,
}

/// Whether an installed update waits for a restart.
#[derive(Default)]
pub struct Updater(pub Option<Ready>);

impl Global for Updater {}

fn enabled() -> bool {
  option_env!("WORKING_UPDATES") == Some("1")
}

/// Starts the background check loop in release builds. It stops once an update is installed.
pub fn start(cx: &mut App) {
  cx.set_global(Updater::default());
  if !enabled() {
    return;
  }
  cx.spawn(async move |cx| {
    cx.background_executor().timer(FIRST_CHECK).await;
    loop {
      let result = cx.background_executor().spawn(async { check_and_install() }).await;
      match result {
        Ok(Some(ready)) => {
          cx.update(|cx| {
            cx.set_global(Updater(Some(ready)));
            cx.refresh_windows();
          });
          return;
        },
        Ok(None) => {},
        Err(err) => tracing::warn!("update failed: {err}"),
      }
      cx.background_executor().timer(INTERVAL).await;
    }
  })
  .detach();
}

/// Checks the manifest and, when a newer version exists, downloads, verifies, and installs it.
fn check_and_install() -> Result<Option<Ready>, String> {
  let current: Version = env!("CARGO_PKG_VERSION").parse().map_err(|err| format!("{err}"))?;
  let config = Config {
    endpoints: vec![ENDPOINT.parse().map_err(|err| format!("{err}"))?],
    pubkey: PUBKEY.into(),
    windows: None,
  };
  let updater = UpdaterBuilder::new(current, config)
    .timeout(CHECK_TIMEOUT)
    .build()
    .map_err(|err| err.to_string())?;
  let Some(mut update) = updater.check().map_err(|err| err.to_string())? else {
    return Ok(None);
  };
  update.timeout = Some(DOWNLOAD_TIMEOUT);
  update.download_and_install().map_err(|err| err.to_string())?;
  Ok(Some(Ready {
    version: update.version.clone(),
    bundle: update.extract_path,
  }))
}

/// Opens the new bundle once this process has exited, then quits.
fn restart(bundle: &std::path::Path, cx: &mut App) {
  let spawned = std::process::Command::new("/bin/sh")
    .arg("-c")
    .arg(r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; open "$2""#)
    .arg("sh")
    .arg(std::process::id().to_string())
    .arg(bundle)
    .spawn();
  match spawned {
    Ok(_) => cx.quit(),
    Err(err) => tracing::warn!("restart failed: {err}"),
  }
}

/// The persistent card in the bottom-right corner of the main window, shown once an update is installed.
pub fn card(cx: &App) -> Option<AnyElement> {
  let ready = cx.try_global::<Updater>()?.0.clone()?;
  let bundle = ready.bundle.clone();
  Some(
    div()
      .absolute()
      .right(px(20.))
      .bottom(px(20.))
      .child(
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
              .on_click(move |_, _, cx| restart(&bundle, cx))
              .child("Restart"),
          ),
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
