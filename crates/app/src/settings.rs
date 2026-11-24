//! The user's settings: kept in `settings.json` in `gchat_core::config_dir` and held as a GPUI global, so views that
//! read them are redrawn when they change.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use gchat_core::{Command, Core, GroupId, McpConfig, McpStatus, Shortcut};
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use crate::theme::{self, ACCENT, ACCENT_BG, BORDER, MUTED, POPOVER, ROW_HOVER, ROW_SELECTED, TEXT, THEMES, rgb};

/// Where the MCP server listens unless the Address and Port fields in Settings, Agents say otherwise.
const MCP_ADDRESS: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const MCP_PORT: u16 = 3021;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Appearance {
  /// Follows the system appearance.
  #[default]
  System,
  Light,
  Dark,
}

/// How messages are drawn: Google Chat's two densities, or the lines of an IRC client.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageStyle {
  /// The user's own messages sit on the right, without avatar or name.
  #[default]
  Comfortable,
  /// Every message on the left, under its sender.
  Compact,
  /// One line a message, as in WeeChat: time, the sender's nick in a right-aligned column, then the text, in the
  /// monospaced font, without avatars or bubbles.
  Irc,
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
  /// As WeeChat: a narrow list of conversation names without avatars or previews, the open conversation under a
  /// one-line title bar, and the nick list of a space on the other side.
  Irc,
}

impl Layout {
  pub const ALL: [Self; 4] = [Self::GoogleChat, Self::Messages, Self::Rail, Self::Irc];

  pub const fn label(self) -> &'static str {
    match self {
      Self::GoogleChat => "Google Chat",
      Self::Messages => "Messages",
      Self::Rail => "Rail",
      Self::Irc => "IRC",
    }
  }

  /// Width of the layout's resizable column (the Google Chat sidebar, or the Messages, Rail or IRC list) until the
  /// user drags it.
  pub const fn default_width(self) -> f32 {
    match self {
      Self::GoogleChat => 264.,
      Self::Messages => 340.,
      Self::Rail => 320.,
      Self::Irc => 200.,
    }
  }

  /// A conversation list stays beside the open conversation, so opening one never leaves the list and Escape
  /// does not go back to Home.
  pub const fn keeps_list(self) -> bool {
    matches!(self, Self::Messages | Self::Rail | Self::Irc)
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

/// How lately a space or a group chat must have been active to stay in the conversation list of the Messages, Rail and
/// IRC layouts. Older ones wait behind the button at the end of the list. One-to-one chats always stay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListWindow {
  Week,
  TwoWeeks,
  Month,
  ThreeMonths,
  /// Every conversation, whenever it was last active.
  #[default]
  All,
}

impl ListWindow {
  pub const ALL: [Self; 5] = [Self::Week, Self::TwoWeeks, Self::Month, Self::ThreeMonths, Self::All];

  pub const fn label(self) -> &'static str {
    match self {
      Self::Week => "1 week",
      Self::TwoWeeks => "15 days",
      Self::Month => "1 month",
      Self::ThreeMonths => "3 months",
      Self::All => "All",
    }
  }

  /// The days of activity the list keeps; `None` keeps everything.
  pub const fn days(self) -> Option<u32> {
    match self {
      Self::Week => Some(7),
      Self::TwoWeeks => Some(15),
      Self::Month => Some(30),
      Self::ThreeMonths => Some(90),
      Self::All => None,
    }
  }
}

/// Where a thread opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadView {
  /// A panel beside the conversation, which stays until closed.
  Side,
  /// A window over the main window, which a click outside it or Escape closes.
  #[default]
  Modal,
}

impl ThreadView {
  pub const ALL: [Self; 2] = [Self::Modal, Self::Side];

  pub const fn label(self) -> &'static str {
    match self {
      Self::Side => "Side panel",
      Self::Modal => "Pop-up",
    }
  }
}

