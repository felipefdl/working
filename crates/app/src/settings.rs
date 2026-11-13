//! The user's settings: kept in `~/.config/working/settings.json` and held as a GPUI global, so views that
//! read them are redrawn when they change.

use std::collections::BTreeMap;

use gchat_core::{Command, Core, McpConfig, McpStatus};
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use crate::theme::{self, ACCENT, ACCENT_BG, BORDER, MUTED, POPOVER, ROW_HOVER, ROW_SELECTED, TEXT, THEMES, rgb};

/// Where the MCP server listens unless `mcp_port` in the settings file says otherwise.
const MCP_PORT: u16 = 3021;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Appearance {
  /// Follows macOS.
  #[default]
  System,
  Light,
  Dark,
}

/// How messages are laid out, as in Google Chat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Density {
  /// The user's own messages sit on the right, without avatar or name.
  #[default]
  Comfortable,
  /// Every message on the left, under its sender.
  Compact,
}

/// How the main window is arranged, and how moving between conversations works in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Layout {
  /// Sidebar with Google Chat's sections, then Home or the open conversation.
  #[default]
  GoogleChat,
  /// One list of every conversation with previews, filtered by chips, beside the open conversation.
  Messages,
  /// An icon rail of categories, the selected one's list with previews, then the open conversation.
  Rail,
}

impl Layout {
  pub const ALL: [Self; 3] = [Self::GoogleChat, Self::Messages, Self::Rail];

  pub const fn label(self) -> &'static str {
    match self {
      Self::GoogleChat => "Google Chat",
      Self::Messages => "Messages",
      Self::Rail => "Rail",
    }
  }

  /// Width of the layout's resizable column (the Google Chat sidebar, or the Messages or Rail list) until the user
  /// drags it.
  pub const fn default_width(self) -> f32 {
    match self {
      Self::GoogleChat => 264.,
      Self::Messages => 340.,
      Self::Rail => 320.,
    }
  }

  /// A conversation list stays beside the open conversation, so opening one never leaves the list and Escape
  /// does not go back to Home.
  pub const fn keeps_list(self) -> bool {
    matches!(self, Self::Messages | Self::Rail)
  }
}

/// The left column(s) of every layout: the sidebar button's three states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SidebarMode {
  /// Beside the panel.
  #[default]
  Open,
  /// Out of the way until the sidebar button opens it again.
  Closed,
  /// Out of the way; the pointer at the window's left edge floats it in over the panel, and it goes when the
  /// pointer leaves.
  AutoHide,
}

/// Which side of the window the left column(s) of a layout sit on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SidebarSide {
  #[default]
  Left,
  Right,
}

/// Narrowest and widest a dragged column may get.
pub const COLUMN_MIN_WIDTH: f32 = 200.;
pub const COLUMN_MAX_WIDTH: f32 = 560.;

/// How a conversation list is ordered. Pinned conversations stay on top either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sort {
  /// Newest activity first.
  #[default]
  Recent,
  /// By name, A to Z.
  Alphabetical,
}

impl Sort {
  pub const ALL: [Self; 2] = [Self::Recent, Self::Alphabetical];

  pub const fn label(self) -> &'static str {
    match self {
      Self::Recent => "Sort by most recent",
      Self::Alphabetical => "Sort alphabetically",
    }
  }
}

/// When the user shows as online to other people.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityStatus {
  /// Online while the Mac is in use, away after a while without input.
  #[default]
  Automatic,
  /// As `Automatic` inside the set hours; away outside them.
  AutomaticInHours,
  /// Online for the whole of the set hours; away outside them.
  OnlineInHours,
  AlwaysOnline,
  AlwaysOffline,
}

/// What a notification sounds like.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sound {
  /// The sound macOS is set to use for alerts.
  #[default]
  Default,
  None,
  Glass,
  Ping,
  Pop,
  Tink,
  Hero,
  Submarine,
  Purr,
  Funk,
}

impl Sound {
  pub const ALL: [Self; 10] = [
    Self::Default,
    Self::None,
    Self::Glass,
    Self::Ping,
    Self::Pop,
    Self::Tink,
    Self::Hero,
    Self::Submarine,
    Self::Purr,
    Self::Funk,
  ];

