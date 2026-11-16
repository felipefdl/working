//! Linux: an `AppImage` run straight from a folder has no desktop entry installed, so docks and launchers show a
//! generic icon and the binary's name. As the `AppImage` specification asks, the app installs its own entry only with
//! the user's permission: the first start asks with a card in the main window, and Settings has a switch for it. The
//! answer is `Settings::desktop_entry`. Yes writes `applications/working.desktop` and the icon into the user's data
//! directory (`~/.local/share`), pointing at the `AppImage` the app runs from, and every start rewrites the entry once
//! that path changes (the `AppImage` was moved or replaced). Off removes those two files, never another one.
//!
//! The app neither asks nor writes when it is not an `AppImage`, when one of the specification's opt-outs holds (a
//! `no_desktopintegration` file, a running `appimaged`, `$DESKTOPINTEGRATION` set), or when an entry it did not write
//! is already there (an integration tool made one).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use crate::settings::Settings;
use crate::theme::{ACCENT, ACCENT_BG, BORDER, MUTED, POPOVER, ROW_HOVER, TEXT, rgb};

const ICON: &[u8] = include_bytes!("../../../logo/icon-light.png");
/// The entry and the icon, under the user's data directory.
const ENTRY: &str = "applications/working.desktop";
const ICON_FILE: &str = "icons/hicolor/512x512/apps/working.png";
/// The line that marks an entry this app wrote: one without it is never changed or removed.
const MARK: &str = "X-Working-Self-Integrated=true";
/// The opt-out files outside the user's data directory.
const SYSTEM_OPT_OUTS: [&str; 2] = [
  "/usr/share/appimagekit/no_desktopintegration",
  "/etc/appimagekit/no_desktopintegration",
];

/// The user's answer to adding the app to the apps menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
  /// Not asked yet: the main window asks.
  #[default]
  NotAsked,
  Yes,
  No,
}

/// What start-up found about this system.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Facts {
  /// One of the specification's opt-outs holds.
  opted_out: bool,
  /// An entry the app did not write is there.
  foreign: bool,
  /// The entry the app wrote is there.
  ours: bool,
}

impl Facts {
  /// The app may ask, and write.
  const fn offer(self) -> bool {
    !self.opted_out && !self.foreign
  }

  /// The Settings switch shows: the app may write, or has files of its own there to remove.
  const fn switch(self) -> bool {
    self.offer() || self.ours
  }
}

/// Whether the main window asks.
fn asks(choice: Choice, facts: Facts) -> bool {
  choice == Choice::NotAsked && facts.offer()
}

/// Whether start-up brings the entry up to date.
fn refreshes(choice: Choice, facts: Facts) -> bool {
  choice == Choice::Yes && facts.offer()
}

/// The running `AppImage` and what start-up found, set only when the app is one.
pub struct Integration {
  appimage: PathBuf,
  data: PathBuf,
  facts: Facts,
}

impl Global for Integration {}

impl Integration {
  /// Whether Settings shows the switch.
  pub const fn switch(&self) -> bool {
    self.facts.switch()
  }
}

/// Looks at the system once the settings are loaded, and with the entry wanted, brings it up to date.
pub fn start(cx: &mut App) {
  let Some(appimage) = std::env::var_os("APPIMAGE").map(PathBuf::from) else {
    return;
  };
  let Some(data) = dirs::data_dir() else {
    return;
  };
  let entry = std::fs::read_to_string(data.join(ENTRY)).ok();
  let mut facts = Facts {
    opted_out: opted_out(
      std::env::var_os("DESKTOPINTEGRATION").as_deref(),
      &data,
      Path::new("/proc"),
      Path::exists,
    ),
    foreign: foreign_entry(&data.join("applications"), &appimage),
    ours: entry.as_deref().is_some_and(marked),
  };
  if refreshes(cx.global::<Settings>().desktop_entry, facts) {
    match write(&data, &appimage) {
      Ok(()) => facts.ours = true,
      Err(error) => tracing::warn!("could not update the desktop entry: {error}"),
    }
  }
  cx.set_global(Integration { appimage, data, facts });
}

