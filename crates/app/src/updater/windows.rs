//! Windows: the verified NSIS installer waits in memory for the restart, which writes it to the app's own updates
//! folder and runs it; the next launch removes it.

use std::io::ErrorKind;
use std::sync::Arc;

use cargo_packager_updater::Update;
use gpui_kit::App;

/// The update and its verified installer, which replaces the running app and so waits for the restart.
pub type Pending = Arc<(Update, Vec<u8>)>;

/// Every release build updates itself.
pub const fn installable() -> bool {
  true
}

/// The per-user install is always replaceable.
pub const fn blocked() -> Option<String> {
  None
}

/// Removes the installer the previous update ran, and its folder: only [`run`] writes there.
pub fn clean() {
  let folder = gchat_core::updates_dir();
  match std::fs::remove_dir_all(&folder) {
    Ok(()) => tracing::info!("removed the previous update's installer from {}", folder.display()),
    Err(err) if err.kind() == ErrorKind::NotFound => {},
    // Still running, or held by a scanner: the next launch tries again.
    Err(err) => tracing::warn!("could not remove {}: {err}", folder.display()),
  }
}

/// Downloads and verifies the installer. Installing runs it and exits this process at once, so that waits for the
/// restart.
pub fn install(update: Update) -> Result<Pending, String> {
  let bytes = update.download().map_err(|err| err.to_string())?;
  Ok(Arc::new((update, bytes)))
}

/// Runs the installer, which closes this process, installs, and opens the new version. Off the main thread: starting a
/// process on Windows can pump the window messages, which re-enters GPUI.
pub fn restart(installer: &Pending, cx: &mut App) {
  let installer = installer.clone();
  cx.background_executor()
    .spawn(async move {
      let (update, bytes) = &*installer;
      if let Err(err) = run(update, bytes) {
        tracing::warn!("restart failed: {err}");
      }
    })
    .detach();
}

/// Writes the installer into [`gchat_core::updates_dir`] and runs it; on success this process exits here.
///
/// cargo-packager-updater writes the installer into `tempfile`'s default temporary folder and keeps it, so this points
/// that folder, for this process, at the app's own updates folder, which [`clean`] empties on the next launch
/// (`docs/workarounds.md`). The process exits right after, so nothing else lands there.
fn run(update: &Update, bytes: &[u8]) -> Result<(), String> {
  let folder = gchat_core::updates_dir();
  std::fs::create_dir_all(&folder).map_err(|err| format!("{}: {err}", folder.display()))?;
  // A second Restart, after a failed one, finds the folder already set.
  if let Err(set) = tempfile::env::override_temp_dir(&folder)
    && set != folder
  {
    return Err(format!("the temporary folder is already {}", set.display()));
  }
  update.install(bytes.to_vec()).map_err(|err| err.to_string())
}
