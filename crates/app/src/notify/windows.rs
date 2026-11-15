//! Windows: toast notifications through `ToastNotificationManager`, under the app's own `AppUserModelID`.
#![allow(unsafe_code, reason = "AppUserModelID and WinRT initialization FFI")]
//!
//! A toast's tag is the message id, so posting the same id replaces it and `clear` finds it, and its group is the
//! conversation key, by which Action Center keeps a conversation's toasts together. The key is also the toast's
//! launch argument, which a click hands back. A toast from an unpackaged app shows only under an
//! `AppUserModelID` that a Start Menu shortcut carries: [`set_app_id`] gives the process the id the installer's
//! shortcut has. Windows has no count on a taskbar icon, so [`set_badge`] shows nothing.
//!
//! Every toast call runs on a thread of its own: from GPUI's single-threaded apartment, a call into the
//! notification platform pumps the window's messages while it waits, which re-enters GPUI while the app is
//! borrowed.

use std::sync::{LazyLock, Mutex};

use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{
  NotificationSetting, ToastActivatedEventArgs, ToastNotification, ToastNotificationManager,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::{HSTRING, IInspectable, Interface, Ref};

use crate::settings::Sound;

/// The app's `AppUserModelID`, also on the installer's Start Menu shortcut.
pub const APP_ID: &str = "dev.felipefdl.working";

/// Where a click on a toast goes: the sender of the receiver the latest [`start`] answered.
static CLICKED: Mutex<Option<flume::Sender<String>>> = Mutex::new(None);

/// What the toast thread does.
enum Job {
  Post {
    id: String,
    thread: String,
    content: String,
    clicked: Option<flume::Sender<String>>,
  },
  Clear(Vec<String>),
  Authorization(flume::Sender<Option<bool>>),
}

/// The toast thread, started on first use; `None` when it could not start.
static JOBS: LazyLock<Option<flume::Sender<Job>>> = LazyLock::new(|| {
  let (jobs, received) = flume::unbounded();
  let started = std::thread::Builder::new().name("toasts".into()).spawn(move || {
    // SAFETY: initializes WinRT once, on this thread, before any WinRT call on it.
    if let Err(error) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
      tracing::warn!("could not initialize WinRT for notifications: {error}");
      return;
    }
    for job in received {
      run(job);
    }
  });
  match started {
    Ok(_) => Some(jobs),
    Err(error) => {
      tracing::warn!("could not start the notification thread: {error}");
      None
    },
  }
});

fn send(job: Job) {
  if let Some(jobs) = &*JOBS {
    let _ = jobs.send(job);
  }
}

fn run(job: Job) {
  match job {
    Job::Post { id, thread, content, clicked } => {
      if let Err(error) = show(&id, &thread, &content, clicked) {
        tracing::warn!("could not show a notification: {error}");
      }
    },
    Job::Clear(ids) => {
      if let Err(error) = remove(&ids) {
        tracing::debug!("could not clear notifications: {error}");
      }
    },
    Job::Authorization(answer) => {
      match ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))
        .and_then(|notifier| notifier.Setting())
      {
        Ok(setting) => {
          let _ = answer.send(Some(setting == NotificationSetting::Enabled));
        },
        Err(error) => tracing::debug!("no toast setting for the app: {error}"),
      }
    },
  }
}

/// Gives the process the app's `AppUserModelID`. Windows reads it when a window first shows, so it runs before any.
pub fn set_app_id() {
  let id = HSTRING::from(APP_ID);
  // SAFETY: a valid null-terminated wide string that outlives the call.
  let result = unsafe { windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(id.as_ptr()) };
  if result < 0 {
    tracing::warn!("could not set the app's AppUserModelID: {result:#x}");
  }
}

/// Starts listening for clicks; Windows asks for no permission. The receiver yields the conversation key of each
/// clicked toast.
pub fn start() -> flume::Receiver<String> {
  let (clicked, clicks) = flume::unbounded();
  if let Ok(mut sender) = CLICKED.lock() {
    *sender = Some(clicked);
  }
  clicks
}

/// Windows shows toasts without asking: the user turns them off in Settings, Notifications.
#[expect(
  clippy::missing_const_for_fn,
  reason = "the signature every OS's notify module shares"
)]
pub fn request() {}

/// Whether the user allows the app's toasts. The channel closes without an answer when Windows does not know the
/// app (no Start Menu shortcut carries its id).
pub fn authorization() -> flume::Receiver<Option<bool>> {
  let (answer, answers) = flume::bounded(1);
  send(Job::Authorization(answer));
  answers
}

