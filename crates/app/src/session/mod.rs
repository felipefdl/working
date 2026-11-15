//! The Google session in a webview's cookie store: read it out after a sign-in or during a call, and put the saved
//! one in before a page loads.
//!
//! Each OS has a file of its own (`macos.rs`, `linux.rs`, `windows.rs`) with the functions below, which only pass the
//! call on, so callers never name an OS. Both run on the main thread, inside a GPUI update, and answer through a
//! channel: an implementation must not wait by spinning the platform's event loop there, which re-enters GPUI while
//! the app is borrowed. Session cookies are domain cookies (`.google.com`): an implementation keeps the leading dot
//! both ways, or `meet.google.com` never receives them.

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

use gchat_core::SessionCookie;

/// Reads the session's cookies as the webview holds them now: those of `.google.com`, `chat.google.com`, and
/// `accounts.google.com`.
pub fn read(web: &wry::WebView) -> flume::Receiver<Vec<SessionCookie>> {
  os::read(web)
}

/// Puts the saved session's cookies (`gchat_core::session_cookies`) into the webview. The returned channel gets a
/// message once all of them are stored, and only then may the first page load. Only the call window needs it, and
/// Linux has none.
#[cfg(not(target_os = "linux"))]
pub fn install(web: &wry::WebView) -> flume::Receiver<()> {
  os::install(web)
}
