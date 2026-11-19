//! The app's key bindings, one table for every OS, and how a binding reads on this OS.
//!
//! Bindings are written with GPUI's `secondary-`: Cmd on macOS, Ctrl elsewhere, so the macOS keys stay Command keys
//! and the others take Ctrl. Only `KeyBinding::load` resolves `secondary-` (`KeyBinding::new` panics on it). A key
//! that means something else off macOS has a constant per OS below.

use std::rc::Rc;

use gpui_kit::component::input::Escape;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::*;

use crate::conversation::{FormatBold, FormatBulletedList, FormatItalic, FormatStrikethrough};
use crate::mail::{CloseMailThread, OpenMailLabels, OpenSelectedThread, SelectNextThread, SelectPreviousThread};
use crate::new_chat::OpenNewChat;
use crate::root::{
  FindInChat, FindNext, FindPrevious, GoBack, GoForward, NextConversation, NextUnread, OpenSettings, OpenSwitcher,
  OpenThemePicker, PreviousConversation, Quit, RewriteDraft, ToggleSidebar,
};

// What the number keys do: each opens the place `Settings::hotkeys` gives its number.
actions!(
  working,
  [Hotkey1, Hotkey2, Hotkey3, Hotkey4, Hotkey5, Hotkey6, Hotkey7, Hotkey8, Hotkey9]
);

/// The number keys, in order: the first is number 1.
fn hotkeys() -> [(&'static str, Box<dyn Action>); 9] {
  [
    ("secondary-1", Box::new(Hotkey1)),
    ("secondary-2", Box::new(Hotkey2)),
    ("secondary-3", Box::new(Hotkey3)),
    ("secondary-4", Box::new(Hotkey4)),
    ("secondary-5", Box::new(Hotkey5)),
    ("secondary-6", Box::new(Hotkey6)),
    ("secondary-7", Box::new(Hotkey7)),
    ("secondary-8", Box::new(Hotkey8)),
    ("secondary-9", Box::new(Hotkey9)),
  ]
}

/// The numbers of the number keys.
pub const HOTKEY_SLOTS: std::ops::RangeInclusive<u8> = 1..=9;

/// The binding of number key `slot`, for `text`.
pub fn hotkey_keys(slot: u8) -> Option<&'static str> {
  hotkeys().get(usize::from(slot).checked_sub(1)?).map(|(keys, _)| *keys)
}

/// The action of number key `slot`: a menu item or tooltip given it shows the key.
pub fn hotkey_action(slot: u8) -> Option<Box<dyn Action>> {
  hotkeys()
    .into_iter()
    .nth(usize::from(slot).checked_sub(1)?)
    .map(|(_, action)| action)
}

/// The next conversation in the order the layout lists them. Option-Command arrows on macOS, as Mail; elsewhere
/// Ctrl+Alt+arrows switch workspaces or rotate the screen, so Alt+arrows, as Slack.
#[cfg(target_os = "macos")]
pub const NEXT_CONVERSATION: &str = "alt-cmd-down";
#[cfg(not(target_os = "macos"))]
pub const NEXT_CONVERSATION: &str = "alt-down";

/// The previous conversation; see [`NEXT_CONVERSATION`].
#[cfg(target_os = "macos")]
const PREVIOUS_CONVERSATION: &str = "alt-cmd-up";
#[cfg(not(target_os = "macos"))]
const PREVIOUS_CONVERSATION: &str = "alt-up";

/// The next conversation with unread messages; see [`NEXT_CONVERSATION`].
#[cfg(target_os = "macos")]
const NEXT_UNREAD: &str = "shift-alt-cmd-down";
#[cfg(not(target_os = "macos"))]
const NEXT_UNREAD: &str = "shift-alt-down";

/// Shows or hides the sidebar. Control-Command-S on macOS; `secondary-ctrl-s` would be plain Ctrl+S elsewhere, so
/// Ctrl+Shift+S there.
#[cfg(target_os = "macos")]
const TOGGLE_SIDEBAR: &str = "ctrl-cmd-s";
#[cfg(not(target_os = "macos"))]
const TOGGLE_SIDEBAR: &str = "ctrl-shift-s";

