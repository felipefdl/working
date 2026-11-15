//! Linux: freedesktop desktop notifications over D-Bus, and the unread count through the launcher entry signal.
//!
//! One thread owns the session-bus connection and every notification the app has shown, so posting never waits
//! on the bus from the UI thread. The notification server knows a notification by the number it answered; the
//! thread keeps which message and which conversation each number stands for, so a later post of the same message
//! replaces it, `clear` closes it, and a click hands back its conversation. The freedesktop specification groups
//! nothing, so notifications are not grouped by conversation.
//!
//! The count on the app icon goes out as `com.canonical.Unity.LauncherEntry`'s `Update` signal, which KDE Plasma's
//! task manager, Ubuntu Dock, Dash to Dock, and Plank show on the launcher of the app's desktop entry. A desktop
//! without such a dock ignores it.

use std::collections::HashMap;
use std::sync::OnceLock;

use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::Value;

use crate::settings::Sound;

/// The app's desktop entry, `working.desktop`, without its suffix: the notification's `desktop-entry` hint, the
/// icon name, and the launcher the count goes on.
const DESKTOP_ID: &str = "working";

const BUS_NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// The action a server invokes when the notification itself is clicked.
const DEFAULT_ACTION: &str = "default";

/// What the bus thread is asked to do.
enum Request {
  Post {
    id: String,
    thread: String,
    summary: String,
    body: String,
    sound: Sound,
  },
  Clear(Vec<String>),
  Badge(usize),
  /// The server reports a click on a notification's action.
  Invoked(u32, String),
  /// The server took a notification away.
  Closed(u32),
}

/// The bus thread's inbox, once `start` has connected.
static REQUESTS: OnceLock<flume::Sender<Request>> = OnceLock::new();

/// Connects to the session bus and starts listening for clicks. The receiver yields the conversation key of each
/// clicked notification; it closes at once when there is no session bus.
pub fn start() -> flume::Receiver<String> {
  let (clicked, clicks) = flume::unbounded();
  let connection = match Connection::session() {
    Ok(connection) => connection,
    Err(error) => {
      tracing::warn!("notifications are off: no session bus: {error}");
      return clicks;
    },
  };
  let (requests, inbox) = flume::unbounded();
  if REQUESTS.set(requests.clone()).is_err() {
    return clicks;
  }
  let signals = connection.clone();
  let listen = std::thread::Builder::new()
    .name("notify-signals".into())
    .spawn(move || listen(&signals, &requests));
  if let Err(error) = listen {
    tracing::warn!("notification clicks are off: {error}");
  }
  let serve = std::thread::Builder::new()
    .name("notify".into())
    .spawn(move || Server::new(connection, clicked).serve(&inbox));
  if let Err(error) = serve {
    tracing::warn!("notifications are off: {error}");
  }
  clicks
}

/// Freedesktop servers keep no permission: notifications are always allowed.
#[expect(
  clippy::missing_const_for_fn,
  reason = "the same signature on every OS, and `notify::request` cannot be const over macOS's"
)]
pub fn request() {}

/// Freedesktop servers keep no permission, so the channel closes without an answer.
pub fn authorization() -> flume::Receiver<Option<bool>> {
  flume::bounded(1).1
}

/// Posts a notification. `id` is the message id, so an edit replaces what is shown; `thread` is the
/// conversation key. The subtitle (the space) goes into the summary, which the specification has no second line
/// for.
pub fn post(id: &str, thread: &str, title: &str, subtitle: Option<&str>, body: &str, sound: Sound) {
  let summary = subtitle.map_or_else(|| title.to_owned(), |space| format!("{title} · {space}"));
  send(Request::Post {
    id: id.to_owned(),
    thread: thread.to_owned(),
    summary,
    body: body.to_owned(),
    sound,
  });
}

/// Closes shown notifications, by the ids they were posted with.
pub fn clear(ids: &[String]) {
  send(Request::Clear(ids.to_vec()));
}

/// Sets the count on the app's launcher; zero hides it.
pub fn set_badge(count: usize) {
  send(Request::Badge(count));
}

fn send(request: Request) {
  if let Some(requests) = REQUESTS.get() {
    let _ = requests.send(request);
  }
}

/// Forwards the server's click and close signals to the bus thread, for as long as the bus lasts.
fn listen(connection: &Connection, requests: &flume::Sender<Request>) {
  let rule = format!("type='signal',interface='{BUS_NAME}',path='{PATH}'");
  let signals = match MessageIterator::for_match_rule(rule.as_str(), connection, None) {
    Ok(signals) => signals,
    Err(error) => {
      tracing::warn!("notification clicks are off: {error}");
      return;
    },
  };
  for message in signals.flatten() {
    let header = message.header();
    let request = match header.member().map(zbus::names::MemberName::as_str) {
      Some("ActionInvoked") => message
        .body()
        .deserialize::<(u32, String)>()
        .ok()
        .map(|(id, action)| Request::Invoked(id, action)),
      Some("NotificationClosed") => message
        .body()
        .deserialize::<(u32, u32)>()
        .ok()
        .map(|(id, _reason)| Request::Closed(id)),
      _ => None,
    };
    if let Some(request) = request
      && requests.send(request).is_err()
    {
      return;
    }
  }
}