/// When the user shows as online to other people.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityStatus {
  /// Online while the computer is in use, away after a while without input.
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
  /// The sound the system is set to use for alerts.
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
  /// The sounds this OS plays: macOS has its named system sounds; elsewhere a notification plays the system's own
  /// sound or none.
  #[cfg(target_os = "macos")]
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
  /// The sounds this OS plays: macOS has its named system sounds; elsewhere a notification plays the system's own
  /// sound or none.
  #[cfg(not(target_os = "macos"))]
  pub const ALL: [Self; 2] = [Self::Default, Self::None];

  /// The sound that plays, one of [`Self::ALL`]: off macOS a named sound (saved on a Mac) is the system default.
  pub const fn resolved(self) -> Self {
    if cfg!(target_os = "macos") || matches!(self, Self::None) {
      self
    } else {
      Self::Default
    }
  }

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
  #[cfg(target_os = "macos")]
  pub fn file(self) -> Option<String> {
    (!matches!(self, Self::Default | Self::None)).then(|| format!("{}.aiff", self.label()))
  }

  /// Plays the sound once, for the user picking one.
  #[cfg(target_os = "macos")]
  pub fn preview(self) {
    if let Some(file) = self.file() {
      let _ = std::process::Command::new("afplay")
        .arg(format!("/System/Library/Sounds/{file}"))
        .spawn();
    }
  }
}

pub const DEFAULT_REWRITE_INSTRUCTION: &str = "\
Always fix grammar, spelling, punctuation, clarity, and flow. Only fix; keep the meaning, tone, and length.

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

/// The model and effort Rewrite runs one tool with. `None` is the harness default: no flag is passed, so the tool
/// picks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RewriteChoice {
  /// What the tool's model flag takes: a Claude Code alias, a Codex slug, Pi's `provider/id`.
  pub model: Option<String>,
  /// What the tool's effort (Pi: thinking) setting takes.
  pub effort: Option<String>,
}

/// Somewhere a number key (`keys::hotkey_keys`) opens: a view, or one conversation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
  Home,
  Mentions,
  Starred,
  Calendar,
  Meet,
  Mail,
  Conversation(GroupId),
}

impl From<Shortcut> for Place {
  fn from(shortcut: Shortcut) -> Self {
    match shortcut {
      Shortcut::Mentions => Self::Mentions,
      Shortcut::Starred => Self::Starred,
    }
  }
}

/// Which of Calendar, Meet, and Mail are on (`Settings::features`). A feature that is off is gone everywhere: no
/// button, key, menu entry, view, notification, MCP tools, or core traffic of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
  pub calendar: bool,
  pub meet: bool,
  pub mail: bool,
}

impl Default for Features {
  fn default() -> Self {
    Self { calendar: true, meet: true, mail: true }
  }
}

impl Features {
  /// Whether `place` can be opened: a view of a feature that is off cannot, wherever it is asked from.
  pub const fn opens(self, place: &Place) -> bool {
    match place {
      Place::Calendar => self.calendar,
      Place::Meet => self.meet,
      Place::Mail => self.mail,
      Place::Home | Place::Mentions | Place::Starred | Place::Conversation(_) => true,
    }
  }
}

/// Which tool groups and write tools the MCP server runs, from their switches and the features
/// (`Settings::mcp_servers`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct McpServers {
  pub chat: bool,
  pub allow_send: bool,
  pub mail: bool,
  pub mail_write: bool,
  pub calendar: bool,
  pub calendar_write: bool,
  pub meet: bool,
}

