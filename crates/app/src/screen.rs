//! Native screen sharing for a call.
#![allow(unsafe_code, reason = "ScreenCaptureKit FFI")]
//!
//! The app shares the screen itself instead of leaving it to `WebKit`, for two reasons. `WebKit` captures a
//! display at its size in points, so a Retina screen reaches viewers at half its resolution and reads blurry.
//! And macOS gives the answer of its picker to one screen-capture client per app: when `WebKit` and the app
//! both captured, `WebKit`'s request for a second share never got its answer.
//!
//! [`Share::pick`] opens the macOS picker. What the user picks there is captured at its real pixels and comes
//! out as JPEG frames; `call.rs` passes them to the Meet page, where `call.js` hands Meet a stream made from
//! them. Content picked this way needs no Screen Recording permission.
//!
//! The picker needs macOS 14. Before that [`available`] is false and the call keeps `WebKit`'s capture.

use std::sync::Mutex;

use base64::Engine as _;
use block2::RcBlock;
use dispatch2::DispatchQueue;
use image::ExtendedColorType;
use image::codecs::jpeg::JpegEncoder;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
  CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
  CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
  kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSDictionary, NSError, NSNumber, NSString};
use objc2_screen_capture_kit::{
  SCContentFilter, SCContentSharingPicker, SCContentSharingPickerConfiguration, SCContentSharingPickerMode,
  SCContentSharingPickerObserver, SCFrameStatus, SCStream, SCStreamConfiguration, SCStreamDelegate,
  SCStreamFrameInfoContentRect, SCStreamFrameInfoContentScale, SCStreamFrameInfoScaleFactor, SCStreamFrameInfoStatus,
  SCStreamOutput, SCStreamOutputType,
};

/// Shared screens are mostly still; this is as often as a change is picked up.
const FRAMES_PER_SECOND: i32 = 5;
const JPEG_QUALITY: u8 = 92;

/// Whether this macOS has the system picker for shared content.
pub fn available() -> bool {
  AnyClass::get(c"SCContentSharingPicker").is_some()
}

/// One captured frame, ready to hand to the page.
pub struct Frame {
  pub jpeg_base64: String,
}

/// Why a share is over without the call having stopped it.
pub enum End {
  /// The picker was closed without a choice, or the capture could not start.
  Cancelled,
  /// macOS stopped the capture: "Stop Sharing" in the menu bar, or the shared window closed.
  Stopped,
}

/// A size in pixels.
type Size = (usize, usize);

/// The running capture of what was picked.
struct Capture {
  stream: Retained<SCStream>,
  /// The frame size the stream is set to.
  size: Size,
}

/// What the picker and the stream report to. Their callbacks arrive on queues of their own.
struct Session {
  frames: flume::Sender<Frame>,
  end: flume::Sender<End>,
  /// Frames are encoded here, off the main thread.
  queue: dispatch2::DispatchRetained<DispatchQueue>,
  capture: Mutex<Option<Capture>>,
}

define_class!(
  #[unsafe(super(NSObject))]
  #[name = "WorkingScreenShare"]
  #[ivars = Session]
  struct Listener;

  unsafe impl NSObjectProtocol for Listener {}

  unsafe impl SCContentSharingPickerObserver for Listener {
    /// Also sent when the picker was opened from the menu bar to change a running share and then closed; that
    /// share goes on.
    #[unsafe(method(contentSharingPicker:didCancelForStream:))]
    fn cancelled(&self, _picker: &SCContentSharingPicker, _stream: Option<&SCStream>) {
      if self.ivars().capture.lock().is_ok_and(|capture| capture.is_none()) {
        let _ = self.ivars().end.send(End::Cancelled);
      }
    }

    /// The first pick starts the capture. A later one, from the menu bar, changes what it shows.
    #[unsafe(method(contentSharingPicker:didUpdateWithFilter:forStream:))]
    fn picked(&self, _picker: &SCContentSharingPicker, filter: &SCContentFilter, _stream: Option<&SCStream>) {
      self.show(filter);
    }

    #[unsafe(method(contentSharingPickerStartDidFailWithError:))]
    fn failed(&self, error: &NSError) {
      tracing::warn!("the screen picker did not open: {}", error.localizedDescription());
      let _ = self.ivars().end.send(End::Cancelled);
    }
  }

  unsafe impl SCStreamDelegate for Listener {
    #[unsafe(method(stream:didStopWithError:))]
    fn stopped(&self, _stream: &SCStream, _error: &NSError) {
      if let Ok(mut capture) = self.ivars().capture.lock() {
        *capture = None;
      }
      let _ = self.ivars().end.send(End::Stopped);
    }
  }

  unsafe impl SCStreamOutput for Listener {
    #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
    fn output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
      if kind == SCStreamOutputType::Screen {
        self.frame(sample);
      }
    }
  }
);

