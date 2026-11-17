//! One in-memory image cache. Fetching goes through the core (disk cached), decoding runs on the background executor,
//! and each URL is requested at most once per size.
//!
//! Two classes: small images (avatars, card pictures, at most `SMALL_MAX` pixels) are kept for the session; larger
//! ones (message thumbnails) are decoded at the size they are drawn, in buckets, and held in a byte-bounded LRU whose
//! evicted images also leave the windows' sprite atlases.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gchat_core::Core;
use gpui_kit::*;

/// The largest decoded size, in pixels, of the small class, which is never evicted.
const SMALL_MAX: u32 = 256;
/// The decoded sizes of the large class: a request takes the smallest that covers it, else the last.
const BUCKETS: [u32; 3] = [512, 1024, 2048];
/// How many bytes of decoded large images the cache keeps before it evicts the least recently drawn.
const LARGE_BUDGET: usize = 192 * 1024 * 1024;

enum Slot {
  Loading,
  Ready(Arc<RenderImage>),
  Failed,
}

/// One image of the large class, with its decoded size and when a view last asked for it.
struct Entry {
  slot: Slot,
  bytes: usize,
  used: Instant,
}

#[derive(Default)]
struct Cache {
  small: HashMap<String, Slot>,
  large: HashMap<(String, u32), Entry>,
  /// The decoded bytes of the ready entries of `large`.
  large_bytes: usize,
}

impl Cache {
  /// Drops the least recently asked-for ready images of the large class until it fits its budget, keeping `keep`.
  fn evict(&mut self, keep: &(String, u32)) -> Vec<Arc<RenderImage>> {
    let mut dropped = Vec::new();
    while self.large_bytes > LARGE_BUDGET {
      let oldest = self
        .large
        .iter()
        .filter(|(key, entry)| *key != keep && matches!(entry.slot, Slot::Ready(_)))
        .min_by_key(|(_, entry)| entry.used)
        .map(|(key, _)| key.clone());
      let Some(entry) = oldest.and_then(|key| self.large.remove(&key)) else {
        break;
      };
      self.large_bytes = self.large_bytes.saturating_sub(entry.bytes);
      if let Slot::Ready(image) = entry.slot {
        dropped.push(image);
      }
    }
    dropped
  }
}

/// Notified whenever an image arrives; views that read it redraw.
pub struct Tick;

#[derive(Clone)]
pub struct Images {
  cache: Rc<RefCell<Cache>>,
  tick: Entity<Tick>,
  core: Core,
}

impl Images {
  pub fn new(core: Core, cx: &mut App) -> Self {
    Self {
      cache: Rc::default(),
      tick: cx.new(|_| Tick),
      core,
    }
  }

  /// The entity notified whenever an image arrives: a cached view that draws images observes it to redraw.
  pub const fn tick(&self) -> &Entity<Tick> {
    &self.tick
  }

  /// Returns the decoded image when it is ready. The first call for a URL starts the fetch.
  /// `max_dim` bounds the decoded size (larger images are downscaled off the UI thread). Up to `SMALL_MAX` the image
  /// stays for the session; above it, it is decoded at the bucket that covers `max_dim` and may be evicted.
  pub fn get(&self, url: &str, max_dim: u32, cx: &App) -> Option<Arc<RenderImage>> {
    if max_dim <= SMALL_MAX {
      self.small(url, max_dim, cx)
    } else {
      self.large(url, bucket(max_dim), cx)
    }
  }

  /// A thumbnail drawn at most `logical` points on its longer side, in a window of `scale` pixels per point.
  pub fn thumb(&self, url: &str, logical: f32, scale: f32, cx: &App) -> Option<Arc<RenderImage>> {
    let needed = (logical * scale).ceil().max(1.) as u32;
    self.get(url, needed.max(SMALL_MAX + 1), cx)
  }

  fn small(&self, url: &str, max_dim: u32, cx: &App) -> Option<Arc<RenderImage>> {
    let _ = self.tick.read(cx);
    {
      let cache = self.cache.borrow();
      match cache.small.get(url) {
        Some(Slot::Ready(image)) => return Some(image.clone()),
        Some(_) => return None,
        None => {},
      }
    }
    self.cache.borrow_mut().small.insert(url.to_string(), Slot::Loading);
    let cache = self.cache.clone();
    self.load(url.to_string(), max_dim, cx, move |url, image, _| {
      cache.borrow_mut().small.insert(url, image.map_or(Slot::Failed, Slot::Ready));
    });
    None
  }