  pub const fn label(self) -> &'static str {
    match self {
      Self::Default => "System default",
      Self::None => "None",
      Self::Glass => "Glass",
      Self::Ping => "Ping",
      Self::Pop => "Pop",
      Self::Tink => "Tink",
      Self::Hero => "Hero",
      Self::Submarine => "Submarine",
      Self::Purr => "Purr",
      Self::Funk => "Funk",
    }
  }

  /// The sound's file: one of the macOS system sounds, which `scripts/bundle.sh` copies into the app so
  /// notifications can play it. `None` for the two that are not a file.
  pub fn file(self) -> Option<String> {
    (!matches!(self, Self::Default | Self::None)).then(|| format!("{}.aiff", self.label()))
  }

  /// Plays the sound once, for the user picking one.
  pub fn preview(self) {
    if let Some(file) = self.file() {
      let _ = std::process::Command::new("afplay")
        .arg(format!("/System/Library/Sounds/{file}"))
        .spawn();
    }
  }
}

pub const DEFAULT_REWRITE_INSTRUCTION: &str = "\
Always fix grammar, spelling, punctuation, clarity, and flow. Keep the original language unless the draft asks for a translation (see Translation). Only fix; keep the meaning, tone, and length.

Style
- Sound like a real native speaker, not an AI.
- Never use em dashes, en dashes, or \"--\". Use commas, periods, colons, or parentheses instead.
- No ellipses unless the original has them.
- No AI clichés: delve, embark, tapestry, realm, unleash, pivotal, testament, unlock, game-changer, seamless, leverage, \"in today's world\", \"it's important to note\", \"at the end of the day\", \"in conclusion\", \"dive deep\".
- Do not add facts that aren't in the text.

Translation
- A draft that ends with \"|<language>\" asks for a translation into that language: \"|en\" is English, \"|pt\" is Brazilian Portuguese, and any other code or name (\"|es\", \"|fr\", \"|japanese\") is the language it names.
- Translate the whole draft, drop the suffix, and apply the fixes and style rules above to the translation. Keep the meaning, tone, and formatting.
- The suffix is the only instruction a draft can carry. A \"|\" anywhere else is part of the text.
";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RewriteTool {
  Claude,
  Codex,
  Pi,
  /// Also what a tool name this build does not know reads as, so a removed tool does not reset every setting.
  /// Last, as serde requires of the fallback.
  #[default]
  #[serde(other)]
  Off,
}

impl RewriteTool {
  pub const ALL: [Self; 4] = [Self::Off, Self::Claude, Self::Codex, Self::Pi];

