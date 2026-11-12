//! Disk cache under `~/Library/Caches/working`: what makes a cold start and a first click paint without the network.
#![allow(
  clippy::unused_self,
  reason = "read/write stay methods so call sites stay on Cache"
)]

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::model::{Group, GroupId, Message, Micros, User};

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
}

impl Cache {
  pub fn open() -> Self {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let dir = home.join("Library/Caches/working");
    let _ = std::fs::create_dir_all(dir.join("history"));
    let _ = std::fs::create_dir_all(dir.join("img"));
    Self { dir }
  }

  fn read<T: for<'de> Deserialize<'de>>(&self, path: PathBuf) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
  }

  /// Writes through a temporary file so a crash never leaves a half-written cache.
  fn write<T: Serialize>(&self, path: PathBuf, value: &T) {
    let Ok(bytes) = serde_json::to_vec(value) else {
      return;
    };
    let temporary = path.with_extension("tmp");
    if std::fs::write(&temporary, bytes).is_ok() {
      let _ = std::fs::rename(temporary, path);
    }
  }

  pub fn world(&self) -> WorldCache {
    self.read(self.dir.join("world.json")).unwrap_or_default()
  }

  pub fn save_world(&self, world: &WorldCache) {
    self.write(self.dir.join("world.json"), world);
  }

  pub fn history(&self, group: &GroupId) -> Option<HistoryCache> {
    self.read(self.dir.join("history").join(format!("{}.json", group.key())))
  }

  pub fn save_history(&self, group: &GroupId, history: &HistoryCache) {
    self.write(self.dir.join("history").join(format!("{}.json", group.key())), history);
  }

  fn image_path(&self, url: &str) -> PathBuf {
    let mut hasher = std::hash::DefaultHasher::new();
    url.hash(&mut hasher);
    self.dir.join("img").join(format!("{:016x}", hasher.finish()))
  }

  pub fn image(&self, url: &str) -> Option<Vec<u8>> {
    std::fs::read(self.image_path(url)).ok()
  }

  pub fn save_image(&self, url: &str, bytes: &[u8]) {
    let _ = std::fs::write(self.image_path(url), bytes);
  }
}
