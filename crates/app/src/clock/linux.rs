//! Linux: the desktop's clock format through the XDG settings portal, else the locale's territory.
//!
//! GNOME and the desktops built on its settings keep the choice in `org.gnome.desktop.interface` `clock-format`
//! (`'24h'` or `'12h'`), which `org.freedesktop.portal.Settings` hands out. The portal call runs on a thread of its
//! own and is given up after [`PORTAL_WAIT`], so a missing or slow portal never holds the UI thread long and never
//! runs inside GPUI. When the portal has no answer (no session bus, no portal, a desktop without that key), the
//! locale decides: a few territories write 12-hour times, the rest 24-hour, and an unset, `C`, or `POSIX` locale
//! 12-hour.

use std::time::Duration;

use zbus::{Connection, OwnedVariant, Variant};

const PORTAL_NAME: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.gnome.desktop.interface";
const KEY: &str = "clock-format";

/// How long the first caller waits for the portal before the locale decides.
const PORTAL_WAIT: Duration = Duration::from_millis(250);

/// Territories whose locales write the hour in 12-hour form.
const TWELVE_HOUR_TERRITORIES: &[&str] = &[
  "AU", "BD", "CA", "EG", "IN", "JO", "KR", "MY", "NZ", "PH", "PK", "SA", "TW", "US",
];

/// Whether the desktop, else the locale, shows the hour in 24-hour form.
pub fn is_24_hour() -> bool {
  portal().unwrap_or_else(|| locale_is_24_hour(&locale()))
}

/// The portal's `clock-format`, when it answers in time.
fn portal() -> Option<bool> {
  let (sender, receiver) = flume::bounded(1);
  let spawned = std::thread::Builder::new().name("clock-format".into()).spawn(move || {
    let _ = sender.send(zbus::block_on(read_clock_format()));
  });
  if let Err(error) = spawned {
    tracing::warn!("could not ask the settings portal for the clock format: {error}");
    return None;
  }
  match receiver.recv_timeout(PORTAL_WAIT).ok().flatten()?.as_str() {
    "24h" => Some(true),
    "12h" => Some(false),
    _ => None,
  }
}

/// Reads `clock-format` from the settings portal: `ReadOne` where the portal has it (version 2), else the older
/// `Read`, whose answer is wrapped in one more variant.
async fn read_clock_format() -> Option<String> {
  let connection = Connection::session().await.ok()?;
  for method in ["ReadOne", "Read"] {
    let reply = connection
      .call_method(Some(PORTAL_NAME), PORTAL_PATH, Some(SETTINGS), method, &(NAMESPACE, KEY))
      .await;
    if let Ok(reply) = reply {
      let value = reply.body().deserialize::<OwnedVariant>().ok()?;
      return text(&value).map(str::to_owned);
    }
  }
  None
}

/// The string inside a value, through any variants around it.
fn text<'a>(value: &'a Variant<'_>) -> Option<&'a str> {
  match value {
    Variant::Variant(inner) => text(inner),
    other => <&str>::try_from(other).ok(),
  }
}

/// The locale that sets the time format: the first set of `LC_ALL`, `LC_TIME`, and `LANG`.
fn locale() -> String {
  ["LC_ALL", "LC_TIME", "LANG"]
    .into_iter()
    .filter_map(|name| std::env::var(name).ok())
    .find(|value| !value.is_empty())
    .unwrap_or_default()
}

/// Whether a POSIX locale name (`language[_TERRITORY][.codeset][@modifier]`) writes the hour in 24-hour form. An
/// unset, `C`, or `POSIX` locale reads as 12-hour, as does a locale of [`TWELVE_HOUR_TERRITORIES`]; any other locale
/// reads as 24-hour.
fn locale_is_24_hour(locale: &str) -> bool {
  let name = locale.split(['.', '@']).next().unwrap_or_default();
  if name.is_empty() || name == "C" || name == "POSIX" {
    return false;
  }
  let territory = name.split_once('_').map_or("", |(_, territory)| territory);
  !TWELVE_HOUR_TERRITORIES.contains(&territory)
}

#[cfg(test)]
mod tests {
  use super::locale_is_24_hour;

  #[test]
  fn twelve_hour_territories() {
    assert!(!locale_is_24_hour("en_US.UTF-8"));
    assert!(!locale_is_24_hour("en_AU.UTF-8"));
    assert!(!locale_is_24_hour("en_IN"));
    assert!(!locale_is_24_hour("ko_KR.UTF-8"));
  }

  #[test]
  fn other_territories_are_24_hour() {
    assert!(locale_is_24_hour("en_GB.UTF-8"));
    assert!(locale_is_24_hour("pt_BR.UTF-8"));
    assert!(locale_is_24_hour("de_DE.UTF-8@euro"));
    assert!(locale_is_24_hour("fr_FR"));
  }

  #[test]
  fn unset_and_posix_are_12_hour() {
    assert!(!locale_is_24_hour(""));
    assert!(!locale_is_24_hour("C"));
    assert!(!locale_is_24_hour("C.UTF-8"));
    assert!(!locale_is_24_hour("POSIX"));
  }
}
