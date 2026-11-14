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
mod proto;

use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use calendar::{EVENT_COLORS, boundary_date, local_midnight};
pub use engine::device_time_zone;
pub use http::SessionCookie;
pub use model::*;

/// True when `WORKING_DEMO=1`: the core serves a made-up workspace and never signs in.
pub fn demo_mode() -> bool {
  std::env::var("WORKING_DEMO").is_ok_and(|value| value == "1")
}

/// Directory holding `cookies.json` (written by the app or `scripts/login.sh`).
///
/// In demo mode, a directory of its own under the temp dir, so the demo never reads the session or changes
/// the settings of a real run.
pub fn config_dir() -> PathBuf {
  if demo_mode() {
    return std::env::temp_dir().join("working-demo");
  }
  let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
  home.join(".config/working")
}

/// Session cookies for an embedded Meet browser.
///
/// Empty when signed out.
pub fn session_cookies() -> Vec<SessionCookie> {
  http::read_cookies(&config_dir().join("cookies.json")).unwrap_or_default()
}

/// Merges Meet-rotated session cookies into the saved file.
///
/// Google rotates cookies while a call is open. Cookies the browser does not hold are kept. Answers whether
/// the file changed.
pub fn save_session_cookies(current: Vec<SessionCookie>) -> io::Result<bool> {
  let path = config_dir().join("cookies.json");
  let saved = session_cookies();
  let mut merged = saved.clone();
  for cookie in current {
    let same =
      |held: &&mut SessionCookie| held.name == cookie.name && held.domain == cookie.domain && held.path == cookie.path;
    match merged.iter_mut().find(same) {
      Some(held) => held.value = cookie.value,
      None => merged.push(cookie),
    }
  }
  if merged == saved {
    return Ok(false);
  }
  write_cookies(&path, &merged)?;
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
  let mut file = std::fs::OpenOptions::new()
    .write(true)
    .create(true)
    .truncate(true)
    .mode(0o600)
    .open(&staging)?;
  file.write_all(text.as_bytes())?;
  std::fs::rename(&staging, path)?;
  Ok(())
}

/// Handle to the running core. Cheap to clone.
#[derive(Clone)]
pub struct Core {
  commands: flume::Sender<Command>,
  events: flume::Receiver<Event>,
  fetcher: Arc<engine::Fetcher>,
}

impl Core {
  /// Starts the core. Never fails: a missing or rejected session arrives as [`Event::SignedOut`].
  /// Cached conversations are emitted immediately, before any network round trip. In [`demo_mode`], it serves
  /// the demo workspace instead.
  pub fn start() -> Self {
    let (commands, command_rx) = flume::unbounded();
    let (event_tx, events) = flume::unbounded();
    let fetcher = if demo_mode() {
      demo::spawn(command_rx, event_tx)
    } else {
      engine::spawn(command_rx, event_tx)
    };
    Self { commands, events, fetcher }
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
  use super::{SessionCookie, write_cookies};
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
}
