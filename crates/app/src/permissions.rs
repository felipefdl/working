//! What macOS lets the app use, for the checklist in Settings: read here, and asked for from there.
#![allow(unsafe_code, reason = "AVFoundation authorization FFI")]

use block2::RcBlock;
use objc2::msg_send;
use objc2::runtime::{AnyClass, Bool};
use objc2_foundation::NSString;

use crate::screen;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
  Allowed,
  Denied,
  /// macOS has not asked the user yet.
  NotAsked,
}

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}

#[derive(Clone, Copy)]
pub enum Media {
  Camera,
  Microphone,
}

impl Media {
  /// The `AVMediaType` value.
  const fn kind(self) -> &'static str {
    match self {
      Self::Camera => "vide",
      Self::Microphone => "soun",
    }
  }
}

pub fn media(media: Media) -> Status {
  let Some(class) = AnyClass::get(c"AVCaptureDevice") else {
    return Status::NotAsked;
  };
  let kind = NSString::from_str(media.kind());
  // SAFETY: a class method taking one NSString and returning an NSInteger enum.
  let status: isize = unsafe { msg_send![class, authorizationStatusForMediaType: &*kind] };
  match status {
    3 => Status::Allowed,
    0 => Status::NotAsked,
    _ => Status::Denied,
  }
}

/// Raises the macOS prompt for the camera or the microphone; it only shows while the status is `NotAsked`.
pub fn request_media(media: Media) {
  let Some(class) = AnyClass::get(c"AVCaptureDevice") else {
    return;
  };
  let kind = NSString::from_str(media.kind());
  let ignore = RcBlock::new(|_granted: Bool| {});
  // SAFETY: a class method taking one NSString and a block of the declared shape.
  unsafe {
    let _: () = msg_send![class, requestAccessForMediaType: &*kind, completionHandler: &*ignore];
  }
}

/// Screen recording. macOS does not tell "denied" from "not asked yet" here.
pub fn screen() -> Status {
  if screen::permitted() {
    Status::Allowed
  } else {
    Status::Denied
  }
}
