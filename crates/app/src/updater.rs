//! Self-update. Release builds check GitHub for a newer version, get it ready, and show a restart card until the
//! user restarts. macOS installs the new bundle over the running one, Linux the new `AppImage` over the running one
//! (only an `AppImage` updates itself: a package manager owns any other install), and Windows keeps the verified NSIS
//! installer for the restart, which runs it. There is no setting: every release build updates itself. Local builds
//! (`just run`) never update, because `WORKING_UPDATES` is set only by the release workflow.

#[cfg(not(target_os = "windows"))]
use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::sync::Arc;
use std::time::Duration;

#[cfg(target_os = "windows")]
use cargo_packager_updater::Update;

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

/// What the restart finishes the update with: the new bundle (macOS) or `AppImage` (Linux), already installed over the
/// running one.
#[cfg(not(target_os = "windows"))]
type Pending = PathBuf;

/// What the restart finishes the update with: the update and its verified installer, which replaces the running app
/// and so waits for the restart.
#[cfg(target_os = "windows")]
type Pending = Arc<(Update, Vec<u8>)>;

/// An update waiting for a restart.
#[derive(Clone)]
pub struct Ready {
  pub version: String,
  pending: Pending,
}

/// Whether an update waits for a restart.
#[derive(Default)]
pub struct Updater(pub Option<Ready>);

impl Global for Updater {}

fn enabled() -> bool {
  let installable = !cfg!(target_os = "linux") || std::env::var_os("APPIMAGE").is_some();
  option_env!("WORKING_UPDATES") == Some("1") && installable
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

/// Checks the manifest and, when a newer version exists, downloads and verifies it, and installs it where the running
/// app can be replaced (macOS, Linux).
fn check_and_install() -> Result<Option<Ready>, String> {
  let current: Version = env!("CARGO_PKG_VERSION").parse().map_err(|err| format!("{err}"))?;
  let config = Config {
    endpoints: vec![ENDPOINT.parse().map_err(|err| format!("{err}"))?],
    pubkey: PUBKEY.into(),
    // The default: Windows runs the NSIS installer passively (`/P /R`), which closes the app, installs, and opens it
    // again.
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
  #[cfg(not(target_os = "windows"))]
  {
    update.download_and_install().map_err(|err| err.to_string())?;
    Ok(Some(Ready {
      version: update.version.clone(),
      pending: update.extract_path,
    }))
  }
  // Installing runs the installer and exits this process at once, so only the download happens now.
  #[cfg(target_os = "windows")]
  {
    let bytes = update.download().map_err(|err| err.to_string())?;
    Ok(Some(Ready {
      version: update.version.clone(),
      pending: Arc::new((update, bytes)),
    }))
  }
}

/// Opens the new bundle once this process has exited, then quits.
#[cfg(target_os = "macos")]
fn restart(bundle: &Path, cx: &mut App) {
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

/// Runs the new `AppImage` once this process has exited, then quits. `$APPIMAGE`, not the running executable, which sits
/// in the old image's temporary mount.
#[cfg(target_os = "linux")]
fn restart(appimage: &Path, cx: &mut App) {
  cx.set_restart_path(appimage.to_path_buf());
  cx.restart();
}

/// Runs the installer, which closes this process, installs, and opens the new version. Off the main thread: starting a
/// process on Windows can pump the window messages, which re-enters GPUI.
#[cfg(target_os = "windows")]
fn restart(installer: &Pending, cx: &mut App) {
  let installer = installer.clone();
  cx.background_executor()
    .spawn(async move {
      let (update, bytes) = &*installer;
      if let Err(err) = update.install(bytes.clone()) {
        tracing::warn!("restart failed: {err}");
      }
    })
    .detach();
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
          .on_click(move |_, _, cx| restart(&pending, cx))
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
