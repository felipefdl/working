//! Disk cache in [`crate::cache_dir`]: what makes a cold start and a first click paint without the network.
#![allow(
  clippy::unused_self,
  reason = "read/write stay methods so call sites stay on Cache"
)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::model::{Group, GroupId, Message, Micros, User};

/// Bytes of images and downloads kept; the ones used longest ago go first.
const IMAGE_BUDGET: u64 = 512 * 1024 * 1024;

#[derive(Default, Serialize, Deserialize)]
pub struct WorldCache {
  pub me: Option<User>,
  pub users: Vec<User>,
  pub groups: Vec<Group>,
  /// XSRF token with the Unix second it was read, to skip the bootstrap request on the next start.
  pub xsrf: Option<(String, u64)>,
  /// Names of the user's own sidebar sections, by section id.
  #[serde(default)]
  pub sections: HashMap<String, String>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct HistoryCache {
  pub messages: Vec<Arc<Message>>,
  pub reached_start: bool,
  /// Sort time of the oldest topic held: where the next older page starts.
  #[serde(default)]
  pub anchor: Micros,
}

#[derive(Clone)]
pub struct Cache {
  dir: PathBuf,
  /// SHA-1 of the conversation list last written, without the token's read time. Held while `world.json` is
  /// written, so two writes never overlap.
  world_written: Arc<Mutex<Option<[u8; 20]>>>,
}

impl Cache {
  pub fn open() -> Self {
    let dir = crate::cache_dir();
    let _ = std::fs::create_dir_all(dir.join("history"));
    let _ = std::fs::create_dir_all(dir.join("img"));
    Self {
      dir,
      world_written: Arc::new(Mutex::new(None)),
    }
  }

  fn read<T: for<'de> Deserialize<'de>>(&self, path: PathBuf) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
  }

  /// Writes through a temporary file so a crash never leaves a half-written cache. True when the file is in place.
  fn write<T: Serialize>(&self, path: &Path, value: &T) -> bool {
    serde_json::to_vec(value).is_ok_and(|bytes| self.write_bytes(path, &bytes))
  }

  /// Writes through a temporary file of its own, so writers of the same file never share one and a reader sees the
  /// old file or the new one, never part of one.
  fn write_bytes(&self, path: &Path, bytes: &[u8]) -> bool {
    let temporary = path.with_extension(format!("{:016x}.tmp", rand::random::<u64>()));
    if std::fs::write(&temporary, bytes).is_err() {
      let _ = std::fs::remove_file(&temporary);
      return false;
    }
    if std::fs::rename(&temporary, path).is_err() {
      let _ = std::fs::remove_file(&temporary);
      return false;
    }
    true
  }

  pub fn world(&self) -> WorldCache {
    self.read(self.dir.join("world.json")).unwrap_or_default()
  }

  /// Saves the conversation list unless it is what was saved last. People and conversations go in id order, and the
  /// token's read time is left out of the comparison, so an unchanged list is never written again.
  pub fn save_world(&self, mut world: WorldCache) {
    world.users.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    world.groups.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    let mut written = self.world_written.lock().unwrap_or_else(PoisonError::into_inner);
    let read_at = world.xsrf.as_mut().map(|(_, read_at)| std::mem::take(read_at));
    let Ok(unstamped) = serde_json::to_vec(&world) else {
      return;
    };
    let fingerprint = sha1_smol::Sha1::from(&unstamped).digest().bytes();
    if *written == Some(fingerprint) {
      return;
    }
    if let (Some((_, slot)), Some(read_at)) = (world.xsrf.as_mut(), read_at) {
      *slot = read_at;
    }
    if self.write(&self.dir.join("world.json"), &world) {
      *written = Some(fingerprint);
    }
  }

  pub fn history(&self, group: &GroupId) -> Option<HistoryCache> {
    self.read(self.dir.join("history").join(format!("{}.json", group.key())))
  }

  pub fn save_history(&self, group: &GroupId, history: &HistoryCache) {
    let _ = self.write(&self.dir.join("history").join(format!("{}.json", group.key())), history);
  }

  /// Where a URL's bytes are kept: named by the SHA-1 of the URL, the same on every build.
  fn image_path(&self, url: &str) -> PathBuf {
    self.dir.join("img").join(sha1_smol::Sha1::from(url).digest().to_string())
  }

  pub fn image(&self, url: &str) -> Option<Vec<u8>> {
    let path = self.image_path(url);
    let bytes = std::fs::read(&path).ok()?;
    // A read counts as a use: `trim_images` goes by modification time.
    if let Ok(file) = std::fs::File::options().append(true).open(&path) {
      let _ = file.set_modified(SystemTime::now());
    }
    Some(bytes)
  }

  pub fn save_image(&self, url: &str, bytes: &[u8]) {
    let _ = self.write_bytes(&self.image_path(url), bytes);
  }

  /// Removes the images used longest ago until the rest fit in `IMAGE_BUDGET`. A 16-digit name is not a key this
  /// cache makes, so no read finds that file: it goes too.
  pub fn trim_images(&self) {
    let Ok(entries) = std::fs::read_dir(self.dir.join("img")) else {
      return;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
      let Ok(metadata) = entry.metadata() else {
        continue;
      };
      if !metadata.is_file() {
        continue;
      }
      if entry.file_name().len() == 16 {
        let _ = std::fs::remove_file(entry.path());
        continue;
      }
      files.push((metadata.modified().unwrap_or(UNIX_EPOCH), metadata.len(), entry.path()));
    }
    files.sort_unstable_by_key(|file| std::cmp::Reverse(file.0));
    let mut kept: u64 = 0;
    for (_, size, path) in files {
      kept = kept.saturating_add(size);
      if kept > IMAGE_BUDGET {
        let _ = std::fs::remove_file(path);
      }
    }
  }
}
