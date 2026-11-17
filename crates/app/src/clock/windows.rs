//! Windows: the user's short time format (Settings, Time & language, Region), which a custom format in Control Panel
//! overrides.
#![allow(unsafe_code, reason = "GetLocaleInfoEx FFI")]
//!
//! `LOCALE_STIMEFORMAT` is a pattern such as `h:mm:ss tt` or `HH:mm:ss`: `H` is the hour in 24-hour form, `h` in
//! 12-hour form. It is read rather than the legacy `LOCALE_ITIME`, which does not follow every custom format.

use windows_sys::Win32::Globalization::{GetLocaleInfoEx, LOCALE_STIMEFORMAT};

/// The longest `LOCALE_STIMEFORMAT` value Windows allows, with its terminating null.
const FORMAT_LEN: usize = 80;

/// Whether the user's time format writes the hour in 24-hour form; 12-hour when Windows gives no format.
pub fn is_24_hour() -> bool {
  let mut buffer = [0_u16; FORMAT_LEN];
  let capacity = i32::try_from(buffer.len()).unwrap_or(0);
  // SAFETY: a null locale name is `LOCALE_NAME_USER_DEFAULT`, and the buffer is writable for `capacity` wide chars.
  let written = unsafe { GetLocaleInfoEx(std::ptr::null(), LOCALE_STIMEFORMAT, buffer.as_mut_ptr(), capacity) };
  let Some(format) = usize::try_from(written)
    .ok()
    .and_then(|written| written.checked_sub(1))
    .and_then(|len| buffer.get(..len))
  else {
    tracing::warn!("could not read the time format; showing 12-hour times");
    return false;
  };
  format_is_24_hour(&String::from_utf16_lossy(format))
}

/// Whether a Windows time pattern writes the hour in 24-hour form: it has an `H` outside quoted literal text. A
/// pattern with no hour at all reads as 12-hour.
fn format_is_24_hour(format: &str) -> bool {
  let mut quoted = false;
  for character in format.chars() {
    match character {
      '\'' => quoted = !quoted,
      'H' if !quoted => return true,
      'h' if !quoted => return false,
      _ => {},
    }
  }
  false
}

#[cfg(test)]
mod tests {
  use super::format_is_24_hour;

  #[test]
  fn reads_the_hour_field() {
    assert!(format_is_24_hour("HH:mm:ss"));
    assert!(format_is_24_hour("H:mm"));
    assert!(!format_is_24_hour("h:mm:ss tt"));
    assert!(!format_is_24_hour("tt hh:mm"));
  }

  #[test]
  fn skips_quoted_text() {
    assert!(!format_is_24_hour("'H' h:mm tt"));
    assert!(format_is_24_hour("'h' HH:mm"));
  }

  #[test]
  fn no_hour_is_12_hour() {
    assert!(!format_is_24_hour(""));
    assert!(!format_is_24_hour("mm:ss"));
  }
}
