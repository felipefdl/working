//! Where the app keeps its files on each OS: one place, which the app reuses.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Directory holding `cookies.json`, `settings.json`, and `mcp-token`: `~/.config/working` on macOS,
/// `$XDG_CONFIG_HOME/working` on Linux (`~/.config/working` by default), `%APPDATA%\working` on Windows.
///
/// In demo mode, a directory of its own under the temp dir, so the demo never reads the session or changes
/// the settings of a real run.
pub fn config_dir() -> PathBuf {
  if crate::demo_mode() {
    return std::env::temp_dir().join("working-demo");
  }
  // macOS keeps the dotfile directory rather than `~/Library/Application Support`, where `dirs` points.
  #[cfg(target_os = "macos")]
  let base = dirs::home_dir().map(|home| home.join(".config"));
  #[cfg(not(target_os = "macos"))]
  let base = dirs::config_dir();
  base.unwrap_or_default().join("working")
}

/// The disk cache: `~/Library/Caches/working` on macOS, `$XDG_CACHE_HOME/working` on Linux (`~/.cache/working`
/// by default), `%LOCALAPPDATA%\dev.felipefdl.working\cache` on Windows.
pub fn cache_dir() -> PathBuf {
  #[cfg(not(target_os = "windows"))]
  let (base, dir) = (dirs::cache_dir().unwrap_or_default(), "working");
  #[cfg(target_os = "windows")]
  let (base, dir) = (local_dir(), "cache");
  base.join(dir)
}

/// The app's machine-local data on Windows, `%LOCALAPPDATA%\dev.felipefdl.working`: apart from the per-user install
/// in `%LOCALAPPDATA%\Working`, which the installer and its uninstaller own.
#[cfg(target_os = "windows")]
fn local_dir() -> PathBuf {
  dirs::data_local_dir().unwrap_or_default().join("dev.felipefdl.working")
}

/// Where the updater writes the downloaded installer, which runs from there at restart:
/// `%LOCALAPPDATA%\dev.felipefdl.working\updates`. Only the updater writes there, so the next launch clears it.
#[cfg(target_os = "windows")]
pub fn updates_dir() -> PathBuf {
  local_dir().join("updates")
}

/// The `WebView2` data folder (the webviews' cookies, cache, and storage):
/// `%LOCALAPPDATA%\dev.felipefdl.working\WebView2`.
///
/// In demo mode, one under the demo's own config directory, so the demo never shares a real run's web session.
#[cfg(target_os = "windows")]
pub fn webview_dir() -> PathBuf {
  if crate::demo_mode() {
    return config_dir().join("WebView2");
  }
  local_dir().join("WebView2")
}

/// Creates or truncates `path` for writing, readable by the user alone where the OS has file modes (0600 on
/// macOS and Linux). On Windows the file takes the access list of the user's profile folder it is in.
pub(crate) fn create_private(path: &Path) -> io::Result<File> {
  let mut options = OpenOptions::new();
  options.write(true).create(true).truncate(true);
  #[cfg(unix)]
  {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
  }
  let file = options.open(path)?;
  // `mode` only applies to a file this call creates.
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
  }
  Ok(file)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn config_and_cache_are_the_apps_own() {
    assert!(config_dir().ends_with("working"));
    #[cfg(target_os = "macos")]
    {
      let home = dirs::home_dir().unwrap();
      assert_eq!(config_dir(), home.join(".config/working"));
      assert_eq!(cache_dir(), home.join("Library/Caches/working"));
    }
    #[cfg(target_os = "linux")]
    assert!(cache_dir().ends_with("working"));
    #[cfg(target_os = "windows")]
    {
      let local = dirs::data_local_dir().unwrap().join("dev.felipefdl.working");
      assert_eq!(cache_dir(), local.join("cache"));
      assert_eq!(webview_dir(), local.join("WebView2"));
      assert_eq!(updates_dir(), local.join("updates"));
    }
  }

  #[cfg(unix)]
  #[test]
  fn private_files_are_the_users_alone() {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::temp_dir().join(format!("working-private-{}", std::process::id()));
    std::fs::write(&path, "old").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    drop(create_private(&path).unwrap());
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    std::fs::remove_file(&path).unwrap();
    assert_eq!(mode, 0o600);
  }
}