/// Writes or removes the entry and keeps the answer. The card and the Settings switch both end here. Under an opt-out
/// or beside an entry the app did not write, on writes nothing: the switch shows there only to remove the app's own
/// files.
pub fn set(on: bool, cx: &mut App) {
  if let Some(integration) = cx.try_global::<Integration>() {
    let facts = integration.facts;
    let result = if !on {
      remove(&integration.data).map(|()| false)
    } else if facts.offer() {
      write(&integration.data, &integration.appimage).map(|()| true)
    } else {
      Ok(facts.ours)
    };
    match result {
      Ok(ours) => cx.global_mut::<Integration>().facts.ours = ours,
      Err(error) => tracing::warn!("could not change the desktop entry: {error}"),
    }
  }
  Settings::update(cx, |settings| {
    settings.desktop_entry = if on { Choice::Yes } else { Choice::No };
  });
}

/// The specification's opt-outs: `$DESKTOPINTEGRATION` not empty, a `no_desktopintegration` file in
/// `$XDG_DATA_HOME/appimagekit`, `/usr/share/appimagekit`, or `/etc/appimagekit`, or a running `appimaged`.
fn opted_out(variable: Option<&OsStr>, data: &Path, proc: &Path, exists: impl Fn(&Path) -> bool) -> bool {
  variable.is_some_and(|value| !value.is_empty())
    || exists(&data.join("appimagekit/no_desktopintegration"))
    || SYSTEM_OPT_OUTS.iter().any(|path| exists(Path::new(path)))
    || appimaged_running(proc)
}

/// Whether a process named `appimaged` runs, from the `comm` of each process under `proc`. Run as an `AppImage`, its
/// runtime's name is the file's, such as `appimaged-x86_64.AppImage`, cut to 15 bytes.
fn appimaged_running(proc: &Path) -> bool {
  let Ok(entries) = std::fs::read_dir(proc) else {
    return false;
  };
  entries.flatten().any(|entry| {
    let pid = entry.file_name();
    pid.to_str().is_some_and(|pid| pid.bytes().all(|byte| byte.is_ascii_digit()))
      && std::fs::read_to_string(entry.path().join("comm")).is_ok_and(|comm| {
        let name = comm.trim_end();
        name == "appimaged" || name.starts_with("appimaged-") || name.starts_with("appimaged.")
      })
  })
}

/// Whether `applications` holds an entry the app did not write: `working.desktop`, or one whose `Exec` runs this
/// `AppImage` (as `appimaged`, `AppImageLauncher`, and Gear Lever write them).
fn foreign_entry(applications: &Path, appimage: &Path) -> bool {
  let Ok(entries) = std::fs::read_dir(applications) else {
    return false;
  };
  let appimage = appimage.to_string_lossy();
  entries.flatten().any(|entry| {
    let name = entry.file_name();
    if Path::new(&name).extension() != Some(OsStr::new("desktop")) {
      return false;
    }
    let Ok(text) = std::fs::read_to_string(entry.path()) else {
      return false;
    };
    !marked(&text)
      && (name == "working.desktop"
        || exec(&text).is_some_and(|args| args.iter().any(|arg| arg.as_str() == appimage.as_ref())))
  })
}

/// Whether an entry's text is one this app wrote.
fn marked(text: &str) -> bool {
  text.lines().any(|line| line.trim_end() == MARK)
}

/// The arguments of the `Exec` key in an entry's `[Desktop Entry]` group, unescaped and unquoted as the Desktop
/// Entry Specification sets out.
fn exec(text: &str) -> Option<Vec<String>> {
  let mut group = false;
  for line in text.lines() {
    let line = line.trim();
    if line.starts_with('[') {
      group = line == "[Desktop Entry]";
    } else if group
      && let Some((key, value)) = line.split_once('=')
      && key.trim_end() == "Exec"
    {
      return Some(arguments(&unescape(value.trim_start())));
    }
  }
  None
}

/// A string value with its escapes (`\s`, `\n`, `\t`, `\r`, `\\`) undone.
fn unescape(value: &str) -> String {
  let mut text = String::with_capacity(value.len());
  let mut chars = value.chars();
  while let Some(ch) = chars.next() {
    if ch != '\\' {
      text.push(ch);
      continue;
    }
    match chars.next() {
      Some('s') => text.push(' '),
      Some('n') => text.push('\n'),
      Some('t') => text.push('\t'),
      Some('r') => text.push('\r'),
      Some(other) => {
        // Not a string escape: left for the quoting rules.
        if other != '\\' {
          text.push('\\');
        }
        text.push(other);
      },
      None => text.push('\\'),
    }
  }
  text
}

