//! Linux: only an `AppImage` updates itself. The new image is written over `$APPIMAGE` at once, and the restart runs
//! it.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use cargo_packager_updater::Update;
use gpui_kit::App;

/// The new `AppImage`, already written over `$APPIMAGE`.
pub type Pending = PathBuf;

/// Waits for the old process to exit, then runs the new `AppImage` in its place. It first closes every descriptor
/// above 2 it inherited: the `AppImage` runtime of the old image keeps its mount until every copy of its keepalive
/// pipe closes, and that pipe is open in this process without close-on-exec.
const RELAUNCH: &str = r#"
for fd in /proc/$$/fd/*; do
  fd=${fd##*/}
  if [ "$fd" -gt 2 ]; then eval "exec $fd>&-" 2>/dev/null; fi
done
while kill -0 "$1" 2>/dev/null; do sleep 0.1; done
exec "$2"
"#;

/// A package manager owns any install that is not an `AppImage`.
pub fn installable() -> bool {
  std::env::var_os("APPIMAGE").is_some()
}

/// Why the `AppImage` cannot be replaced: the updater moves `$APPIMAGE` aside and writes the new image in its place,
/// which takes write access to its folder. Checked before anything is downloaded.
pub fn blocked() -> Option<String> {
  let appimage = PathBuf::from(std::env::var_os("APPIMAGE")?);
  let folder = appimage.parent()?;
  (!writable(folder)).then(|| format!("{} is not writable, so the AppImage there cannot be replaced", folder.display()))
}

/// Whether a file can be made in `folder`: the true answer, ACLs and read-only mounts included, which the mode bits
/// alone do not give.
fn writable(folder: &Path) -> bool {
  let probe = folder.join(format!(".working-update-probe-{}", std::process::id()));
  let made = OpenOptions::new().write(true).create_new(true).open(&probe).is_ok();
  if made && let Err(err) = std::fs::remove_file(&probe) {
    tracing::warn!("could not remove {}: {err}", probe.display());
  }
  made
}

/// The replaced image goes away with its temporary folder; nothing is left for a later launch.
pub const fn clean() {}

/// Downloads, verifies, and writes the new `AppImage` over `$APPIMAGE`.
pub fn install(update: Update) -> Result<Pending, String> {
  update.download_and_install().map_err(|err| err.to_string())?;
  Ok(update.extract_path)
}

/// Runs the new `AppImage` once this process has exited, then quits. `$APPIMAGE`, not the running executable, which
/// sits in the old image's temporary mount. GPUI's own restart would leave the old mount up while the new app runs
/// (`docs/workarounds.md`).
pub fn restart(appimage: &Path, cx: &mut App) {
  use std::os::unix::process::CommandExt as _;

  let spawned = std::process::Command::new("/usr/bin/env")
    .arg("bash")
    .arg("-c")
    .arg(RELAUNCH)
    .arg("bash")
    .arg(std::process::id().to_string())
    .arg(appimage)
    .process_group(0)
    .spawn();
  match spawned {
    Ok(_) => cx.quit(),
    Err(err) => tracing::warn!("restart failed: {err}"),
  }
}

#[cfg(test)]
mod tests {
  use std::os::unix::fs::PermissionsExt;
  use std::path::PathBuf;

  use super::writable;

  fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("working-updater-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
  }

  #[test]
  fn a_folder_without_write_access_is_not_writable() {
    let open = scratch("open");
    assert!(writable(&open));
    assert_eq!(std::fs::read_dir(&open).unwrap().count(), 0, "the probe is removed");

    let locked = scratch("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let result = writable(&locked);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_dir_all(&locked).unwrap();
    std::fs::remove_dir_all(&open).unwrap();
    // Root writes anywhere, so the answer only means something for a regular user.
    if !running_as_root() {
      assert!(!result);
    }
  }

  fn running_as_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
      .ok()
      .and_then(|status| status.lines().find(|line| line.starts_with("Uid:")).map(str::to_owned))
      .is_some_and(|line| line.split_whitespace().nth(1) == Some("0"))
  }
}
