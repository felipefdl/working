//! macOS: the hour format of the current locale, which carries the 24-hour switch of System Settings (General, Date
//! & Time) as well as the region's own choice.
//!
//! The `j` skeleton asks the locale for its preferred hour field: the pattern it answers has an `a` (the AM/PM
//! marker) only when hours show in 12-hour form.

use objc2_foundation::{NSDateFormatter, NSLocale, NSString};

/// Whether the user's locale writes the hour in 24-hour form; 12-hour when the locale gives no pattern.
pub fn is_24_hour() -> bool {
  let template = NSString::from_str("j");
  let locale = NSLocale::currentLocale();
  NSDateFormatter::dateFormatFromTemplate_options_locale(&template, 0, Some(&locale))
    .is_some_and(|pattern| !pattern.to_string().contains('a'))
}
