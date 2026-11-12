//! Native screen capture for a shared screen in a call.
#![allow(unsafe_code, reason = "CoreGraphics screen-recording FFI")]
//!
//! `WebKit` captures a display at its size in points, so a Retina screen reaches viewers at half its resolution
//! and reads blurry. This captures the display's real pixels instead; `call.rs` hands the frames to the Meet
//! page, which sends them in place of `WebKit`'s capture.
//!
//! macOS only lets an app capture the screen after the user allows it under Privacy & Security > Screen &
//! System Audio Recording. Without that, the call keeps `WebKit`'s capture.

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use base64::Engine as _;
use image::ExtendedColorType;
use image::codecs::jpeg::JpegEncoder;
use objc2::runtime::AnyObject;
use xcap::{Monitor, Window};

/// Shared screens are mostly still; this is as often as a change is picked up.
const FRAME_INTERVAL: Duration = Duration::from_millis(200);
const JPEG_QUALITY: u8 = 92;

#[link(name = "ScreenCaptureKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
  fn CGPreflightScreenCaptureAccess() -> bool;
  fn CGRequestScreenCaptureAccess() -> bool;
}

/// Whether macOS lets this app capture the screen.
pub fn permitted() -> bool {
  // SAFETY: plain C calls without arguments.
  unsafe { CGPreflightScreenCaptureAccess() }
}

/// Asks macOS for the permission. Neither the request call nor a capture attempt raises the system prompt on
/// current macOS (the permission service refuses to prompt for them); asking `ScreenCaptureKit` what can be
/// shared does, and lists the app in System Settings.
pub fn request_permission() {
  let ignore = block2::RcBlock::new(|_content: *mut AnyObject, _error: *mut AnyObject| {});
  // SAFETY: a class method taking one block, called with a block of the declared shape.
  unsafe {
    CGRequestScreenCaptureAccess();
    let _: () = objc2::msg_send![objc2::class!(SCShareableContent), getShareableContentWithCompletionHandler: &*ignore];
  }
}

/// One captured frame, ready to hand to the page.
pub struct Frame {
  pub jpeg_base64: String,
}

/// What a share shows, as the page knows it: a whole display or one window, its size in points, and the name
/// `WebKit` gives it.
pub enum Shared {
  Display { points: (u32, u32) },
  Window { points: (u32, u32), label: String },
}

/// The display or window found for a share.
enum Source {
  Display(Monitor),
  Window(Window),
}

impl Source {
  fn find(shared: &Shared) -> Option<Self> {
    match shared {
      Shared::Display { points } => {
        let monitors = Monitor::all().ok()?;
        let matches =
          |monitor: &Monitor| monitor.width().ok() == Some(points.0) && monitor.height().ok() == Some(points.1);
        let by_size = monitors.iter().position(matches);
        let primary = monitors.iter().position(|monitor| monitor.is_primary().unwrap_or(false));
        monitors.into_iter().nth(by_size.or(primary).unwrap_or(0)).map(Source::Display)
      },
      // The picker does not say which window it was: it is found by its size, and by name when several
      // windows have that size.
      Shared::Window { points, label } => {
        let label = label.to_lowercase();
        let same_size = |window: &Window| {
          let close = |a: u32, b: u32| a.abs_diff(b) <= 2;
          !window.is_minimized().unwrap_or(true)
            && window.width().is_ok_and(|width| close(width, points.0))
            && window.height().is_ok_and(|height| close(height, points.1))
        };
        let named = |window: &Window| {
          let within = |name: Result<String, _>| {
            name.is_ok_and(|name: String| !name.is_empty() && label.contains(&name.to_lowercase()))
          };
          within(window.title()) || within(window.app_name())
        };
        let candidates: Vec<Window> = Window::all().ok()?.into_iter().filter(same_size).collect();
        let index = candidates.iter().position(named).unwrap_or(0);
        candidates.into_iter().nth(index).map(Source::Window)
      },
    }
  }

  fn picture(&self) -> Result<xcap::image::RgbaImage, String> {
    match self {
      Self::Display(monitor) => monitor.capture_image(),
      Self::Window(window) => window.capture_image(),
    }
    .map_err(|error| error.to_string())
  }
}

/// A running capture of one display or window. Dropping it stops the capture.
pub struct ScreenCapture {
  stop: Arc<AtomicBool>,
}

impl ScreenCapture {
  /// Captures what `shared` describes, sending a frame each time its picture changes. When it cannot be
  /// found or captured, the channel closes without a frame.
  pub fn start(shared: Shared, frames: flume::Sender<Frame>) -> Self {
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    std::thread::spawn(move || {
      let Some(source) = Source::find(&shared) else {
        return;
      };
      let mut last = None;
      while !stopped.load(Ordering::Relaxed) {
        let started = Instant::now();
        match capture(&source, &mut last) {
          Ok(Some(frame)) => {
            if frames.send(frame).is_err() {
              break;
            }
          },
          Ok(None) => {},
          Err(error) => {
            tracing::warn!("screen capture failed: {error}");
            break;
          },
        }
        std::thread::sleep(FRAME_INTERVAL.saturating_sub(started.elapsed()));
      }
    });
    Self { stop }
  }
}

impl Drop for ScreenCapture {
  fn drop(&mut self) {
    self.stop.store(true, Ordering::Relaxed);
  }
}

/// Captures the source, answering a frame only when the picture differs from the previous one.
fn capture(source: &Source, last: &mut Option<u64>) -> Result<Option<Frame>, String> {
  let picture = source.picture()?;
  let (width, height) = (picture.width(), picture.height());
  let rgba = picture.as_raw();

  let mut hasher = std::collections::hash_map::DefaultHasher::new();
  rgba.hash(&mut hasher);
  let fingerprint = hasher.finish();
  if last.replace(fingerprint) == Some(fingerprint) {
    return Ok(None);
  }

  let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
  for pixel in rgba.as_chunks::<4>().0 {
    rgb.extend_from_slice(&pixel[..3]);
  }
  let mut jpeg = Vec::with_capacity(rgb.len() / 8);
  JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
    .encode(&rgb, width, height, ExtendedColorType::Rgb8)
    .map_err(|error| error.to_string())?;
  Ok(Some(Frame {
    jpeg_base64: base64::engine::general_purpose::STANDARD.encode(jpeg),
  }))
}
