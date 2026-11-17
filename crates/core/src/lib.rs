//! Google Chat client core: session, protocol, realtime stream, and local cache, behind a command/event pair.
//!
//! The core owns a Tokio runtime on its own threads. A UI on any executor sends [`Command`]s and awaits
//! [`Event`]s; nothing here touches the UI thread.

mod cache;
mod calendar;
mod channel;
mod convert;
mod demo;
mod engine;
pub mod fuzzy;
mod http;
mod markup;
pub mod mcp;
pub mod model;
mod paths;
mod proto;

use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use calendar::{EVENT_COLORS, boundary_date, local_midnight};
pub use engine::device_time_zone;
pub use http::SessionCookie;
pub use model::*;
pub use paths::{cache_dir, config_dir};
#[cfg(target_os = "windows")]
pub use paths::{updates_dir, webview_dir};

/// True when `WORKING_DEMO=1`: the core serves a made-up workspace and never signs in.
pub fn demo_mode() -> bool {
  std::env::var("WORKING_DEMO").is_ok_and(|value| value == "1")
}

/// Session cookies for an embedded Meet browser.
///
/// Empty when signed out.
pub fn session_cookies() -> Vec<SessionCookie> {
  http::read_cookies(&config_dir().join("cookies.json")).unwrap_or_default()
}

/// One writer of the session file at a time: the chat side saves what Google refreshes from the core's threads,
/// and a Meet call saves its rotated cookies from the app's.
static SESSION_FILE: Mutex<()> = Mutex::new(());

/// Merges Meet-rotated session cookies into the saved file.
///
/// Google rotates cookies while a call is open. Cookies the browser does not hold are kept. Answers whether
/// the file changed.
pub fn save_session_cookies(current: Vec<SessionCookie>) -> io::Result<bool> {
  merge_session_cookies(&config_dir().join("cookies.json"), current)
}

/// Puts `current` into the session file at `path`: a cookie the file holds takes the new value, any other is
/// added, and what `current` does not name stays. Answers whether the file changed.
pub(crate) fn merge_session_cookies(path: &Path, current: Vec<SessionCookie>) -> io::Result<bool> {
  let _writing = SESSION_FILE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
  let saved = http::read_cookies(path).unwrap_or_default();
  let mut merged = saved.clone();
  for cookie in current {
    match merged.iter_mut().find(|held| http::same_cookie(held, &cookie)) {
      Some(held) => held.value = cookie.value,
      None => merged.push(cookie),
    }
  }
  if merged == saved {
    return Ok(false);
  }
  write_cookies(path, &merged)?;
  Ok(true)
}

/// Replaces the saved session after a fresh sign-in.
pub fn write_session_cookies(cookies: &[SessionCookie]) -> io::Result<()> {
  write_cookies(&config_dir().join("cookies.json"), cookies)
}

/// True when a session file exists (always, in demo mode). The core still has to accept it.
pub fn has_session() -> bool {
  demo_mode() || config_dir().join("cookies.json").is_file()
}

fn write_cookies(path: &Path, cookies: &[SessionCookie]) -> io::Result<()> {
  if let Some(dir) = path.parent() {
    std::fs::create_dir_all(dir)?;
  }
  let text = serde_json::to_string_pretty(cookies).map_err(io::Error::other)?;
  let staging = path.with_extension("json.new");
  paths::create_private(&staging)?.write_all(text.as_bytes())?;
  std::fs::rename(&staging, path)?;
  Ok(())
}

/// Handle to the running core. Cheap to clone.
#[derive(Clone)]
pub struct Core {
  commands: flume::Sender<Command>,
  events: flume::Receiver<Event>,
  fetcher: Arc<engine::Fetcher>,
  /// Ends the core's command loop (`shutdown`).
  stop: flume::Sender<()>,
  /// Disconnects once the core finished: its last cache writes are on disk.
  stopped: flume::Receiver<()>,
}