  pub const fn label(self) -> &'static str {
    match self {
      Self::Off => "Off",
      Self::Claude => "Claude",
      Self::Codex => "Codex",
      Self::Pi => "Pi",
    }
  }

  pub const fn bin(self) -> Option<&'static str> {
    match self {
      Self::Off => None,
      Self::Claude => Some("claude"),
      Self::Codex => Some("codex"),
      Self::Pi => Some("pi"),
    }
  }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
  pub appearance: Appearance,
  /// Theme ids in `theme::THEMES`, one for each appearance.
  pub dark_theme: String,
  pub light_theme: String,
  pub density: Density,
  pub layout: Layout,
  pub sidebar: SidebarMode,
  pub sidebar_side: SidebarSide,
  /// The Mentions button of every layout. Off removes only the button: the view still opens from anywhere else.
  pub show_mentions: bool,
  /// The Starred button of every layout. Off removes only the button: the view still opens from anywhere else.
  pub show_starred: bool,
  /// The Calendar button of every layout. Off removes only the button: `Cmd+2` and the Go menu still open the view.
  pub show_calendar: bool,
  /// The Meet button of every layout. Off removes only the button: `Cmd+3` and the Go menu still open the view.
  pub show_meet: bool,
  /// Dragged widths of each layout's resizable column; a layout without an entry uses `Layout::default_width`.
  pub column_widths: BTreeMap<Layout, f32>,
  /// Order of Home and of the Messages and Rail list.
  pub list_sort: Sort,
  /// Order of each Google Chat sidebar section, by `Section::key`. A section without an entry uses its own
  /// default: A to Z for people, newest first elsewhere, as the web app does.
  pub section_sort: BTreeMap<String, Sort>,
  /// Calls never get the camera, and have no camera button.
  pub camera_off: bool,
  /// Open Google Meet in Working's call window. Off opens the link in the browser.
  pub meet_builtin: bool,
  /// After the call button posts a meeting, join it. Off only posts the link.
  pub meet_auto_open: bool,
  pub status: ActivityStatus,
  /// The set hours of `status`, as minutes of the day.
  pub hours_start: u16,
  pub hours_end: u16,
  pub sound: Sound,
  /// Serve the MCP tools to AI agents on this Mac.
  pub mcp: bool,
  pub mcp_port: u16,
  /// Agents may send messages, not only read.
  pub mcp_send: bool,
  /// The status button's "Set as away". Stays until Automatic is chosen, whatever the activity status says.
  #[serde(default)]
  pub away: bool,
  /// CLI that rewrites the composer draft. Off hides the button.
  pub rewrite: RewriteTool,
  /// Extra instructions appended to the rewrite system prompt.
  pub rewrite_instruction: String,
  /// Every message is rewritten before it sends. Right-clicking the composer's rewrite button flips it.
  #[serde(default)]
  pub rewrite_always: bool,
  /// The Calendar view last used.
  #[serde(default)]
  pub calendar_mode: crate::calendar_layout::Mode,
  /// The Calendar view's side panel (the small month and the list of calendars) is closed.
  #[serde(default)]
  pub calendar_panel_closed: bool,
  /// Ids of the calendars unchecked in that list: their events are not shown.
  #[serde(default)]
  pub calendar_hidden: Vec<String>,
}

impl Default for Settings {
  fn default() -> Self {
    Self {
      appearance: Appearance::default(),
      dark_theme: theme::DEFAULT_DARK.into(),
      light_theme: theme::DEFAULT_LIGHT.into(),
      density: Density::default(),
      layout: Layout::default(),
      sidebar: SidebarMode::default(),
      sidebar_side: SidebarSide::default(),
      show_mentions: true,
      show_starred: true,
      show_calendar: true,
      show_meet: true,
      column_widths: BTreeMap::new(),
      list_sort: Sort::default(),
      section_sort: BTreeMap::new(),
      camera_off: false,
      meet_builtin: true,
      meet_auto_open: true,
      status: ActivityStatus::default(),
      hours_start: 9 * 60,
      hours_end: 18 * 60,
      sound: Sound::default(),
      mcp: false,
      mcp_port: MCP_PORT,
      mcp_send: false,
      away: false,
      rewrite: RewriteTool::Off,
      rewrite_instruction: DEFAULT_REWRITE_INSTRUCTION.into(),
      rewrite_always: false,
      calendar_mode: crate::calendar_layout::Mode::default(),
      calendar_panel_closed: false,
      calendar_hidden: Vec::new(),
    }
  }
}

/// What the core last said about the MCP server, for the Settings window.
#[derive(Default)]
pub struct McpState(pub McpStatus);

impl Global for McpState {}

impl Global for Settings {}

fn path() -> std::path::PathBuf {
  gchat_core::config_dir().join("settings.json")
}

impl Settings {
  pub fn load() -> Self {
    std::fs::read_to_string(path())
      .ok()
      .and_then(|text| serde_json::from_str(&text).ok())
      .unwrap_or_default()
  }

  /// Changes the settings, saves them and applies them.
  pub fn update(cx: &mut App, change: impl FnOnce(&mut Self)) {
    if Self::preview(cx, change) {
      Self::commit(cx);
    }
  }

  /// Changes and applies the settings without saving them, for a change still going on (a column being dragged).
  /// Returns whether anything changed. `commit` saves.
  pub fn preview(cx: &mut App, change: impl FnOnce(&mut Self)) -> bool {
    let mut settings = cx.global::<Self>().clone();
    change(&mut settings);
    if &settings == cx.global::<Self>() {
      return false;
    }
    cx.set_global(settings);
    apply_appearance(cx);
    true
  }