/// The bus thread's state.
struct Server {
  connection: Connection,
  clicked: flume::Sender<String>,
  /// Whether the server reads markup in the body, which then needs its `&`, `<`, and `>` escaped.
  markup: bool,
  /// The server's number for each shown message.
  shown: HashMap<String, u32>,
  /// The conversation of each shown notification, by the server's number.
  threads: HashMap<u32, String>,
}

impl Server {
  fn new(connection: Connection, clicked: flume::Sender<String>) -> Self {
    let capabilities = connection
      .call_method(Some(BUS_NAME), PATH, Some(BUS_NAME), "GetCapabilities", &())
      .and_then(|reply| reply.body().deserialize::<Vec<String>>());
    let markup = match capabilities {
      Ok(capabilities) => capabilities.iter().any(|capability| capability == "body-markup"),
      Err(error) => {
        tracing::warn!("no notification server answered: {error}");
        false
      },
    };
    Self {
      connection,
      clicked,
      markup,
      shown: HashMap::new(),
      threads: HashMap::new(),
    }
  }

  fn serve(mut self, inbox: &flume::Receiver<Request>) {
    for request in inbox {
      match request {
        Request::Post { id, thread, summary, body, sound } => self.post(id, thread, &summary, &body, sound),
        Request::Clear(ids) => self.clear(&ids),
        Request::Badge(count) => self.badge(count),
        Request::Invoked(number, action) => {
          if action == DEFAULT_ACTION
            && let Some(thread) = self.threads.get(&number)
          {
            let _ = self.clicked.send(thread.clone());
          }
        },
        Request::Closed(number) => self.forget(number),
      }
    }
  }

  fn post(&mut self, id: String, thread: String, summary: &str, body: &str, sound: Sound) {
    let replaces = self.shown.get(&id).copied().unwrap_or(0);
    let body = if self.markup {
      escape(body)
    } else {
      body.to_owned()
    };
    let mut hints: HashMap<&str, Value<'_>> = HashMap::from([
      ("desktop-entry", Value::from(DESKTOP_ID)),
      ("category", Value::from("im.received")),
    ]);
    match sound {
      Sound::None => {
        hints.insert("suppress-sound", Value::from(true));
      },
      // The sound theme's own name for an incoming instant message, which the desktop maps to its sound.
      _ => {
        hints.insert("sound-name", Value::from("message-new-instant"));
      },
    }
    let actions = [DEFAULT_ACTION, "Open"];
    let shown = self
      .connection
      .call_method(
        Some(BUS_NAME),
        PATH,
        Some(BUS_NAME),
        "Notify",
        &(
          "Working",
          replaces,
          DESKTOP_ID,
          summary,
          body.as_str(),
          &actions[..],
          hints,
          -1_i32,
        ),
      )
      .and_then(|reply| reply.body().deserialize::<u32>());
    match shown {
      Ok(number) => {
        if replaces != 0 && replaces != number {
          self.threads.remove(&replaces);
        }
        self.shown.insert(id, number);
        self.threads.insert(number, thread);
      },
      Err(error) => tracing::warn!("posting a notification failed: {error}"),
    }
  }

  fn clear(&mut self, ids: &[String]) {
    for id in ids {
      let Some(number) = self.shown.remove(id) else {
        continue;
      };
      self.threads.remove(&number);
      if let Err(error) =
        self
          .connection
          .call_method(Some(BUS_NAME), PATH, Some(BUS_NAME), "CloseNotification", &number)
      {
        tracing::debug!("closing a notification failed: {error}");
      }
    }
  }

  fn forget(&mut self, number: u32) {
    if self.threads.remove(&number).is_some() {
      self.shown.retain(|_, shown| *shown != number);
    }
  }

  fn badge(&self, count: usize) {
    let properties: HashMap<&str, Value<'_>> = HashMap::from([
      ("count", Value::from(i64::try_from(count).unwrap_or(i64::MAX))),
      ("count-visible", Value::from(count > 0)),
    ]);
    let app = format!("application://{DESKTOP_ID}.desktop");
    let sent = self.connection.emit_signal(
      None::<&str>,
      format!("/com/canonical/unity/launcherentry/{DESKTOP_ID}").as_str(),
      "com.canonical.Unity.LauncherEntry",
      "Update",
      &(app.as_str(), properties),
    );
    if let Err(error) = sent {
      tracing::debug!("sending the unread count failed: {error}");
    }
  }
}

/// Escapes the characters the specification's body markup reads.
fn escape(text: &str) -> String {
  let mut escaped = String::with_capacity(text.len());
  for char in text.chars() {
    match char {
      '&' => escaped.push_str("&amp;"),
      '<' => escaped.push_str("&lt;"),
      '>' => escaped.push_str("&gt;"),
      other => escaped.push(other),
    }
  }
  escaped
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn escapes_markup_characters() {
    assert_eq!(escape("a <b> & c"), "a &lt;b&gt; &amp; c");
  }
}