/// An `Exec` value split into arguments: spaces separate them outside double quotes, a backslash inside them takes
/// the next character as it is, and `%%` is a percent sign.
fn arguments(value: &str) -> Vec<String> {
  let mut args = Vec::new();
  let mut arg = String::new();
  let mut started = false;
  let mut quoted = false;
  let mut chars = value.chars().peekable();
  while let Some(ch) = chars.next() {
    match ch {
      '"' => {
        quoted.toggle();
        started = true;
      },
      '\\' if quoted => {
        if let Some(next) = chars.next() {
          arg.push(next);
        }
      },
      '%' if chars.peek() == Some(&'%') => {
        chars.next();
        arg.push('%');
        started = true;
      },
      ch if ch.is_whitespace() && !quoted => {
        if started {
          args.push(std::mem::take(&mut arg));
          started = false;
        }
      },
      ch => {
        arg.push(ch);
        started = true;
      },
    }
  }
  if started {
    args.push(arg);
  }
  args
}

/// Writes the icon and the entry where they are missing or differ. An entry the app did not write stays as it is.
fn write(data: &Path, appimage: &Path) -> std::io::Result<()> {
  let entry = data.join(ENTRY);
  let current = std::fs::read_to_string(&entry).ok();
  if current.as_deref().is_some_and(|text| !marked(text)) {
    return Err(std::io::Error::new(
      std::io::ErrorKind::AlreadyExists,
      format!("{} was not written by Working", entry.display()),
    ));
  }
  let icon = data.join(ICON_FILE);
  if std::fs::read(&icon).ok().as_deref() != Some(ICON) {
    if let Some(dir) = icon.parent() {
      std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&icon, ICON)?;
  }
  let text = entry_text(appimage);
  if current.as_deref() == Some(text.as_str()) {
    return Ok(());
  }
  if let Some(dir) = entry.parent() {
    std::fs::create_dir_all(dir)?;
  }
  std::fs::write(entry, text)
}

/// Removes the entry and the icon, only when the entry is one the app wrote.
fn remove(data: &Path) -> std::io::Result<()> {
  let entry = data.join(ENTRY);
  if !std::fs::read_to_string(&entry).is_ok_and(|text| marked(&text)) {
    return Ok(());
  }
  std::fs::remove_file(entry)?;
  match std::fs::remove_file(data.join(ICON_FILE)) {
    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
    _ => Ok(()),
  }
}

/// The entry `scripts/package-linux.sh` puts in the `AppImage`, with `Exec` naming the `AppImage` itself and the mark
/// of an entry this app wrote. The window class and the notifications' desktop entry are both `working`, which ties
/// them to this file.
fn entry_text(appimage: &Path) -> String {
  format!(
    "[Desktop Entry]\n\
     Type=Application\n\
     Name=Working\n\
     GenericName=Chat\n\
     Comment=Google Chat on your desktop\n\
     Exec={}\n\
     Icon=working\n\
     Terminal=false\n\
     Categories=Network;InstantMessaging;Chat;\n\
     StartupWMClass=working\n\
     StartupNotify=true\n\
     {MARK}\n",
    quote(&appimage.to_string_lossy()),
  )
}

/// Quotes a path for an `Exec` key: in double quotes, with the characters the Desktop Entry Specification reserves
/// escaped by a backslash and `%` doubled, then a backslash doubled again, as every string value escapes it.
fn quote(path: &str) -> String {
  let mut quoted = String::with_capacity(path.len() + 2);
  quoted.push('"');
  for ch in path.chars() {
    match ch {
      '"' | '`' | '$' => quoted.push_str("\\\\"),
      '\\' => quoted.push_str("\\\\\\"),
      '%' => quoted.push('%'),
      _ => {},
    }
    quoted.push(ch);
  }
  quoted.push('"');
  quoted
}