impl Core {
  /// Starts the core. Never fails: a missing or rejected session arrives as [`Event::SignedOut`].
  /// Cached conversations are emitted immediately, before any network round trip. In [`demo_mode`], it serves
  /// the demo workspace instead.
  pub fn start() -> Self {
    let (commands, command_rx) = flume::unbounded();
    let (event_tx, events) = flume::unbounded();
    let (stop, stop_rx) = flume::bounded(1);
    let (done, stopped) = flume::bounded(1);
    let fetcher = if demo_mode() {
      // The demo writes nothing to disk: it is done at once.
      drop((stop_rx, done));
      demo::spawn(command_rx, event_tx)
    } else {
      engine::spawn(command_rx, event_tx, stop_rx, done)
    };
    Self { commands, events, fetcher, stop, stopped }
  }

  /// Stops the core for the app's quit: its command loop ends, and the history changes it still holds back from the
  /// disk cache are written. Blocks the calling thread until that is done or `timeout` passes, whichever is first.
  /// Every clone of this handle shares the one core, so commands sent afterwards are not handled.
  pub fn shutdown(&self, timeout: Duration) {
    let _ = self.stop.try_send(());
    let _ = self.stopped.recv_timeout(timeout);
  }

  /// Enqueue a command. Dropped only if the core thread has stopped.
  pub fn send(&self, command: Command) {
    let _ = self.commands.send(command);
  }

  /// Stream of events. Await `recv_async()` on it from any executor.
  pub const fn events(&self) -> &flume::Receiver<Event> {
    &self.events
  }

  /// Downloads `url` with the session (avatars, attachment thumbnails). Results are cached on disk, so a repeat
  /// call, also after a restart, answers without the network.
  pub async fn fetch(&self, url: String) -> anyhow::Result<Arc<Vec<u8>>> {
    self.fetcher.fetch(url).await
  }
}

#[cfg(test)]
mod tests {
  use super::{SessionCookie, merge_session_cookies, write_cookies};
  use crate::http::read_cookies;

  fn cookie(name: &str, domain: &str, value: &str) -> SessionCookie {
    SessionCookie {
      name: name.into(),
      value: value.into(),
      domain: domain.into(),
      path: "/".into(),
    }
  }

  #[test]
  fn write_cookies_roundtrip() {
    let dir = std::env::temp_dir().join(format!("working-session-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("cookies.json");
    let cookies = vec![cookie("SAPISID", ".google.com", "x")];
    write_cookies(&path, &cookies).expect("write");
    let read = read_cookies(&path).expect("read");
    assert_eq!(read.len(), cookies.len());
    assert!(read.iter().zip(&cookies).all(|(got, want)| {
      got.name == want.name && got.domain == want.domain && got.path == want.path && got.value == want.value
    }));
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn merge_updates_and_adds_and_keeps_the_rest() {
    let dir = std::env::temp_dir().join(format!("working-session-merge-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("cookies.json");
    let saved = vec![cookie("SID", ".google.com", "kept"), cookie("SIDCC", ".google.com", "old")];
    write_cookies(&path, &saved).expect("write");
    let refreshed = vec![
      cookie("SIDCC", ".google.com", "new"),
      cookie("COMPASS", "chat.google.com", "added"),
    ];
    assert!(merge_session_cookies(&path, refreshed.clone()).expect("merge"));
    let values: Vec<_> = read_cookies(&path)
      .expect("read")
      .into_iter()
      .map(|cookie| (cookie.name, cookie.value))
      .collect();
    let want =
      [("SID", "kept"), ("SIDCC", "new"), ("COMPASS", "added")].map(|(name, value)| (name.into(), value.into()));
    assert_eq!(values, want);
    // Nothing new: the file is left alone.
    assert!(!merge_session_cookies(&path, refreshed).expect("merge"));
    let _ = std::fs::remove_dir_all(&dir);
  }
}