impl McpServers {
  /// Whether any tool group runs, and so the server.
  pub const fn any(self) -> bool {
    self.chat || self.mail || self.calendar || self.meet
  }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
  pub appearance: Appearance,
  /// Theme ids in `theme::THEMES`, one for each appearance.
  pub dark_theme: String,
  pub light_theme: String,
  /// The interface font, by family name. Absent is the system font (`theme::SYSTEM_FONT`). A family that is no
  /// longer installed reads as absent.
  pub font: Option<String>,
  /// The font of code spans and code blocks, by family name. Absent is the system monospaced font
  /// (`theme::SYSTEM_MONO`).
  pub mono_font: Option<String>,
  pub message_style: MessageStyle,
  pub layout: Layout,
  pub sidebar: SidebarMode,
  pub sidebar_side: SidebarSide,
  /// The Mentions button of every layout. Off removes only the button: the view still opens from anywhere else.
  pub show_mentions: bool,
  /// The Starred button of every layout. Off removes only the button: the view still opens from anywhere else.
  pub show_starred: bool,
  /// Google Calendar, a feature (`Features`). Off removes it everywhere: its view and buttons, its key and Go menu
  /// entry, its event reminders, and its MCP tools.
  pub calendar: bool,
  /// Google Meet, a feature (`Features`). Off removes its view and buttons, its key and Go menu entry, the call window
  /// (meetings open in the browser), and its MCP tools.
  pub meet: bool,
  /// Gmail, a feature (`Features`). Off, Working does not read Gmail at all: no view, buttons, key, or Go menu entry,
  /// no new-mail notifications, no mail in the Dock number, and no MCP tools.
  pub mail: bool,
  /// A notification for each unread email that reaches the inbox, while `mail` is on.
  pub mail_notify: bool,
  /// Every email shown whole is drawn on the other background (`mail_page::Colors::other`: a light page under a dark
  /// theme, a dark one under a light theme), for emails whose own colors do not read on the theme's. The contrast
  /// button on an email card switches it.
  pub mail_other_background: bool,
  /// A notification when an event's own reminder time comes, while `calendar` is on (`Command::WatchEvents`).
  pub event_reminders: bool,
  /// What each number key opens, by its number (1 to 9). A right-click on a view's button or a conversation assigns
  /// one.
  pub hotkeys: BTreeMap<u8, Place>,
  /// Dragged widths of each layout's resizable column; a layout without an entry uses `Layout::default_width`.
  pub column_widths: BTreeMap<Layout, f32>,
  /// Order of Home and of the Messages and Rail list.
  pub list_sort: Sort,
  /// Which spaces and group chats the Messages, Rail and IRC list shows before its "older" button.
  pub list_window: ListWindow,
  pub thread_view: ThreadView,
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
  /// Serve the chat tools to AI agents on this computer, on the MCP server at `/mcp`.
  pub mcp_chat: bool,
  /// The address the MCP server listens on. A loopback one is this computer only.
  pub mcp_address: IpAddr,
  /// The port the MCP server listens on.
  pub mcp_port: u16,
  /// Agents may send chat messages, not only read.
  pub mcp_send: bool,
  /// Serve the mail tools on the MCP server. They are served only while `mail` is on.
  pub mcp_mail: bool,
  /// Agents may send new emails and replies, forward, save drafts, archive, mark read, star, and label, not only read.
  /// Only while `mcp_mail` is on.
  pub mcp_mail_write: bool,
  /// Serve the calendar tools on the MCP server while `calendar` is on.
  pub mcp_calendar: bool,
  /// Agents may create, change, delete, and answer events, not only read.
  pub mcp_calendar_write: bool,
  /// Serve the Meet tools on the MCP server while `meet` is on.
  pub mcp_meet: bool,
  /// The status button's "Set as away". Stays until Automatic is chosen, whatever the activity status says.
  #[serde(default)]
  pub away: bool,
  /// CLI that rewrites the composer draft. Off hides the button.
  pub rewrite: RewriteTool,
  /// The model and effort each tool runs with. A tool without an entry runs with the harness defaults.
  pub rewrite_choices: BTreeMap<RewriteTool, RewriteChoice>,
  /// Extra instructions appended to the rewrite system prompt.
  pub rewrite_instruction: String,
  /// Every message is rewritten before it sends. Right-clicking the composer's rewrite button flips it.
  #[serde(default)]
  pub rewrite_always: bool,
  /// Rewrite also reads the latest messages of the conversation the draft goes into, and writes in their language
  /// unless the instructions or the draft ask for another.
  pub rewrite_context: bool,
  /// The formatting bar above the text of every composer, which the toggle at the composer's right shows and hides. The
  /// formatting keys work whether it is shown or not.
  #[serde(default)]
  pub show_format_bar: bool,
  /// The Calendar view last used.
  #[serde(default)]
  pub calendar_mode: crate::calendar_layout::Mode,
  /// The Calendar view's side panel (the small month and the list of calendars) is closed.
  #[serde(default)]
  pub calendar_panel_closed: bool,
  /// Ids of the calendars unchecked in that list: their events are not shown.
  #[serde(default)]
  pub calendar_hidden: Vec<String>,
  /// The version whose changelog was last seen: start-up stores the running version, and after an update opens What's
  /// New once (`changelog::startup`). Absent on a fresh install and in settings files from before it.
  pub changelog_seen: Option<String>,
  /// Linux: whether the app may put its desktop entry and icon in the user's data directory (`desktop_entry.rs`).
  #[cfg(target_os = "linux")]
  pub desktop_entry: crate::desktop_entry::Choice,
}