/// Opens the Mail view's labels popover (`mail_labels.rs`), in the Mail context.
pub const MAIL_LABELS: &str = "secondary-l";

/// This OS, for spelling a `secondary-` binding.
#[cfg(target_os = "macos")]
const OS: OperatingSystem = OperatingSystem::Mac;
#[cfg(target_os = "linux")]
const OS: OperatingSystem = OperatingSystem::Linux;
#[cfg(target_os = "windows")]
const OS: OperatingSystem = OperatingSystem::Windows;

/// Every binding: keys, action, and the key context it is live in.
fn table() -> Vec<(&'static str, Box<dyn Action>, Option<&'static str>)> {
  let mut table: Vec<(&'static str, Box<dyn Action>, Option<&'static str>)> = vec![
    ("secondary-k", Box::new(OpenSwitcher), None),
    ("secondary-shift-t", Box::new(OpenThemePicker), None),
    ("secondary-n", Box::new(OpenNewChat), None),
    // Safari's pair. The kit's text field binds the same keys to outdent and indent, which a chat composer has no
    // use for: binding them again in its "Input" context, after `init`, takes them back.
    ("secondary-[", Box::new(GoBack), None),
    ("secondary-]", Box::new(GoForward), None),
    ("secondary-[", Box::new(GoBack), Some("Input")),
    ("secondary-]", Box::new(GoForward), Some("Input")),
    // Between conversations, in the order the layout lists them. The Go menu shows the first key of each. Ctrl+Tab
    // is Control on every OS: Command-Tab is the macOS app switcher.
    (NEXT_CONVERSATION, Box::new(NextConversation), None),
    (PREVIOUS_CONVERSATION, Box::new(PreviousConversation), None),
    ("ctrl-tab", Box::new(NextConversation), None),
    ("ctrl-shift-tab", Box::new(PreviousConversation), None),
    (NEXT_UNREAD, Box::new(NextUnread), None),
    (TOGGLE_SIDEBAR, Box::new(ToggleSidebar), None),
    // The kit's text field takes the conversation keys to add a cursor (alt-cmd on macOS, shift-alt on Linux):
    // binding them again in its "Input" context, after `init`, takes them back, and the other conversation keys
    // work in a composer too.
    (NEXT_CONVERSATION, Box::new(NextConversation), Some("Input")),
    (PREVIOUS_CONVERSATION, Box::new(PreviousConversation), Some("Input")),
    ("ctrl-tab", Box::new(NextConversation), Some("Input")),
    ("ctrl-shift-tab", Box::new(PreviousConversation), Some("Input")),
    (NEXT_UNREAD, Box::new(NextUnread), Some("Input")),
    // The auto-hide sidebar's overlay is open (the root takes the "Overlay" context): Escape closes it even when no
    // text field has the keyboard.
    ("escape", Box::new(Escape), Some("Overlay")),
    // The event editor closes on Escape from any of its controls, not only its text fields.
    ("escape", Box::new(Escape), Some("EventEditor")),
    // The Mail view's list: the arrows move the selection, Enter opens it, Escape goes back to the list. Its search
    // field is a text field, whose "Input" context is deeper and keeps these keys for itself.
    ("down", Box::new(SelectNextThread), Some("Mail")),
    ("up", Box::new(SelectPreviousThread), Some("Mail")),
    ("enter", Box::new(OpenSelectedThread), Some("Mail")),
    ("escape", Box::new(CloseMailThread), Some("Mail")),
    // The labels popover of the Mail view; the search field keeps no Cmd or Ctrl letter of its own, so it works
    // from there too.
    (MAIL_LABELS, Box::new(OpenMailLabels), Some("Mail")),
    ("secondary-,", Box::new(OpenSettings), None),
    ("secondary-shift-r", Box::new(RewriteDraft), None),
    // Formatting a draft, with Google Chat's keys. In the kit's "Input" context, so they are live while a text field
    // has the keyboard; the conversation takes them only from its composers. Every OS delivers Shift+8 as the key
    // `*` with the shift folded into it, so that is how the bulleted list's key is written.
    ("secondary-b", Box::new(FormatBold), Some("Input")),
    ("secondary-i", Box::new(FormatItalic), Some("Input")),
    ("secondary-shift-x", Box::new(FormatStrikethrough), Some("Input")),
    ("secondary-*", Box::new(FormatBulletedList), Some("Input")),
    ("secondary-f", Box::new(FindInChat), None),
    ("secondary-g", Box::new(FindNext), None),
    ("secondary-shift-g", Box::new(FindPrevious), None),
    ("secondary-q", Box::new(Quit), None),
  ];
  table.extend(hotkeys().into_iter().map(|(keys, action)| (keys, action, None)));
  table
}