  fn large(&self, url: &str, size: u32, cx: &App) -> Option<Arc<RenderImage>> {
    let _ = self.tick.read(cx);
    let key = (url.to_string(), size);
    {
      let mut cache = self.cache.borrow_mut();
      if let Some(entry) = cache.large.get_mut(&key) {
        entry.used = Instant::now();
        return match &entry.slot {
          Slot::Ready(image) => Some(image.clone()),
          Slot::Loading | Slot::Failed => None,
        };
      }
      cache.large.insert(
        key.clone(),
        Entry {
          slot: Slot::Loading,
          bytes: 0,
          used: Instant::now(),
        },
      );
    }
    let cache = self.cache.clone();
    self.load(url.to_string(), size, cx, move |_, image, cx| {
      let dropped = {
        let mut guard = cache.borrow_mut();
        let cache = &mut *guard;
        let bytes = image.as_deref().map_or(0, decoded_bytes);
        let Some(entry) = cache.large.get_mut(&key) else {
          return;
        };
        entry.slot = image.map_or(Slot::Failed, Slot::Ready);
        entry.bytes = bytes;
        cache.large_bytes += bytes;
        cache.evict(&key)
      };
      // Every view that draws images redraws on the tick that follows, so none paints an evicted one again.
      for image in dropped {
        cx.drop_image(image, None);
      }
    });
    None
  }

  /// Fetches and decodes `url` at `max_dim`, hands the result to `store`, then notifies the tick.
  fn load(
    &self,
    url: String,
    max_dim: u32,
    cx: &App,
    store: impl FnOnce(String, Option<Arc<RenderImage>>, &mut App) + 'static,
  ) {
    let tick = self.tick.clone();
    let core = self.core.clone();
    cx.spawn(async move |cx| {
      let host = url.split('/').nth(2).unwrap_or("?").to_string();
      let image = match core.fetch(url.clone()).await {
        Ok(bytes) => {
          let len = bytes.len();
          let decoded = cx.background_executor().spawn(async move { decode(&bytes, max_dim) }).await;
          if decoded.is_none() {
            tracing::warn!("image decode failed: {host} ({len} bytes)");
          }
          decoded
        },
        Err(err) => {
          tracing::warn!("image fetch failed: {host}: {err}");
          None
        },
      };
      cx.update(|cx| {
        store(url, image, cx);
        tick.update(cx, |_, cx| cx.notify());
      });
    })
    .detach();
  }
}

/// The decoded size of the large class that covers `max_dim` pixels.
fn bucket(max_dim: u32) -> u32 {
  BUCKETS
    .iter()
    .copied()
    .find(|bucket| *bucket >= max_dim)
    .unwrap_or(BUCKETS[BUCKETS.len() - 1])
}

/// How many bytes an image's frames take, four per pixel.
fn decoded_bytes(image: &RenderImage) -> usize {
  (0..image.frame_count())
    .map(|frame| {
      let size = image.size(frame);
      (size.width.0 as usize) * (size.height.0 as usize) * 4
    })
    .sum()
}

pub fn decode(bytes: &[u8], max_dim: u32) -> Option<Arc<RenderImage>> {
  let mut image = image::load_from_memory(bytes).ok()?;
  if image.width() > max_dim || image.height() > max_dim {
    image = image.thumbnail(max_dim, max_dim);
  }
  let mut rgba = image.into_rgba8();
  for pixel in rgba.as_chunks_mut::<4>().0 {
    pixel.swap(0, 2);
  }
  Some(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])))
}

#[cfg(test)]
mod tests {
  use super::bucket;

  #[test]
  fn buckets_cover_the_request_up_to_the_largest() {
    assert_eq!(bucket(257), 512);
    assert_eq!(bucket(840), 1024);
    assert_eq!(bucket(1024), 1024);
    assert_eq!(bucket(5000), 2048);
  }
}