/// Posts a toast. `id` is the message id, so an edit replaces what is shown; `thread` is the conversation key.
pub fn post(id: &str, thread: &str, title: &str, subtitle: Option<&str>, body: &str, sound: Sound) {
  send(Job::Post {
    id: id.to_owned(),
    thread: thread.to_owned(),
    content: content(thread, title, subtitle, body, sound),
    clicked: CLICKED.lock().ok().and_then(|sender| sender.clone()),
  });
}

fn show(id: &str, thread: &str, content: &str, clicked: Option<flume::Sender<String>>) -> windows::core::Result<()> {
  let xml = XmlDocument::new()?;
  xml.LoadXml(&HSTRING::from(content))?;
  let toast = ToastNotification::CreateToastNotification(&xml)?;
  toast.SetTag(&HSTRING::from(id))?;
  toast.SetGroup(&HSTRING::from(thread))?;
  if let Some(clicked) = clicked {
    toast.Activated(&TypedEventHandler::new(
      move |_: Ref<'_, ToastNotification>, args: Ref<'_, IInspectable>| {
        let key = args.ok()?.cast::<ToastActivatedEventArgs>()?.Arguments()?;
        let _ = clicked.send(key.to_string_lossy());
        Ok(())
      },
    ))?;
  }
  ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?.Show(&toast)
}

/// The toast's XML: the sender as its title, the message, the conversation's name as its attribution line, and the
/// conversation key as its launch argument.
fn content(thread: &str, title: &str, subtitle: Option<&str>, body: &str, sound: Sound) -> String {
  let attribution = subtitle.map_or_else(String::new, |subtitle| {
    format!(r#"<text placement="attribution">{}</text>"#, escape(subtitle))
  });
  let audio = if sound.resolved() == Sound::None {
    r#"<audio silent="true"/>"#
  } else {
    ""
  };
  format!(
    r#"<toast launch="{}"><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text>{attribution}</binding></visual>{audio}</toast>"#,
    escape(thread),
    escape(title),
    escape(body),
  )
}

/// Text as XML character data or an attribute value.
fn escape(text: &str) -> String {
  let mut out = String::with_capacity(text.len());
  for c in text.chars() {
    match c {
      '&' => out.push_str("&amp;"),
      '<' => out.push_str("&lt;"),
      '>' => out.push_str("&gt;"),
      '"' => out.push_str("&quot;"),
      '\'' => out.push_str("&apos;"),
      // XML 1.0 has no other control characters, and the parser refuses the whole toast for one.
      c if c.is_control() && !matches!(c, '\n' | '\r' | '\t') => {},
      c => out.push(c),
    }
  }
  out
}

/// Takes delivered toasts out of Action Center, by the ids they were posted with.
pub fn clear(ids: &[String]) {
  send(Job::Clear(ids.to_vec()));
}

fn remove(ids: &[String]) -> windows::core::Result<()> {
  let app = HSTRING::from(APP_ID);
  let history = ToastNotificationManager::History()?;
  for toast in history.GetHistoryWithId(&app)? {
    let tag = toast.Tag()?.to_string_lossy();
    if ids.contains(&tag) {
      history.RemoveGroupedTagWithId(&HSTRING::from(tag), &toast.Group()?, &app)?;
    }
  }
  Ok(())
}

/// Windows offers no count on a taskbar icon: the unread count shows in the window only.
#[expect(
  clippy::missing_const_for_fn,
  reason = "the signature every OS's notify module shares"
)]
pub fn set_badge(_count: usize) {}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn content_escapes_text_and_marks_silence() {
    let xml = content("dm_a1", "Jordan Park", Some("Q&A"), "<b>hi</b> \"there\"\u{7}", Sound::None);
    assert_eq!(
      xml,
      concat!(
        r#"<toast launch="dm_a1"><visual><binding template="ToastGeneric"><text>Jordan Park</text>"#,
        r#"<text>&lt;b&gt;hi&lt;/b&gt; &quot;there&quot;</text><text placement="attribution">Q&amp;A</text>"#,
        r#"</binding></visual><audio silent="true"/></toast>"#,
      )
    );
  }

  #[test]
  fn content_keeps_the_default_sound() {
    let xml = content("space_b2", "Riley Brooks", None, "line one\nline two", Sound::Glass);
    assert!(!xml.contains("<audio"));
    assert!(!xml.contains("attribution"));
    assert!(xml.contains("<text>line one\nline two</text>"));
  }
}
