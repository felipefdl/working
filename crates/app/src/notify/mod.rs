//! Desktop notifications for incoming messages, and the unread count on the app icon.
//!
//! Each OS has a file of its own (`macos.rs`, `linux.rs`, `windows.rs`) with the functions below, which only pass
//! the call on, so callers never name an OS. Each notification carries its conversation's key: where the OS groups
//! notifications, it groups a conversation's by it, and a click hands it back so the app opens that conversation.
//! Where the OS offers no notifications to this process (macOS gives a notification center to the app bundle only),
//! every function does nothing and the click receiver closes at once.

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

use crate::settings::Sound;

/// Gives the process the app's `AppUserModelID`, the one the installer's Start Menu shortcut carries: toasts show and
/// group under it. Windows reads it when a window first shows, so `main` calls it before any.
#[cfg(target_os = "windows")]
pub use windows::set_app_id;

/// Starts listening for clicks, and asks for permission where the OS asks (macOS prompts once). The receiver yields
/// the conversation key of each clicked notification.
pub fn start() -> flume::Receiver<String> {
  os::start()
}

/// Asks the OS to allow notifications; a prompt only shows the first time. Only the macOS Settings permissions group
/// calls it.
#[cfg_attr(
  not(target_os = "macos"),
  expect(dead_code, reason = "only the macOS Settings permissions group asks")
)]
pub fn request() {
  os::request();
}

/// Whether the user allows notifications; `None` while the OS has not asked yet. The channel closes without an
/// answer when the OS keeps no such permission or notifications are unavailable. Only the macOS Settings
/// permissions group reads it.
#[cfg_attr(
  not(target_os = "macos"),
  expect(dead_code, reason = "only the macOS Settings permissions group asks")
)]
pub fn authorization() -> flume::Receiver<Option<bool>> {
  os::authorization()
}

/// Posts a notification. `id` is the message id, so an edit replaces what is shown; `thread` is the conversation
/// key. `sound` is one of this OS's [`Sound::ALL`].
pub fn post(id: &str, thread: &str, title: &str, subtitle: Option<&str>, body: &str, sound: Sound) {
  os::post(id, thread, title, subtitle, body, sound);
}

/// Takes delivered notifications away, by the ids they were posted with.
pub fn clear(ids: &[String]) {
  os::clear(ids);
}

/// Shows the unread count on the app icon; zero clears it. The Dock badge on macOS; elsewhere what the platform
/// offers for it, or nothing where it offers nothing.
pub fn set_badge(count: usize) {
  os::set_badge(count);
}