impl Default for Settings {
  fn default() -> Self {
    Self {
      appearance: Appearance::default(),
      dark_theme: theme::DEFAULT_DARK.into(),
      light_theme: theme::DEFAULT_LIGHT.into(),
      font: None,
      mono_font: None,
      message_style: MessageStyle::default(),
      layout: Layout::default(),
      sidebar: SidebarMode::default(),
      sidebar_side: SidebarSide::default(),
      show_mentions: true,
      show_starred: true,
      calendar: true,
      meet: true,
      mail: true,
      mail_notify: true,
      mail_other_background: false,
      event_reminders: true,
      hotkeys: BTreeMap::from([(1, Place::Home), (2, Place::Calendar), (3, Place::Meet), (4, Place::Mail)]),
      column_widths: BTreeMap::new(),
      list_sort: Sort::default(),
      list_window: ListWindow::default(),
      thread_view: ThreadView::default(),
      section_sort: BTreeMap::new(),
      camera_off: false,
      meet_builtin: true,
      meet_auto_open: true,
      status: ActivityStatus::default(),
      hours_start: 9 * 60,
      hours_end: 18 * 60,
      sound: Sound::default(),
      mcp_chat: false,
      mcp_address: MCP_ADDRESS,
      mcp_port: MCP_PORT,
      mcp_send: false,
      mcp_mail: false,
      mcp_mail_write: false,
      mcp_calendar: false,
      mcp_calendar_write: false,
      mcp_meet: false,
      away: false,
      rewrite: RewriteTool::Off,
      rewrite_choices: BTreeMap::new(),
      rewrite_instruction: DEFAULT_REWRITE_INSTRUCTION.into(),
      rewrite_always: false,
      rewrite_context: true,
      show_format_bar: false,
      calendar_mode: crate::calendar_layout::Mode::default(),
      calendar_panel_closed: false,
      calendar_hidden: Vec::new(),
      changelog_seen: None,
      #[cfg(target_os = "linux")]
      desktop_entry: crate::desktop_entry::Choice::NotAsked,
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
  /// The model and effort of the tool Rewrite runs.
  pub fn rewrite_choice(&self) -> RewriteChoice {
    self.rewrite_choices.get(&self.rewrite).cloned().unwrap_or_default()
  }

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
    let held = cx.global::<Self>();
    if &settings == held {
      return false;
    }
    let restyled = !settings.same_look(held);
    cx.set_global(settings);
    if restyled {
      apply_appearance(cx);
    }
    true
  }

  /// Whether `other` puts the same theme and fonts on the windows.
  fn same_look(&self, other: &Self) -> bool {
    self.appearance == other.appearance
      && self.light_theme == other.light_theme
      && self.dark_theme == other.dark_theme
      && self.font == other.font
      && self.mono_font == other.mono_font
  }

  /// Saves the settings as they are now. The file is written off the UI thread, through a temporary file renamed over
  /// it, so quitting mid-write leaves the old one whole; of two saves close together, the later one's settings are
  /// what the file keeps.
  pub fn commit(cx: &App) {
    Self::save(cx).detach();
  }

  /// `commit`, as a task that ends once the file is written: what quitting waits for.
  pub fn save(cx: &App) -> Task<()> {
    /// The number of the latest save asked for.
    static ASKED: AtomicU64 = AtomicU64::new(0);
    /// The number of the save the file holds.
    static WRITTEN: Mutex<u64> = Mutex::new(0);
    let text = match serde_json::to_string_pretty(cx.global::<Self>()) {
      Ok(text) => text,
      Err(error) => {
        tracing::warn!("could not save settings: {error}");
        return Task::ready(());
      },
    };
    let save = ASKED.fetch_add(1, Ordering::Relaxed) + 1;
    cx.background_executor().spawn(async move {
      let mut written = WRITTEN.lock().unwrap_or_else(PoisonError::into_inner);
      if *written > save {
        return;
      }
      let path = path();
      let partial = path.with_extension("json.tmp");
      let saved = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&partial, text))
        .and_then(|()| std::fs::rename(&partial, &path));
      match saved {
        Ok(()) => *written = save,
        Err(error) => tracing::warn!("could not save settings: {error}"),
      }
    })
  }

  /// The number key that opens `place`.
  pub fn hotkey_of(&self, place: &Place) -> Option<u8> {
    self.hotkeys.iter().find_map(|(slot, held)| (held == place).then_some(*slot))
  }

  /// Gives `place` the number key `slot`, taking it from what had it, or with `None` leaves `place` without a key. A
  /// place has one key at most.
  pub fn assign_hotkey(&mut self, slot: Option<u8>, place: Place) {
    self.hotkeys.retain(|_, held| *held != place);
    if let Some(slot) = slot {
      self.hotkeys.insert(slot, place);
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

  /// Whether any MCP tool group switch is on.
  pub const fn any_mcp(&self) -> bool {
    self.mcp_chat || self.mcp_mail || self.mcp_calendar || self.mcp_meet
  }

  /// Which of Calendar, Meet, and Mail are on.
  pub const fn features(&self) -> Features {
    Features {
      calendar: self.calendar,
      meet: self.meet,
      mail: self.mail,
    }
  }

  /// Which tool groups and write tools the MCP server runs: a group whose feature is off does not, whatever its
  /// switch says, and a write flag holds only while its group runs.
  pub const fn mcp_servers(&self) -> McpServers {
    let mail = self.mail && self.mcp_mail;
    let calendar = self.calendar && self.mcp_calendar;
    McpServers {
      chat: self.mcp_chat,
      allow_send: self.mcp_chat && self.mcp_send,
      mail,
      mail_write: mail && self.mcp_mail_write,
      calendar,
      calendar_write: calendar && self.mcp_calendar_write,
      meet: self.meet && self.mcp_meet,
    }
  }

  /// How the MCP server should run: `None` while no tool group is served.
  fn mcp_config(&self) -> Option<McpConfig> {
    let served = self.mcp_servers();
    if !served.any() {
      return None;
    }
    match gchat_core::mcp::token() {
      Ok(token) => Some(McpConfig {
        address: self.mcp_address,
        port: self.mcp_port,
        token,
        chat: served.chat,
        allow_send: served.allow_send,
        mail: served.mail,
        mail_write: served.mail_write,
        calendar: served.calendar,
        calendar_write: served.calendar_write,
        meet: served.meet,
      }),
      Err(error) => {
        tracing::warn!("could not store the MCP token: {error}");
        None
      },
    }
  }

  /// Where an agent on this computer reaches the MCP server, for its link and Claude Code command: the address and
  /// port it listens on, with 127.0.0.1 for an unspecified address (0.0.0.0 or ::), which is not one to connect to.
  pub const fn mcp_reach(&self) -> SocketAddr {
    let address = if self.mcp_address.is_unspecified() {
      IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
      self.mcp_address
    };
    SocketAddr::new(address, self.mcp_port)
  }

  /// Whether windows show the light theme: the chosen appearance, or the system's.
  fn light(&self, cx: &App) -> bool {
    match self.appearance {
      Appearance::Light => true,
      Appearance::Dark => false,
      Appearance::System => matches!(cx.window_appearance(), WindowAppearance::Light | WindowAppearance::VibrantLight),
    }
  }

  /// The interface family windows draw with: the saved one when it is installed, else the system font. Without a
  /// saved one the installed families are not listed.
  pub fn shown_font(&self, cx: &App) -> Option<SharedString> {
    let saved = self.font.as_deref()?;
    installed_family(Some(saved), installed_fonts(cx))
  }

  /// The monospace family windows draw with: the saved one when it is installed, else the system monospaced font.
  /// Without a saved one the installed families are not listed.
  pub fn shown_mono_font(&self, cx: &App) -> Option<SharedString> {
    let saved = self.mono_font.as_deref()?;
    installed_family(Some(saved), installed_fonts(cx))
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
  /// the pick shows; System keeps following the system appearance.
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

/// The font families installed on the computer, sorted, listed once: asking the text system costs around a hundred
/// milliseconds and the answer does not change while the app runs. The bundled emoji font is not one to pick.
pub fn installed_fonts(cx: &App) -> &'static [String] {
  static NAMES: OnceLock<Vec<String>> = OnceLock::new();
  NAMES.get_or_init(|| {
    let mut names = cx.text_system().all_font_names();
    names.retain(|name| !crate::emoji_font::is_bundled(name));
    names
  })
}

/// `saved` when `installed` has it. GPUI panics on the first line it lays out in a family it cannot find, so a
/// font that was uninstalled since it was saved reads as System (`None`).
fn installed_family(saved: Option<&str>, installed: &[String]) -> Option<SharedString> {
  let saved = saved?;
  installed
    .binary_search_by(|name| name.as_str().cmp(saved))
    .is_ok()
    .then(|| SharedString::from(saved.to_string()))
}

/// How many times the MCP token was replaced while the app runs: a new count makes `keep_mcp` read the token again.
#[derive(Default)]
struct TokenRotations(u64);

impl Global for TokenRotations {}

/// Starts the MCP server when the settings ask for it, and restarts or stops it whenever they (or its token)
/// change. The token file is read only when one of those changed, not on every settings change.
pub fn keep_mcp(core: Core, cx: &mut App) {
  cx.set_global(McpState::default());
  let mut running = None;
  let mut asked = None;
  let mut follow = move |cx: &mut App| {
    let settings = cx.global::<Settings>();
    let rotations = cx.try_global::<TokenRotations>().map_or(0, |rotations| rotations.0);
    // The tool groups as they would run, and where: a feature or a switch going on or off, or a new address or
    // port, restarts the server.
    let ask = (settings.mcp_servers(), settings.mcp_address, settings.mcp_port, rotations);
    if asked == Some(ask) {
      return;
    }
    asked = Some(ask);
    let wanted = settings.mcp_config();
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
  cx.default_global::<TokenRotations>().0 += 1;
  // Tells whoever follows the settings to look again: the server restarts with the new token.
  let settings = cx.global::<Settings>().clone();
  cx.set_global(settings);
}

/// Puts the theme of the chosen appearance (or the system's) and the chosen fonts on every window.
pub fn apply_appearance(cx: &mut App) {
  let index = cx.global::<Settings>().shown_theme(cx);
  show_theme(index, cx);
}

/// Puts the theme at `index` in `THEMES` on every window, without saving it: the theme picker previews with it.
pub fn show_theme(index: usize, cx: &mut App) {
  theme::set(index);
  let settings = cx.global::<Settings>();
  let (sans, mono) = (settings.shown_font(cx), settings.shown_mono_font(cx));
  // Linux has no one interface or monospace family every distribution installs: System is the first one this
  // computer has.
  #[cfg(target_os = "linux")]
  let (sans, mono) = (
    sans.or_else(|| theme::installed_sans(installed_fonts(cx))),
    mono.or_else(|| theme::installed_mono(installed_fonts(cx))),
  );
  theme::set_fonts(sans, mono);
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
    // Inputs, menus and the composer draw with the kit's families, not the window root's.
    kit.font_family = theme::sans();
    kit.mono_font_family = theme::mono();
  });
  cx.refresh_windows();
}

#[cfg(test)]
mod tests {
  use super::{Appearance, Features, Place, Settings, Sound, installed_family};
  use crate::theme::find;

  #[test]
  fn unknown_rewrite_tool_reads_as_off() {
    let tool: super::RewriteTool = serde_json::from_str("\"grok\"").unwrap_or(super::RewriteTool::Pi);
    assert_eq!(tool, super::RewriteTool::Off);
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn notification_sound_files() {
    assert_eq!(Sound::Glass.file().as_deref(), Some("Glass.aiff"));
    assert_eq!(Sound::Default.file(), None);
    assert_eq!(Sound::None.file(), None);
  }

  #[test]
  fn every_offered_sound_plays_as_itself() {
    for sound in Sound::ALL {
      assert_eq!(sound.resolved(), sound);
    }
    assert_eq!(Sound::None.resolved(), Sound::None);
    let glass = if cfg!(target_os = "macos") {
      Sound::Glass
    } else {
      Sound::Default
    };
    assert_eq!(Sound::Glass.resolved(), glass);
  }

  #[test]
  fn meet_settings_default_on() {
    let settings = Settings::default();
    assert!(settings.meet_builtin);
    assert!(settings.meet_auto_open);
    assert!(settings.calendar);
    assert!(settings.show_mentions);
    assert!(settings.show_starred);
    assert!(settings.meet);
    assert!(settings.mail);
    assert!(settings.mail_notify);
    assert!(settings.event_reminders);
  }

  #[test]
  fn meet_settings_missing_keys_are_on() {
    let settings: Settings = serde_json::from_str("{}").expect("empty object uses Default");
    assert!(settings.meet_builtin);
    assert!(settings.meet_auto_open);
    assert!(settings.calendar);
    assert!(settings.show_mentions);
    assert!(settings.show_starred);
    assert!(settings.meet);
    assert!(settings.mail);
    assert!(settings.mail_notify);
    assert!(settings.event_reminders);
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
  fn mail_other_background_is_off_until_switched() {
    assert!(!Settings::default().mail_other_background);
    let settings: Settings = serde_json::from_str("{}").expect("empty object uses Default");
    assert!(!settings.mail_other_background);
  }

  #[test]
  fn mail_other_background_roundtrip() {
    let settings = Settings {
      mail_other_background: true,
      ..Settings::default()
    };
    let text = serde_json::to_string(&settings).expect("serialize");
    let loaded: Settings = serde_json::from_str(&text).expect("deserialize");
    assert!(loaded.mail_other_background);
  }

  #[test]
  fn view_buttons_and_features_off_roundtrip() {
    let settings = Settings {
      show_mentions: false,
      show_starred: false,
      calendar: false,
      meet: false,
      mail: false,
      ..Settings::default()
    };
    let text = serde_json::to_string(&settings).expect("serialize");
    let loaded: Settings = serde_json::from_str(&text).expect("deserialize");
    assert!(!loaded.calendar);
    assert!(!loaded.show_mentions);
    assert!(!loaded.show_starred);
    assert!(!loaded.meet);
    assert!(!loaded.mail);
    assert_eq!(
      loaded.features(),
      Features {
        calendar: false,
        meet: false,
        mail: false
      }
    );
  }

  #[test]
  fn a_feature_that_is_off_opens_nowhere() {
    let features = Features { calendar: false, meet: true, mail: false };
    assert!(features.opens(&Place::Home));
    assert!(features.opens(&Place::Mentions));
    assert!(!features.opens(&Place::Calendar));
    assert!(features.opens(&Place::Meet));
    assert!(!features.opens(&Place::Mail));
    assert!(features.opens(&Place::Conversation(gchat_core::GroupId::Dm("jpark".into()))));
    assert!(Features::default().opens(&Place::Mail));
  }

  #[test]
  fn the_format_bar_is_hidden_until_it_is_shown() {
    assert!(!Settings::default().show_format_bar);
    let settings: Settings = serde_json::from_str("{}").expect("empty object uses Default");
    assert!(!settings.show_format_bar);
  }

  #[test]
  fn the_format_bar_roundtrip() {
    let settings = Settings {
      show_format_bar: true,
      ..Settings::default()
    };
    let text = serde_json::to_string(&settings).expect("serialize");
    let loaded: Settings = serde_json::from_str(&text).expect("deserialize");
    assert!(loaded.show_format_bar);
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

  #[test]
  fn fonts_default_to_system_and_roundtrip() {
    let settings: Settings = serde_json::from_str("{}").expect("empty object uses Default");
    assert_eq!(settings.font, None);
    assert_eq!(settings.mono_font, None);
    let settings = Settings {
      font: Some("Helvetica Neue".into()),
      mono_font: Some("Courier New".into()),
      ..Settings::default()
    };
    let text = serde_json::to_string(&settings).expect("serialize");
    let loaded: Settings = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(loaded.font.as_deref(), Some("Helvetica Neue"));
    assert_eq!(loaded.mono_font.as_deref(), Some("Courier New"));
  }

  #[test]
  fn a_font_that_is_not_installed_reads_as_system() {
    let installed = ["Courier New".to_string(), "Helvetica Neue".to_string(), "Menlo".to_string()];
    assert_eq!(installed_family(Some("Menlo"), &installed).as_deref(), Some("Menlo"));
    assert_eq!(installed_family(Some("Comic Neue"), &installed), None);
    assert_eq!(installed_family(Some(""), &installed), None);
    assert_eq!(installed_family(None, &installed), None);
  }

  #[test]
  fn number_keys_default_to_home_calendar_meet_and_mail() {
    let settings: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(settings.hotkey_of(&Place::Home), Some(1));
    assert_eq!(settings.hotkey_of(&Place::Calendar), Some(2));
    assert_eq!(settings.hotkey_of(&Place::Meet), Some(3));
    assert_eq!(settings.hotkey_of(&Place::Mail), Some(4));
    assert_eq!(settings.hotkey_of(&Place::Mentions), None);
  }

  #[test]
  fn a_number_key_moves_with_its_place_and_displaces_the_old_one() {
    let chat = Place::Conversation(gchat_core::GroupId::Dm("jpark".into()));
    let mut settings = Settings::default();
    settings.assign_hotkey(Some(2), chat.clone());
    assert_eq!(settings.hotkey_of(&chat), Some(2));
    assert_eq!(settings.hotkey_of(&Place::Calendar), None);
    // A place keeps one key: taking another gives the first one up.
    settings.assign_hotkey(Some(5), chat.clone());
    assert_eq!(settings.hotkey_of(&chat), Some(5));
    assert!(!settings.hotkeys.contains_key(&2));
    let back: Settings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
    assert_eq!(back.hotkeys, settings.hotkeys);
    settings.assign_hotkey(None, chat.clone());
    assert_eq!(settings.hotkey_of(&chat), None);
  }
}

#[cfg(test)]
mod mcp_tests {
  use super::Settings;

  #[test]
  fn every_mcp_tool_group_starts_off() {
    let settings: Settings = serde_json::from_str("{}").expect("empty object uses Default");
    assert!(!settings.mcp_chat && !settings.mcp_send);
    assert!(!settings.mcp_mail && !settings.mcp_mail_write);
    assert!(!settings.mcp_calendar && !settings.mcp_calendar_write);
    assert!(!settings.mcp_meet);
    assert!(!settings.any_mcp());
    assert_eq!(settings.mcp_port, super::MCP_PORT);
  }

  #[test]
  fn each_mcp_tool_group_keeps_its_own_switch() {
    let settings = Settings {
      mcp_mail: true,
      mcp_calendar: true,
      mcp_calendar_write: true,
      ..Settings::default()
    };
    let text = serde_json::to_string(&settings).expect("serialize");
    let loaded: Settings = serde_json::from_str(&text).expect("deserialize");
    assert!(loaded.mcp_mail && loaded.mcp_calendar && loaded.mcp_calendar_write);
    assert!(!loaded.mcp_chat && !loaded.mcp_send && !loaded.mcp_meet);
    assert!(loaded.any_mcp());
  }

  #[test]
  fn mail_writes_need_the_mail_tools() {
    let settings: Settings = serde_json::from_str(r#"{"mcp_mail_write":true}"#).expect("deserialize");
    assert!(settings.mcp_mail_write);
    assert!(!settings.mcp_servers().mail_write);
    let settings = Settings {
      mcp_mail: true,
      mcp_mail_write: true,
      ..Settings::default()
    };
    let text = serde_json::to_string(&settings).expect("serialize");
    let loaded: Settings = serde_json::from_str(&text).expect("deserialize");
    assert!(loaded.mcp_mail_write && loaded.mcp_servers().mail_write);
    assert!(!loaded.mcp_send && !loaded.mcp_calendar_write);
  }

  #[test]
  fn a_feature_that_is_off_serves_no_mcp_tools() {
    let all = Settings {
      mcp_chat: true,
      mcp_send: true,
      mcp_mail: true,
      mcp_mail_write: true,
      mcp_calendar: true,
      mcp_calendar_write: true,
      mcp_meet: true,
      ..Settings::default()
    };
    let served = all.mcp_servers();
    assert!(served.mail && served.mail_write && served.calendar && served.calendar_write && served.meet);
    let off = Settings {
      mail: false,
      calendar: false,
      meet: false,
      ..all.clone()
    };
    let served = off.mcp_servers();
    assert!(!served.mail && !served.mail_write);
    assert!(!served.calendar && !served.calendar_write);
    assert!(!served.meet);
    assert!(served.chat && served.allow_send && served.any());
    let only_mail = Settings {
      mail: false,
      mcp_chat: false,
      mcp_calendar: false,
      mcp_meet: false,
      ..all
    };
    assert!(!only_mail.mcp_servers().any());
  }

  #[test]
  fn agents_reach_an_unspecified_address_at_loopback() {
    let at = |address: &str| {
      Settings {
        mcp_address: address.parse().expect("an address"),
        mcp_port: 4000,
        ..Settings::default()
      }
      .mcp_reach()
      .to_string()
    };
    assert_eq!(Settings::default().mcp_reach().to_string(), "127.0.0.1:3021");
    assert_eq!(at("0.0.0.0"), "127.0.0.1:4000");
    assert_eq!(at("::"), "127.0.0.1:4000");
    assert_eq!(at("192.168.1.20"), "192.168.1.20:4000");
    assert_eq!(at("::1"), "[::1]:4000");
  }
}