  /// Saves the settings as they are now.
  pub fn commit(cx: &App) {
    match serde_json::to_string_pretty(cx.global::<Self>()) {
      Ok(text) => {
        let path = path();
        let written = path
          .parent()
          .map_or(Ok(()), std::fs::create_dir_all)
          .and_then(|()| std::fs::write(&path, text));
        if let Err(error) = written {
          tracing::warn!("could not save settings: {error}");
        }
      },
      Err(error) => tracing::warn!("could not save settings: {error}"),
    }
  }

  /// Width of a layout's resizable column: the dragged one, or its default.
  pub fn column_width(&self, layout: Layout) -> f32 {
    self
      .column_widths
      .get(&layout)
      .copied()
      .unwrap_or_else(|| layout.default_width())
      .clamp(COLUMN_MIN_WIDTH, COLUMN_MAX_WIDTH)
  }

  /// How the MCP server should run: `None` while it is off.
  fn mcp_config(&self) -> Option<McpConfig> {
    if !self.mcp {
      return None;
    }
    match gchat_core::mcp::token() {
      Ok(token) => Some(McpConfig {
        port: self.mcp_port,
        token,
        allow_send: self.mcp_send,
      }),
      Err(error) => {
        tracing::warn!("could not store the MCP token: {error}");
        None
      },
    }
  }

  /// Whether windows show the light theme: the chosen appearance, or the system's.
  fn light(&self, cx: &App) -> bool {
    match self.appearance {
      Appearance::Light => true,
      Appearance::Dark => false,
      Appearance::System => matches!(cx.window_appearance(), WindowAppearance::Light | WindowAppearance::VibrantLight),
    }
  }

  /// Index in `THEMES` of the theme windows show now.
  pub fn shown_theme(&self, cx: &App) -> usize {
    if self.light(cx) {
      theme::find(&self.light_theme, false)
    } else {
      theme::find(&self.dark_theme, true)
    }
  }

  /// Keeps the theme at `index` for its appearance. A pinned appearance of the other kind switches over to it, so
  /// the pick shows; System keeps following macOS.
  pub fn pick_theme(&mut self, index: usize) {
    let Some(picked) = THEMES.get(index) else {
      return;
    };
    if picked.dark {
      self.dark_theme = picked.id.into();
    } else {
      self.light_theme = picked.id.into();
    }
    if self.appearance != Appearance::System {
      self.appearance = if picked.dark {
        Appearance::Dark
      } else {
        Appearance::Light
      };
    }
  }
}

/// Starts the MCP server when the settings ask for it, and restarts or stops it whenever they (or its token)
/// change.
pub fn keep_mcp(core: Core, cx: &mut App) {
  cx.set_global(McpState::default());
  let mut running = None;
  let mut follow = move |cx: &mut App| {
    let wanted = cx.global::<Settings>().mcp_config();
    if wanted != running {
      running = wanted.clone();
      core.send(Command::SetMcp(wanted));
    }
  };
  follow(cx);
  cx.observe_global::<Settings>(follow).detach();
}

/// Replaces the MCP token, which cuts off every agent that holds the old one.
pub fn replace_mcp_token(cx: &mut App) {
  if let Err(error) = gchat_core::mcp::rotate_token() {
    tracing::warn!("could not store the MCP token: {error}");
  }
  // Tells whoever follows the settings to look again: the server restarts with the new token.
  let settings = cx.global::<Settings>().clone();
  cx.set_global(settings);
}

/// Puts the theme of the chosen appearance (or the system's) on every window.
pub fn apply_appearance(cx: &mut App) {
  let index = cx.global::<Settings>().shown_theme(cx);
  show_theme(index, cx);
}