/// Binds every key of the table, and on macOS the app and window keys of its menu bar. Call after `gpui_kit::init`,
/// so the "Input" bindings here win over the kit's.
pub fn bind(cx: &mut App) {
  let bindings: Vec<KeyBinding> = table()
    .into_iter()
    .filter_map(|(keys, action, context)| binding(keys, action, context, cx))
    .collect();
  cx.bind_keys(bindings);
  // Only macOS hides apps and has a Window menu; elsewhere the title bar has the window's buttons.
  #[cfg(target_os = "macos")]
  cx.bind_keys([
    KeyBinding::new("cmd-h", crate::root::Hide, None),
    KeyBinding::new("alt-cmd-h", crate::root::HideOthers, None),
    KeyBinding::new("cmd-m", crate::root::Minimize, None),
  ]);
}

fn binding(keys: &str, action: Box<dyn Action>, context: Option<&str>, cx: &App) -> Option<KeyBinding> {
  let predicate = match context.map(KeyBindingContextPredicate::parse).transpose() {
    Ok(predicate) => predicate.map(Rc::new),
    Err(error) => {
      tracing::error!("key context {context:?} of {keys}: {error}");
      return None;
    },
  };
  KeyBinding::load(keys, action, predicate, false, None, cx)
    .inspect_err(|error| tracing::error!("key binding {keys}: {error}"))
    .ok()
}

/// How `binding` reads on this OS, as the kit's key labels write it: `⇧⌘X` on macOS, `Ctrl+Shift+X` elsewhere.
pub fn text(binding: &str) -> String {
  Keystroke::parse_with_os(binding, OS).map_or_else(|_| binding.to_string(), |keystroke| Kbd::format(&keystroke))
}

#[cfg(test)]
mod tests {
  use gpui_kit::{KeyBindingContextPredicate, Keystroke, OperatingSystem};

  use super::{NEXT_CONVERSATION, table, text};

  #[test]
  fn every_binding_parses_on_every_os() {
    for (keys, _, context) in table() {
      for os in [OperatingSystem::Mac, OperatingSystem::Linux, OperatingSystem::Windows] {
        assert!(Keystroke::parse_with_os(keys, os).is_ok(), "{keys} on {os:?}");
      }
      if let Some(context) = context {
        assert!(KeyBindingContextPredicate::parse(context).is_ok(), "{context}");
      }
    }
  }

  #[test]
  fn secondary_is_command_on_macos_and_control_elsewhere() {
    let bold = Keystroke::parse_with_os("secondary-b", OperatingSystem::Mac).unwrap();
    assert!(bold.modifiers.platform && !bold.modifiers.control);
    for os in [OperatingSystem::Linux, OperatingSystem::Windows] {
      let bold = Keystroke::parse_with_os("secondary-b", os).unwrap();
      assert!(bold.modifiers.control && !bold.modifiers.platform);
    }
  }

  #[test]
  fn text_follows_this_os() {
    #[cfg(target_os = "macos")]
    {
      assert_eq!(text("secondary-shift-x"), "⇧⌘X");
      assert_eq!(text(NEXT_CONVERSATION), "⌥⌘↓");
    }
    #[cfg(not(target_os = "macos"))]
    {
      assert_eq!(text("secondary-shift-x"), "Ctrl+Shift+X");
      assert_eq!(text(NEXT_CONVERSATION), "Alt+Down");
    }
  }
}