/// The question in the bottom-right corner of the main window, until it has an answer.
pub fn card(cx: &App) -> Option<AnyElement> {
  let integration = cx.try_global::<Integration>()?;
  if !asks(cx.global::<Settings>().desktop_entry, integration.facts) {
    return None;
  }
  let button = |id: &'static str, label: &'static str| {
    div()
      .id(id)
      .h(px(28.))
      .px_3()
      .flex()
      .items_center()
      .rounded(px(8.))
      .text_size(px(13.))
      .cursor_pointer()
      .child(label)
  };
  Some(
    v_flex()
      .id("desktop-entry-card")
      .w(px(300.))
      .p_3()
      .gap_3()
      .rounded(px(12.))
      .border_1()
      .border_color(rgb(BORDER))
      .bg(rgb(POPOVER))
      .shadow_lg()
      .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
      .child(
        v_flex()
          .gap_0p5()
          .child(
            div()
              .text_size(px(13.))
              .font_weight(FontWeight::SEMIBOLD)
              .text_color(rgb(TEXT))
              .child("Add Working to your apps menu?"),
          )
          .child(
            div()
              .text_size(px(12.))
              .text_color(rgb(MUTED))
              .child("Shows its icon and name in the dock and the app launcher."),
          ),
      )
      .child(
        h_flex()
          .gap_2()
          .justify_end()
          .child(
            button("desktop-entry-no", "No thanks")
              .text_color(rgb(MUTED))
              .hover(|style| style.bg(rgb(ROW_HOVER)))
              .on_click(|_, _, cx| set(false, cx)),
          )
          .child(
            button("desktop-entry-yes", "Add")
              .bg(rgb(ACCENT_BG))
              .text_color(rgb(ACCENT))
              .hover(|style| style.opacity(0.85))
              .on_click(|_, _, cx| set(true, cx)),
          ),
      )
      .into_any_element(),
  )
}

#[cfg(test)]
mod tests {
  use super::{
    Choice, Facts, appimaged_running, arguments, asks, entry_text, exec, foreign_entry, opted_out, quote, refreshes,
    remove, unescape, write,
  };
  use std::ffi::OsStr;
  use std::path::{Path, PathBuf};

  /// An empty directory of the test's own under the system temp dir.
  fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("working-desktop-entry-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
  }

  #[test]
  fn exec_quotes_spaces_and_reserved_characters() {
    assert_eq!(
      quote("/home/jpark/My Apps/Working.AppImage"),
      "\"/home/jpark/My Apps/Working.AppImage\""
    );
    assert_eq!(quote("/tmp/a$b%c\\d"), "\"/tmp/a\\\\$b%%c\\\\\\\\d\"");
  }

  #[test]
  fn quoted_paths_read_back_as_the_same_argument() {
    for path in [
      "/home/jpark/Applications/Working.AppImage",
      "/home/jpark/My Apps/Working.AppImage",
      "/tmp/a$b%c\\d\"e`f",
    ] {
      assert_eq!(arguments(&unescape(&quote(path))), [path]);
    }
  }

  #[test]
  fn entry_names_the_appimage_the_window_class_and_the_mark() {
    let text = entry_text(Path::new("/home/jpark/Applications/Working.AppImage"));
    assert!(text.contains("Exec=\"/home/jpark/Applications/Working.AppImage\"\n"));
    assert!(text.contains("StartupWMClass=working\n"));
    assert!(text.ends_with("X-Working-Self-Integrated=true\n"));
    assert_eq!(exec(&text).unwrap(), ["/home/jpark/Applications/Working.AppImage"]);
  }

  #[test]
  fn exec_reads_only_the_desktop_entry_group() {
    let text = "[Desktop Entry]\nName=Working\nExec=/opt/Working.AppImage %U\n\n[Desktop Action New]\nExec=/bin/true\n";
    assert_eq!(exec(text).unwrap(), ["/opt/Working.AppImage", "%U"]);
    assert_eq!(exec("[Desktop Action New]\nExec=/bin/true\n"), None);
  }

  #[test]
  fn each_opt_out_holds_alone() {
    let data = Path::new("/home/jpark/.local/share");
    let proc = scratch("proc-empty");
    let none = |_: &Path| false;
    assert!(!opted_out(None, data, &proc, none));
    assert!(!opted_out(Some(OsStr::new("")), data, &proc, none));
    assert!(opted_out(Some(OsStr::new("0")), data, &proc, none));
    for file in [
      "/home/jpark/.local/share/appimagekit/no_desktopintegration",
      "/usr/share/appimagekit/no_desktopintegration",
      "/etc/appimagekit/no_desktopintegration",
    ] {
      assert!(opted_out(None, data, &proc, |path| path == Path::new(file)));
    }
    let running = scratch("proc-appimaged");
    for (pid, comm) in [("1", "systemd\n"), ("42", "appimaged\n"), ("self", "appimaged\n")] {
      std::fs::create_dir_all(running.join(pid)).unwrap();
      std::fs::write(running.join(pid).join("comm"), comm).unwrap();
    }
    assert!(opted_out(None, data, &running, none));
  }