impl Listener {
  fn show(&self, filter: &SCContentFilter) {
    let Ok(mut capture) = self.ivars().capture.lock() else {
      return;
    };
    // SAFETY: plain getters on the filter the picker just handed over.
    let (rect, scale) = unsafe { (filter.contentRect(), f64::from(filter.pointPixelScale())) };
    let size = (pixels(rect.size.width * scale), pixels(rect.size.height * scale));
    let configuration = configuration(size);
    if let Some(running) = capture.as_mut() {
      // SAFETY: both take an object of the declared class; no completion handler is wanted.
      unsafe {
        running.stream.updateContentFilter_completionHandler(filter, None);
        running.stream.updateConfiguration_completionHandler(&configuration, None);
      }
      running.size = size;
      return;
    }
    // SAFETY: the designated initializer, with this object as the delegate it declares it is.
    let stream = unsafe {
      SCStream::initWithFilter_configuration_delegate(
        SCStream::alloc(),
        filter,
        &configuration,
        Some(ProtocolObject::from_ref(self)),
      )
    };
    // SAFETY: this object implements the output protocol, and the queue outlives the stream.
    let added = unsafe {
      stream.addStreamOutput_type_sampleHandlerQueue_error(
        ProtocolObject::from_ref(self),
        SCStreamOutputType::Screen,
        Some(&self.ivars().queue),
      )
    };
    if let Err(error) = added {
      tracing::warn!("the screen capture has no output: {}", error.localizedDescription());
      let _ = self.ivars().end.send(End::Cancelled);
      return;
    }
    let end = self.ivars().end.clone();
    let started = RcBlock::new(move |error: *mut NSError| {
      // SAFETY: the handler gets a valid error or null.
      if let Some(error) = unsafe { error.as_ref() } {
        tracing::warn!("the screen capture did not start: {}", error.localizedDescription());
        let _ = end.send(End::Cancelled);
      }
    });
    // SAFETY: a block of the declared shape.
    unsafe { stream.startCaptureWithCompletionHandler(Some(&started)) };
    *capture = Some(Capture { stream, size });
  }

  /// Sends the frame on when it holds a new picture. Called on the encoding queue; waits there for the page
  /// to take the previous frame, so pictures do not pile up.
  fn frame(&self, sample: &CMSampleBuffer) {
    let Some(content) = Content::of(sample) else {
      return;
    };
    // SAFETY: a getter on a valid sample buffer.
    let Some(buffer) = (unsafe { sample.image_buffer() }) else {
      return;
    };
    // A shared window was resized: the following frames come at its new size.
    if let Ok(mut capture) = self.ivars().capture.lock()
      && let Some(running) = capture.as_mut()
      && running.size != content.full
    {
      running.size = content.full;
      // SAFETY: takes an object of the declared class; no completion handler is wanted.
      unsafe {
        running
          .stream
          .updateConfiguration_completionHandler(&configuration(content.full), None);
      }
    }
    if let Some(jpeg_base64) = jpeg(&buffer, content.shown) {
      let _ = self.ivars().frames.send(Frame { jpeg_base64 });
    }
  }
}

/// Where the shared content is in a frame.
struct Content {
  /// The part of the frame that holds the content, from its top left.
  shown: Size,
  /// The content's size at its real pixels, which the stream should be set to.
  full: Size,
}

impl Content {
  /// Reads what the stream attached to a frame. `None` when the frame has no new picture.
  fn of(sample: &CMSampleBuffer) -> Option<Self> {
    // SAFETY: a getter on a valid sample buffer.
    let attachments = unsafe { sample.sample_attachments_array(false) }?;
    if attachments.count() < 1 {
      return None;
    }
    // SAFETY: the index is in bounds, and a frame's attachments are dictionaries.
    let info = unsafe { attachments.value_at_index(0).cast::<NSDictionary>().as_ref() }?;
    // SAFETY: the keys are constants of the framework.
    let (status, rect, content_scale, scale) = unsafe {
      (
        SCStreamFrameInfoStatus,
        SCStreamFrameInfoContentRect,
        SCStreamFrameInfoContentScale,
        SCStreamFrameInfoScaleFactor,
      )
    };
    let value =
      |dictionary: &NSDictionary, key: &NSString| dictionary.objectForKey(key.as_ref())?.downcast::<NSNumber>().ok();
    let number = |dictionary: &NSDictionary, key: &NSString| Some(value(dictionary, key)?.doubleValue());
    if value(info, status)?.integerValue() != SCFrameStatus::Complete.0 {
      return None;
    }
    // The rectangle comes as a dictionary of its numbers, in points of the frame.
    let rect = info.objectForKey(rect.as_ref())?.downcast::<NSDictionary>().ok()?;
    let width = number(&rect, &NSString::from_str("Width"))?;
    let height = number(&rect, &NSString::from_str("Height"))?;
    let scale = number(info, scale)?;
    // How much the content was shrunk to fit a frame that was set for a smaller window.
    let content_scale = number(info, content_scale).filter(|scale| *scale > 0.).unwrap_or(1.);
    Some(Self {
      shown: (pixels(width * scale), pixels(height * scale)),
      full: (pixels(width / content_scale * scale), pixels(height / content_scale * scale)),
    })
  }
}

