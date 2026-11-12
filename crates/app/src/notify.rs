//! macOS notifications for incoming messages, and the unread count on the Dock icon.
#![allow(unsafe_code, reason = "UserNotifications FFI")]
//!
//! Each notification carries its conversation's key as the thread identifier: macOS groups a conversation's
//! notifications by it, and a click sends it back so the app can open that conversation.

use std::ptr::NonNull;
use std::sync::LazyLock;

use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_foundation::{NSArray, NSError, NSString};

use crate::settings::Sound;
use objc2_user_notifications::{
  UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
  UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse, UNNotificationSettings,
  UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

/// Whether notifications are available: macOS only gives a notification center to an app bundle.
static ENABLED: LazyLock<bool> =
  LazyLock::new(|| std::env::current_exe().is_ok_and(|path| path.to_string_lossy().contains(".app/Contents/MacOS/")));

define_class!(
  #[unsafe(super(NSObject))]
  #[name = "WorkingNotificationDelegate"]
  #[ivars = flume::Sender<String>]
  struct Delegate;

  unsafe impl NSObjectProtocol for Delegate {}

  unsafe impl UNUserNotificationCenterDelegate for Delegate {
    /// Shown also while the app is in front: the store already skips the conversation being looked at.
    #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
    fn will_present(
      &self,
      _center: &UNUserNotificationCenter,
      _notification: &UNNotification,
      done: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
    ) {
      done.call((UNNotificationPresentationOptions::Banner
        | UNNotificationPresentationOptions::List
        | UNNotificationPresentationOptions::Sound,));
    }

    #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
    fn did_receive(&self, _center: &UNUserNotificationCenter, response: &UNNotificationResponse, done: &DynBlock<dyn Fn()>) {
      let key = response.notification().request().content().threadIdentifier().to_string();
      let _ = self.ivars().send(key);
      done.call(());
    }
  }
);

/// Asks for permission (macOS prompts once) and starts listening for clicks. The receiver yields the
/// conversation key of each clicked notification; it closes at once when notifications are unavailable.
pub fn start() -> flume::Receiver<String> {
  let (clicked, clicks) = flume::unbounded();
  if !*ENABLED {
    return clicks;
  }
  let delegate: Retained<Delegate> = {
    let this = Delegate::alloc().set_ivars(clicked);
    // SAFETY: plain NSObject initializer.
    unsafe { msg_send![super(this), init] }
  };
  let center = UNUserNotificationCenter::currentNotificationCenter();
  center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
  // The center holds its delegate weakly, and this one lives as long as the app.
  std::mem::forget(delegate);
  request();
  clicks
}

/// Asks macOS to allow notifications; the prompt only shows the first time.
pub fn request() {
  if !*ENABLED {
    return;
  }
  let options = UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound | UNAuthorizationOptions::Badge;
  UNUserNotificationCenter::currentNotificationCenter().requestAuthorizationWithOptions_completionHandler(
    options,
    &RcBlock::new(|_granted: Bool, _error: *mut NSError| {}),
  );
}

/// Whether the user allows notifications; `None` while macOS has not asked yet. The channel closes without
/// an answer when notifications are unavailable.
pub fn authorization() -> flume::Receiver<Option<bool>> {
  let (answer, answers) = flume::bounded(1);
  if !*ENABLED {
    return answers;
  }
  let read = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
    // SAFETY: the center hands a live settings object to its completion handler.
    let status = unsafe { settings.as_ref() }.authorizationStatus();
    let allowed = if status == UNAuthorizationStatus::NotDetermined {
      None
    } else {
      Some(status != UNAuthorizationStatus::Denied)
    };
    let _ = answer.send(allowed);
  });
  UNUserNotificationCenter::currentNotificationCenter().getNotificationSettingsWithCompletionHandler(&read);
  answers
}

/// Posts a notification. `id` is the message id, so an edit replaces what is shown; `thread` is the
/// conversation key.
pub fn post(id: &str, thread: &str, title: &str, subtitle: Option<&str>, body: &str, sound: Sound) {
  if !*ENABLED {
    return;
  }
  let content = UNMutableNotificationContent::new();
  content.setTitle(&NSString::from_str(title));
  if let Some(subtitle) = subtitle {
    content.setSubtitle(&NSString::from_str(subtitle));
  }
  content.setBody(&NSString::from_str(body));
  content.setThreadIdentifier(&NSString::from_str(thread));
  match sound {
    Sound::None => {},
    Sound::Default => content.setSound(Some(&UNNotificationSound::defaultSound())),
    named => content.setSound(
      named
        .file()
        .map(|file| UNNotificationSound::soundNamed(&NSString::from_str(&file)))
        .as_deref(),
    ),
  }
  let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(id), &content, None);
  UNUserNotificationCenter::currentNotificationCenter().addNotificationRequest_withCompletionHandler(&request, None);
}

/// Takes delivered notifications out of Notification Center, by the ids they were posted with.
pub fn clear(ids: &[String]) {
  if *ENABLED {
    let ids: Vec<_> = ids.iter().map(|id| NSString::from_str(id)).collect();
    UNUserNotificationCenter::currentNotificationCenter()
      .removeDeliveredNotificationsWithIdentifiers(&NSArray::from_retained_slice(&ids));
  }
}

/// Sets the number on the Dock icon; zero clears it.
pub fn set_badge(count: usize) {
  if *ENABLED {
    UNUserNotificationCenter::currentNotificationCenter().setBadgeCount_withCompletionHandler(count as isize, None);
  }
}
