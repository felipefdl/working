//! macOS: the new bundle is installed over the running one at once, and the restart opens it.

use std::path::{Path, PathBuf};

use cargo_packager_updater::Update;
use gpui_kit::App;

/// The new bundle, already installed over the running one.
pub type Pending = PathBuf;

/// Every release build updates itself.
pub const fn installable() -> bool {
  true
}

/// The bundle is always replaceable as far as the updater knows.
pub const fn blocked() -> Option<String> {
  None
}

/// Installing leaves nothing behind.
pub const fn clean() {}

/// Downloads, verifies, and installs the new bundle over the running one.
pub fn install(update: Update) -> Result<Pending, String> {
  update.download_and_install().map_err(|err| err.to_string())?;
  Ok(update.extract_path)
}

/// Opens the new bundle once this process has exited, then quits.
pub fn restart(bundle: &Path, cx: &mut App) {
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
