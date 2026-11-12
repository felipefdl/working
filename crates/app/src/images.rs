//! One in-memory image cache keyed by URL. Fetching goes through the core (disk cached), decoding runs on the
//! background executor, and each URL is requested at most once.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gchat_core::Core;
use gpui_kit::*;

enum Slot {
  Loading,
  Ready(Arc<RenderImage>),
  Failed,
}

/// Notified whenever an image arrives; views that read it redraw.
pub struct Tick;

#[derive(Clone)]
pub struct Images {
  slots: Rc<RefCell<HashMap<String, Slot>>>,
  tick: Entity<Tick>,
  core: Core,
}

impl Images {
  pub fn new(core: Core, cx: &mut App) -> Self {
    Self {
      slots: Rc::default(),
      tick: cx.new(|_| Tick),
      core,
    }
  }

  /// Returns the decoded image when it is ready. The first call for a URL starts the fetch.
  /// `max_dim` bounds the decoded size (larger images are downscaled off the UI thread).
  pub fn get(&self, url: &str, max_dim: u32, cx: &App) -> Option<Arc<RenderImage>> {
    let _ = self.tick.read(cx);
    {
      let slots = self.slots.borrow();
      match slots.get(url) {
        Some(Slot::Ready(image)) => return Some(image.clone()),
        Some(_) => return None,
        None => {},
      }
    }
    self.slots.borrow_mut().insert(url.to_string(), Slot::Loading);
    let slots = self.slots.clone();
    let tick = self.tick.clone();
    let core = self.core.clone();
    let url = url.to_string();
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
        let slot = image.map_or(Slot::Failed, Slot::Ready);
        slots.borrow_mut().insert(url, slot);
        tick.update(cx, |_, cx| cx.notify());
      });
    })
    .detach();
    None
  }
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