/// Puts the theme at `index` in `THEMES` on every window, without saving it: the theme picker previews with it.
pub fn show_theme(index: usize, cx: &mut App) {
  theme::set(index);
  Theme::change(
    if theme::is_light() {
      ThemeMode::Light
    } else {
      ThemeMode::Dark
    },
    None,
    cx,
  );
  // gpui-kit's inputs and tooltips paint with its own colors; give them the theme's. A separate update from the
  // mode change, which reloads the kit's colors.
  Theme::update(cx, |kit| {
    kit.foreground = rgb(TEXT).into();
    kit.muted_foreground = rgb(MUTED).into();
    kit.caret = rgb(ACCENT).into();
    kit.selection = rgb(ACCENT_BG).into();
    kit.popover = rgb(POPOVER).into();
    kit.popover_foreground = rgb(TEXT).into();
    kit.border = rgb(BORDER).into();
    kit.input = rgb(BORDER).into();
    // Menus and selects: the kit's rows highlight with its own colors unless given the theme's.
    kit.list_hover = rgb(ROW_HOVER).into();
    kit.list_active = rgb(ROW_SELECTED).into();
    kit.accent = rgb(ROW_SELECTED).into();
    kit.accent_foreground = rgb(TEXT).into();
  });
  cx.refresh_windows();
}

#[cfg(test)]
mod tests {
  use super::{Appearance, Settings, Sound};
  use crate::theme::find;

  #[test]
  fn unknown_rewrite_tool_reads_as_off() {
    let tool: super::RewriteTool = serde_json::from_str("\"grok\"").unwrap_or(super::RewriteTool::Pi);
    assert_eq!(tool, super::RewriteTool::Off);
  }

  #[test]
  fn notification_sound_files() {
    assert_eq!(Sound::Glass.file().as_deref(), Some("Glass.aiff"));
    assert_eq!(Sound::Default.file(), None);
    assert_eq!(Sound::None.file(), None);
  }

  #[test]
  fn meet_settings_default_on() {
    let settings = Settings::default();
    assert!(settings.meet_builtin);
    assert!(settings.meet_auto_open);
    assert!(settings.show_calendar);
    assert!(settings.show_mentions);
    assert!(settings.show_starred);
    assert!(settings.show_meet);
  }

  #[test]
  fn meet_settings_missing_keys_are_on() {
    let settings: Settings = serde_json::from_str("{}").expect("empty object uses Default");
    assert!(settings.meet_builtin);
    assert!(settings.meet_auto_open);
    assert!(settings.show_calendar);
    assert!(settings.show_mentions);
    assert!(settings.show_starred);
    assert!(settings.show_meet);
  }

  #[test]
  fn meet_settings_off_roundtrip() {
    let settings = Settings {
      meet_builtin: false,
      meet_auto_open: false,
      ..Settings::default()
    };
    let text = serde_json::to_string(&settings).expect("serialize");
    let loaded: Settings = serde_json::from_str(&text).expect("deserialize");
    assert!(!loaded.meet_builtin);
    assert!(!loaded.meet_auto_open);
  }

  #[test]
  fn view_buttons_off_roundtrip() {
    let settings = Settings {
      show_mentions: false,
      show_starred: false,
      show_calendar: false,
      show_meet: false,
      ..Settings::default()
    };
    let text = serde_json::to_string(&settings).expect("serialize");
    let loaded: Settings = serde_json::from_str(&text).expect("deserialize");
    assert!(!loaded.show_calendar);
    assert!(!loaded.show_mentions);
    assert!(!loaded.show_starred);
    assert!(!loaded.show_meet);
  }

  #[test]
  fn a_pick_keeps_the_theme_for_its_own_appearance() {
    let mut settings = Settings::default();
    settings.pick_theme(find("dracula", true));
    settings.pick_theme(find("solarized-light", false));
    assert_eq!(settings.dark_theme, "dracula");
    assert_eq!(settings.light_theme, "solarized-light");
    assert_eq!(settings.appearance, Appearance::System);
  }

  #[test]
  fn a_pick_of_the_other_kind_switches_a_pinned_appearance() {
    let mut settings = Settings {
      appearance: Appearance::Dark,
      ..Settings::default()
    };
    settings.pick_theme(find("github-light", false));
    assert_eq!(settings.appearance, Appearance::Light);
    assert_eq!(settings.light_theme, "github-light");
    assert_eq!(settings.dark_theme, crate::theme::DEFAULT_DARK);
  }
}