  #[test]
  fn appimaged_is_found_by_process_name() {
    let proc = scratch("proc-names");
    let comm = |pid: &str, name: &str| {
      std::fs::create_dir_all(proc.join(pid)).unwrap();
      std::fs::write(proc.join(pid).join("comm"), format!("{name}\n")).unwrap();
    };
    comm("1", "systemd");
    comm("7", "appimagetool");
    assert!(!appimaged_running(&proc));
    // A directory that is not a process does not count.
    comm("self", "appimaged");
    assert!(!appimaged_running(&proc));
    comm("99", "appimaged-x86_6");
    assert!(appimaged_running(&proc));
  }

  #[test]
  fn foreign_entries_are_told_from_the_apps_own() {
    let data = scratch("foreign");
    let applications = data.join("applications");
    std::fs::create_dir_all(&applications).unwrap();
    let appimage = Path::new("/home/jpark/Apps/Working.AppImage");
    assert!(!foreign_entry(&applications, appimage));

    // The app's own entry, and an entry for another AppImage.
    write(&data, appimage).unwrap();
    std::fs::write(
      applications.join("other.desktop"),
      "[Desktop Entry]\nExec=/home/jpark/Apps/Other.AppImage\n",
    )
    .unwrap();
    assert!(!foreign_entry(&applications, appimage));

    // An integration tool's entry for this AppImage.
    let tool = applications.join("appimagekit_0f3c-working.desktop");
    std::fs::write(&tool, "[Desktop Entry]\nExec=/home/jpark/Apps/Working.AppImage %U\n").unwrap();
    assert!(foreign_entry(&applications, appimage));
    std::fs::remove_file(tool).unwrap();

    // A working.desktop the app did not write, whatever it runs.
    std::fs::write(applications.join("working.desktop"), "[Desktop Entry]\nExec=working\n").unwrap();
    assert!(foreign_entry(&applications, appimage));
  }

  #[test]
  fn the_answer_decides_asking_and_refreshing() {
    let free = Facts::default();
    assert!(asks(Choice::NotAsked, free));
    assert!(!asks(Choice::Yes, free));
    assert!(!asks(Choice::No, free));
    assert!(refreshes(Choice::Yes, free));
    assert!(!refreshes(Choice::NotAsked, free));
    assert!(!refreshes(Choice::No, free));

    for facts in [Facts { opted_out: true, ..free }, Facts { foreign: true, ..free }] {
      for choice in [Choice::NotAsked, Choice::Yes, Choice::No] {
        assert!(!asks(choice, facts));
        assert!(!refreshes(choice, facts));
      }
    }
  }

  #[test]
  fn the_switch_shows_while_the_app_may_write_or_has_files_to_remove() {
    let free = Facts::default();
    assert!(free.switch());
    assert!(!Facts { opted_out: true, ..free }.switch());
    assert!(!Facts { foreign: true, ..free }.switch());
    assert!(Facts { opted_out: true, ours: true, ..free }.switch());
  }

  #[test]
  fn writing_follows_the_appimage_and_removing_takes_only_the_apps_files() {
    let data = scratch("write");
    write(&data, Path::new("/home/jpark/Apps/Working.AppImage")).unwrap();
    write(&data, Path::new("/home/jpark/Downloads/Working.AppImage")).unwrap();
    let entry = data.join("applications/working.desktop");
    let icon = data.join("icons/hicolor/512x512/apps/working.png");
    let text = std::fs::read_to_string(&entry).unwrap();
    assert_eq!(exec(&text).unwrap(), ["/home/jpark/Downloads/Working.AppImage"]);
    assert!(icon.exists());

    remove(&data).unwrap();
    assert!(!entry.exists());
    assert!(!icon.exists());

    // An entry the app did not write is neither replaced nor removed, and neither is the icon beside it.
    std::fs::write(&entry, "[Desktop Entry]\nExec=working\n").unwrap();
    std::fs::write(&icon, b"png").unwrap();
    assert!(write(&data, Path::new("/home/jpark/Apps/Working.AppImage")).is_err());
    remove(&data).unwrap();
    assert_eq!(std::fs::read_to_string(&entry).unwrap(), "[Desktop Entry]\nExec=working\n");
    assert!(icon.exists());
  }
}
