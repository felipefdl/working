//! Whether the OS shows clock times in 12-hour or 24-hour form.
//!
//! Each OS has a file of its own (`macos.rs`, `linux.rs`, `windows.rs`) that reads the user's preference, so callers
//! never name an OS. Only the 12/24 choice comes from here: each screen formats its own times. The answer is read once
//! per run and kept, so render code may ask freely; a preference changed while the app runs shows after a restart.
//! Where the preference cannot be read, the answer is 12-hour. `warm` starts the read as the app starts, so the first
//! frame does not wait for it.

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

use std::sync::LazyLock;

/// The OS's answer, read on first use.
static IS_24_HOUR: LazyLock<bool> = LazyLock::new(os::is_24_hour);

/// Whether the OS shows clock times in 24-hour form. Read once per run.
pub fn is_24_hour() -> bool {
  *IS_24_HOUR
}

/// Starts the read on a thread of its own, at startup: Linux may wait on the settings portal, and by the time the
/// first window draws the answer is there. Asking before it is waits only for what is left of the read.
pub fn warm() {
  let spawned = std::thread::Builder::new().name("clock-warm".into()).spawn(|| {
    LazyLock::force(&IS_24_HOUR);
  });
  if let Err(error) = spawned {
    tracing::warn!("could not read the clock format ahead of the first frame: {error}");
  }
}