const fn pixels(length: f64) -> usize {
  length.round().max(0.) as usize
}

fn configuration(size: Size) -> Retained<SCStreamConfiguration> {
  // SAFETY: plain setters on a new configuration.
  unsafe {
    let configuration = SCStreamConfiguration::new();
    configuration.setWidth(size.0);
    configuration.setHeight(size.1);
    configuration.setPixelFormat(kCVPixelFormatType_32BGRA);
    configuration.setMinimumFrameInterval(CMTime::new(1, FRAMES_PER_SECOND));
    configuration.setShowsCursor(true);
    configuration
  }
}

/// The top left `shown` pixels of a BGRA frame as a base64 JPEG.
fn jpeg(buffer: &CVPixelBuffer, shown: Size) -> Option<String> {
  let read_only = CVPixelBufferLockFlags::ReadOnly;
  // SAFETY: locked for reading here and unlocked below, with the same flags.
  if unsafe { CVPixelBufferLockBaseAddress(buffer, read_only) } != 0 {
    return None;
  }
  let rgb = rgb(buffer, shown);
  // SAFETY: the lock taken above.
  unsafe { CVPixelBufferUnlockBaseAddress(buffer, read_only) };
  let (rgb, (width, height)) = rgb?;
  let mut jpeg = Vec::with_capacity(rgb.len() / 8);
  JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
    .encode(
      &rgb,
      u32::try_from(width).ok()?,
      u32::try_from(height).ok()?,
      ExtendedColorType::Rgb8,
    )
    .ok()?;
  Some(base64::engine::general_purpose::STANDARD.encode(jpeg))
}

/// The pixels of a locked BGRA frame without their alpha, and the size they make up.
fn rgb(buffer: &CVPixelBuffer, shown: Size) -> Option<(Vec<u8>, Size)> {
  let width = shown.0.min(CVPixelBufferGetWidth(buffer));
  let height = shown.1.min(CVPixelBufferGetHeight(buffer));
  let stride = CVPixelBufferGetBytesPerRow(buffer);
  let base = CVPixelBufferGetBaseAddress(buffer).cast::<u8>();
  if base.is_null() || width == 0 || height == 0 || stride < width * 4 {
    return None;
  }
  // SAFETY: a locked pixel buffer holds `stride` bytes for each of its rows, and `height` is within them.
  let bytes = unsafe { std::slice::from_raw_parts(base, stride * height) };
  let mut rgb = Vec::with_capacity(width * height * 3);
  for row in bytes.chunks_exact(stride) {
    for &[blue, green, red, _] in row[..width * 4].as_chunks::<4>().0 {
      rgb.extend_from_slice(&[red, green, blue]);
    }
  }
  Some((rgb, (width, height)))
}

/// A screen share from the picker to its end. Dropping it closes the picker and stops the capture.
pub struct Share {
  listener: Retained<Listener>,
}

impl Share {
  /// Opens the macOS picker for a window or a display. Frames of what is picked go to `frames`, one each time
  /// the picture changes; `end` hears when the share is over without having been dropped.
  pub fn pick(frames: flume::Sender<Frame>, end: flume::Sender<End>) -> Self {
    let session = Session {
      frames,
      end,
      queue: DispatchQueue::new("dev.felipefdl.working.screen", None),
      capture: Mutex::new(None),
    };
    let listener: Retained<Listener> = {
      let this = Listener::alloc().set_ivars(session);
      // SAFETY: plain NSObject initializer.
      unsafe { msg_send![super(this), init] }
    };
    // SAFETY: setters and calls of the picker with objects of the classes they declare.
    unsafe {
      let picker = SCContentSharingPicker::sharedPicker();
      let configuration = SCContentSharingPickerConfiguration::new();
      configuration
        .setAllowedPickerModes(SCContentSharingPickerMode::SingleWindow | SCContentSharingPickerMode::SingleDisplay);
      picker.setDefaultConfiguration(&configuration);
      picker.setMaximumStreamCount(Some(&NSNumber::new_isize(1)));
      picker.addObserver(ProtocolObject::from_ref(&*listener));
      picker.setActive(true);
      picker.present();
    }
    Self { listener }
  }
}

impl Drop for Share {
  fn drop(&mut self) {
    // SAFETY: calls of the picker and the stream with objects of the classes they declare.
    unsafe {
      let picker = SCContentSharingPicker::sharedPicker();
      picker.removeObserver(ProtocolObject::from_ref(&*self.listener));
      picker.setActive(false);
      // The stream holds the listener as its output: taking it out here lets both go.
      if let Ok(mut capture) = self.listener.ivars().capture.lock()
        && let Some(running) = capture.take()
      {
        running.stream.stopCaptureWithCompletionHandler(None);
      }
    }
  }
}
