//! The Settings modal over the main window: sections on the left, their options on the right.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use gchat_core::{Command, Core, DndSchedule, DndSchedules, McpStatus};
use gpui_kit::component::IndexPath;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::searchable_list::{SearchableListDelegate, SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::{FocusTrapElement as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::agent_dialog::{AgentDialog, AgentEvent};
use crate::keys;
use crate::listen_dialog::{ListenDialog, ListenEvent};
#[cfg(target_os = "macos")]
use crate::notify;
#[cfg(target_os = "macos")]
use crate::permissions::{self, Media, Status};
use crate::rewrite::{self, Model};
use crate::settings::{
  self, ActivityStatus, Appearance, DEFAULT_REWRITE_INSTRUCTION, Layout, ListWindow, McpState, MessageStyle,
  RewriteChoice, RewriteTool, Settings, SidebarMode, SidebarSide, Sound, ThreadView,
};
use crate::status::{self, DndState};
use crate::theme::{
  ACCENT, ACCENT_BG, BG, BORDER, BUBBLE, GREEN, MUTED, POPOVER, RED, ROW_HOVER, TEXT, THEMES, Tone, WHITE,
  control_fill, rgb,
};
use crate::ui::{Icon, icon, switch};

#[cfg(target_os = "macos")]
const CAMERA_PANE: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera";
#[cfg(target_os = "macos")]
const MICROPHONE_PANE: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";
#[cfg(target_os = "macos")]
const NOTIFICATIONS_PANE: &str = "x-apple.systempreferences:com.apple.Notifications-Settings.extension";
/// How long typing in the rewrite instructions pauses before they are saved.
const INSTRUCTION_SAVE_DELAY: Duration = Duration::from_millis(500);
/// The first item of Rewrite's Model and Effort selects: no flag is passed and the tool picks.
const HARNESS_DEFAULT: &str = "Harness default";
/// The least and the most the Settings window measures as the main window grows.
const SETTINGS_SIZE: (Size<f32>, Size<f32>) =
  (Size { width: 780.0, height: 500.0 }, Size { width: 1120.0, height: 860.0 });

/// What a row of a feature that is off says, in place of what it would do.
fn feature_off(feature: &str) -> String {
  format!("{feature} is off in Settings, Features.")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
  Features,
  Appearance,
  Layout,
  Messages,
  Status,
  Notifications,
  Calls,
  Rewrite,
  Agents,
  /// What macOS lets the app use. Other OSes keep no such list the app can read or open.
  #[cfg(target_os = "macos")]
  Permissions,
}

impl Section {
  #[cfg(target_os = "macos")]
  const ALL: [Self; 10] = [
    Self::Features,
    Self::Appearance,
    Self::Layout,
    Self::Messages,
    Self::Status,
    Self::Notifications,
    Self::Calls,
    Self::Rewrite,
    Self::Agents,
    Self::Permissions,
  ];
  #[cfg(not(target_os = "macos"))]
  const ALL: [Self; 9] = [
    Self::Features,
    Self::Appearance,
    Self::Layout,
    Self::Messages,
    Self::Status,
    Self::Notifications,
    Self::Calls,
    Self::Rewrite,
    Self::Agents,
  ];

  const fn title(self) -> &'static str {
    match self {
      Self::Features => "Features",
      Self::Appearance => "Appearance",
      Self::Layout => "Layout",
      Self::Messages => "Messages",
      Self::Status => "Activity status",
      Self::Notifications => "Notifications",
      Self::Calls => "Calls",
      Self::Rewrite => "Rewrite",
      Self::Agents => "Agents",
      #[cfg(target_os = "macos")]
      Self::Permissions => "Permissions",
    }
  }

  const fn icon(self) -> Icon {
    match self {
      Self::Features => Icon::LayoutGrid,
      Self::Appearance => Icon::Monitor,
      Self::Layout => Icon::PanelLeft,
      Self::Messages => Icon::Thread,
      Self::Status => Icon::Activity,
      Self::Notifications => Icon::Bell,
      Self::Calls => Icon::Video,
      Self::Rewrite => Icon::Sparkles,
      Self::Agents => Icon::Bot,
      #[cfg(target_os = "macos")]
      Self::Permissions => Icon::Shield,
    }
  }
}

/// One group of the MCP server's tools, each switched on by itself.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tools {
  Chat,
  Mail,
  Calendar,
  Meet,
}

impl Tools {
  /// Its place among the groups, for the ids of its switches.
  const fn ix(self) -> usize {
    match self {
      Self::Chat => 0,
      Self::Mail => 1,
      Self::Calendar => 2,
      Self::Meet => 3,
    }
  }
}

/// What an agent's settings need to reach the MCP server, each copied by a button of its own. The setup for a
/// particular agent is in `agent_dialog`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
  /// One address with the token in it.
  Link,
  /// The token the server takes.
  Token,
}

impl Access {
  /// The text, for an agent reaching the server at `reach` (`Settings::mcp_reach`).
  fn text(self, reach: SocketAddr) -> std::io::Result<String> {
    let token = gchat_core::mcp::token()?;
    Ok(match self {
      Self::Link => format!("http://{reach}/mcp/{token}"),
      Self::Token => token,
    })
  }
}

/// What macOS allows right now. `notifications` is `None` when the app runs outside its bundle, where macOS
/// offers no notifications at all.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Allowed {
  camera: Status,
  microphone: Status,
  notifications: Option<Status>,
}

pub enum SettingsEvent {
  Close,
}

pub struct SettingsView {
  core: Core,
  section: Section,
  /// Holds the dialog's focus trap: it takes the keyboard on opening, so Tab stays in the dialog.
  focus: FocusHandle,
  #[cfg(target_os = "macos")]
  allowed: Allowed,
  /// What was just copied: its button says so for a moment.
  copied: Option<Access>,
  /// The dialog that changes the MCP server's address and port, while it is open.
  listen: Option<(Entity<ListenDialog>, Subscription)>,
  /// The dialog that adds the MCP server to an agent, while it is open.
  agent: Option<(Entity<AgentDialog>, Subscription)>,
  /// The do-not-disturb schedule whose days and times are open for changing.
  editing: Option<usize>,
  rewrite_instruction: Entity<TextareaState>,
  /// The instructions' reset button was clicked once: a second click within a few seconds resets them.
  reset_armed: bool,
  _reset_disarm: Option<Task<()>>,
  /// Saves the instructions once typing pauses. While it waits, closing or quitting saves them instead.
  instruction_save: Option<Task<()>>,
  _pending_saves: [Subscription; 2],
  dark_theme: Entity<SelectState<Vec<ThemeChoice>>>,
  light_theme: Entity<SelectState<Vec<ThemeChoice>>>,
  font: Entity<SelectState<SearchableVec<FontChoice>>>,
  mono_font: Entity<SelectState<SearchableVec<FontChoice>>>,
  list_window: Entity<SelectState<Vec<ListWindow>>>,
  thread_view: Entity<SelectState<Vec<ThreadView>>>,
  /// The models listed for one tool, kept until another tool's are listed. A failure keeps its message.
  rewrite_models: Option<(RewriteTool, Result<Arc<Vec<Model>>, String>)>,
  /// The listing on its way, and for which tool. Replacing it cancels the one before.
  rewrite_load: Option<(RewriteTool, Task<()>)>,
  rewrite_model: Entity<SelectState<SearchableVec<ModelChoice>>>,
  rewrite_effort: Entity<SelectState<Vec<EffortChoice>>>,
  /// What the Model and Effort selects were last filled from: they are filled again only when it changes.
  rewrite_shown: Option<RewriteShown>,
  /// The current model offers efforts: the Effort row shows.
  rewrite_has_efforts: bool,
  _copied_reset: Option<Task<()>>,
  #[cfg(target_os = "macos")]
  _refresh: Task<()>,
  _globals: [Subscription; 3],
  _rewrite_sub: Subscription,
  _theme_subs: [Subscription; 2],
  _font_subs: [Subscription; 2],
  _list_window_sub: Subscription,
  _thread_view_sub: Subscription,
  _rewrite_choice_subs: [Subscription; 2],
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
  pub fn new(core: Core, window: &mut Window, cx: &mut Context<Self>) -> Self {
    // Google Chat is where the schedules live: show what it has now.
    core.send(Command::LoadDnd);
    // The user changes these in System Settings while this is open: keep the list current.
    #[cfg(target_os = "macos")]
    let refresh = cx.spawn(async move |this, cx| {
      loop {
        let notifications = match notify::authorization().recv_async().await {
          Ok(Some(true)) => Some(Status::Allowed),
          Ok(Some(false)) => Some(Status::Denied),
          Ok(None) => Some(Status::NotAsked),
          Err(_) => None,
        };
        let allowed = Allowed { notifications, ..Self::read_allowed() };
        let updated = this.update(cx, |this, cx| {
          if this.allowed != allowed {
            this.allowed = allowed;
            cx.notify();
          }
        });
        if updated.is_err() {
          break;
        }
        cx.background_executor().timer(Duration::from_secs(2)).await;
      }
    });
    let instruction = cx.global::<Settings>().rewrite_instruction.clone();
    // No row cap: the box grows with the text, so the settings page is the only thing that scrolls.
    let rewrite_instruction = cx.new(|cx| {
      TextareaState::new(window, cx)
        .auto_grow(6, usize::MAX)
        .placeholder("How to rewrite")
    });
    rewrite_instruction.update(cx, |state, cx| state.set_value(instruction, window, cx));
    // Each keystroke applies the instructions; the file is saved once typing pauses, and on closing or quitting.
    let rewrite_sub = cx.subscribe_in(&rewrite_instruction, window, |this, field, event: &InputEvent, _, cx| {
      if matches!(event, InputEvent::Change) {
        let text = field.read(cx).value().to_string();
        if Settings::preview(cx, |settings| settings.rewrite_instruction = text) {
          this.save_instruction(cx);
        }
      }
    });
    let pending_saves = [
      cx.on_release(|this: &mut Self, cx| {
        if this.instruction_save.take().is_some() {
          Settings::commit(cx);
        }
      }),
      cx.on_app_quit(|this, cx| {
        let save = this.instruction_save.take().map(|_| Settings::save(cx));
        async move {
          if let Some(save) = save {
            save.await;
          }
        }
      }),
    ];
    let dark_theme = Self::theme_select(true, window, cx);
    let light_theme = Self::theme_select(false, window, cx);
    let theme_subs = [&dark_theme, &light_theme].map(|select| {
      cx.subscribe(select, |_, _, event: &SelectEvent<Vec<ThemeChoice>>, cx| {
        let SelectEvent::Confirm(Some(index)) = event else {
          return;
        };
        let index = *index;
        Settings::update(cx, |settings| settings.pick_theme(index));
      })
    });
    let font = Self::font_select(false, window, cx);
    let mono_font = Self::font_select(true, window, cx);
    let font_subs = [(&font, false), (&mono_font, true)].map(|(select, mono)| {
      cx.subscribe(select, move |_, _, event: &SelectEvent<SearchableVec<FontChoice>>, cx| {
        let SelectEvent::Confirm(Some(family)) = event else {
          return;
        };
        let family = family.clone();
        Settings::update(cx, |settings| {
          if mono {
            settings.mono_font = family;
          } else {
            settings.font = family;
          }
        });
      })
    });
    let (list_window, list_window_sub) = Self::list_window_select(window, cx);
    let (thread_view, thread_view_sub) = Self::thread_view_select(window, cx);
    let (rewrite_model, rewrite_effort, rewrite_choice_subs) = Self::rewrite_selects(window, cx);
    let focus = cx.focus_handle();
    window.focus(&focus, cx);
    Self {
      core,
      section: Section::Features,
      focus,
      #[cfg(target_os = "macos")]
      allowed: Self::read_allowed(),
      copied: None,
      listen: None,
      agent: None,
      editing: None,
      rewrite_instruction,
      reset_armed: false,
      _reset_disarm: None,
      instruction_save: None,
      _pending_saves: pending_saves,
      dark_theme,
      light_theme,
      font,
      mono_font,
      list_window,
      thread_view,
      rewrite_models: None,
      rewrite_load: None,
      rewrite_model,
      rewrite_effort,
      rewrite_shown: None,
      rewrite_has_efforts: false,
      _copied_reset: None,
      #[cfg(target_os = "macos")]
      _refresh: refresh,
      _globals: [
        cx.observe_global::<McpState>(|_, cx| cx.notify()),
        cx.observe_global::<DndState>(|_, cx| cx.notify()),
        // ⌘⇧T picks themes too: keep both selects on the saved ones. A new rewrite tool lists its models.
        cx.observe_global_in::<Settings>(window, |this, window, cx| {
          this.sync_theme_selects(window, cx);
          this.sync_rewrite(window, cx);
        }),
      ],
      _rewrite_sub: rewrite_sub,
      _theme_subs: theme_subs,
      _font_subs: font_subs,
      _list_window_sub: list_window_sub,
      _thread_view_sub: thread_view_sub,
      _rewrite_choice_subs: rewrite_choice_subs,
    }
  }

  /// Opens on Notifications, for the status button.
  pub fn at_notifications(core: Core, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let mut view = Self::new(core, window, cx);
    view.section = Section::Notifications;
    view
  }

  fn copy(&mut self, access: Access, cx: &mut Context<Self>) {
    match access.text(cx.global::<Settings>().mcp_reach()) {
      Ok(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
      Err(error) => {
        tracing::warn!("could not read the MCP token: {error}");
        return;
      },
    }
    self.copied = Some(access);
    self._copied_reset = Some(cx.spawn(async move |this, cx| {
      cx.background_executor().timer(Duration::from_millis(1500)).await;
      this
        .update(cx, |this, cx| {
          this.copied = None;
          cx.notify();
        })
        .ok();
    }));
    cx.notify();
  }

  /// Opens the dialog that adds the MCP server to an agent. Closing it gives the keyboard back to the settings.
  fn open_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let reach = cx.global::<Settings>().mcp_reach();
    let dialog = cx.new(|cx| AgentDialog::new(reach, window, cx));
    let sub = cx.subscribe_in(&dialog, window, |this, _, _: &AgentEvent, window, cx| {
      this.agent = None;
      window.focus(&this.focus, cx);
      cx.notify();
    });
    self.agent = Some((dialog, sub));
    cx.notify();
  }

  /// Opens the dialog that changes where the MCP server listens. Closing it gives the keyboard back to the settings.
  fn open_listen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let dialog = cx.new(|cx| ListenDialog::new(window, cx));
    let sub = cx.subscribe_in(&dialog, window, |this, _, _: &ListenEvent, window, cx| {
      this.listen = None;
      window.focus(&this.focus, cx);
      cx.notify();
    });
    self.listen = Some((dialog, sub));
    cx.notify();
  }

  /// A small button at the end of a row.
  fn pill(id: impl Into<ElementId>, label: &'static str) -> Stateful<Div> {
    div()
      .id(id)
      .px_3()
      .py_1()
      .flex_shrink_0()
      .rounded_full()
      .bg(control_fill())
      .text_size(px(12.))
      .cursor_pointer()
      .hover(|button| button.opacity(0.85))
      .child(label)
  }

  /// Everything that can be read at once; notifications answer through a callback.
  #[cfg(target_os = "macos")]
  fn read_allowed() -> Allowed {
    Allowed {
      camera: permissions::media(Media::Camera),
      microphone: permissions::media(Media::Microphone),
      notifications: None,
    }
  }

  fn group(title: &'static str, card: Div) -> Div {
    v_flex()
      .gap_2()
      .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(title))
      .child(card)
  }

  fn card() -> Div {
    v_flex().px_4().rounded(px(12.)).bg(rgb(BUBBLE))
  }

  fn row(title: impl Into<SharedString>, detail: impl Into<SharedString>, first: bool) -> Div {
    Self::tinted_row(title, detail, MUTED, first)
  }

  /// A row whose description is in `tone`.
  fn tinted_row(title: impl Into<SharedString>, detail: impl Into<SharedString>, tone: Tone, first: bool) -> Div {
    h_flex()
      .py_3()
      .gap_4()
      .items_center()
      .when(!first, |row| row.border_t_1().border_color(rgb(BORDER)))
      .child(
        v_flex()
          .flex_1()
          .min_w_0()
          .gap_0p5()
          .child(div().text_size(px(14.)).child(title.into()))
          .child(div().text_size(px(12.)).text_color(rgb(tone)).child(detail.into())),
      )
  }

  /// A select at a fixed width. The select fills its parent, so without the wrapper it would take the row
  /// and squeeze the title into a one-letter column.
  fn select_field<D>(select: Select<D>) -> Div
  where
    D: SearchableListDelegate + 'static,
    <D::Item as SearchableListItem>::Value: PartialEq + Clone,
  {
    div().w(px(220.)).flex_shrink_0().child(select.w_full().menu_max_h(px(320.)))
  }

  fn toggle(id: impl Into<ElementId>, on: bool) -> Stateful<Div> {
    switch(on, 24.).id(id).cursor_pointer()
  }

  /// A switch that flips `flip` while `enabled`, and is dimmed and does nothing while not (its feature is off).
  fn gated_toggle(
    id: impl Into<ElementId>,
    on: bool,
    enabled: bool,
    flip: impl Fn(&mut Settings) + 'static,
  ) -> Stateful<Div> {
    let switch = Self::toggle(id, on);
    if enabled {
      switch.on_click(move |_, _, cx| Settings::update(cx, &flip))
    } else {
      switch.opacity(0.4).cursor_default()
    }
  }

  /// Calendar, Meet, and Mail, each on or off as a whole.
  fn features(settings: &Settings) -> Div {
    let feature =
      |id: &'static str, title: &'static str, detail: &'static str, on: bool, first: bool, flip: fn(&mut Settings)| {
        Self::row(title, detail, first).child(Self::toggle(id, on).on_click(move |_, _, cx| Settings::update(cx, flip)))
      };
    Self::group(
      "Features",
      Self::card()
        .child(feature(
          "feature-calendar",
          "Calendar",
          "Google Calendar: your calendars and events, and reminders when an event is about to start. Off removes it \
           everywhere: its view and buttons, its reminders, and its tools for agents.",
          settings.calendar,
          true,
          |settings| settings.calendar.toggle(),
        ))
        .child(feature(
          "feature-meet",
          "Meet",
          "Google Meet: start and join meetings, call people, and take calls in Working's own window. Off removes it \
           everywhere: its view and buttons, the call window and its notifications, and its tools for agents. Meeting \
           links then open in the browser.",
          settings.meet,
          false,
          |settings| settings.meet.toggle(),
        ))
        .child(feature(
          "feature-mail",
          "Mail",
          "Gmail: read and write email, with the unread count. Off removes it everywhere: its view and buttons, its \
           notifications and its count in the Dock, and its tools for agents. While it is off, Working does not read \
           Gmail.",
          settings.mail,
          false,
          |settings| settings.mail.toggle(),
        )),
    )
  }

  fn appearance_picker(current: Appearance) -> Div {
    Self::segmented(
      &[
        ("System", Appearance::System),
        ("Light", Appearance::Light),
        ("Dark", Appearance::Dark),
      ],
      current,
      |settings, value| settings.appearance = value,
    )
  }

  fn side_picker(current: SidebarSide) -> Div {
    Self::segmented(
      &[("Left", SidebarSide::Left), ("Right", SidebarSide::Right)],
      current,
      |settings, value| settings.sidebar_side = value,
    )
  }

  /// A row of short choices, one selected: picking one writes it with `set`.
  fn segmented<T: Copy + PartialEq + 'static>(
    choices: &[(&'static str, T)],
    current: T,
    set: fn(&mut Settings, T),
  ) -> Div {
    let mut picker = h_flex().flex_shrink_0().p(px(3.)).gap_1().rounded(px(9.)).bg(control_fill());
    for &(label, value) in choices {
      let selected = value == current;
      picker = picker.child(
        div()
          .id(label)
          .px_3()
          .py_1()
          .rounded(px(7.))
          .text_size(px(12.))
          .text_color(rgb(if selected { TEXT } else { MUTED }))
          .cursor_pointer()
          .border_1()
          .border_color(if selected {
            rgb(ACCENT)
          } else {
            transparent_black().into()
          })
          .when(selected, |option| option.bg(rgb(ACCENT_BG)))
          .on_click(move |_, _, cx| Settings::update(cx, |settings| set(settings, value)))
          .child(label),
      );
    }
    picker
  }

  fn rewrite_picker(current: RewriteTool) -> Div {
    let mut picker = h_flex()
      .flex_shrink_0()
      .flex_wrap()
      .p(px(3.))
      .gap_1()
      .rounded(px(9.))
      .bg(control_fill());
    for value in RewriteTool::ALL {
      let selected = value == current;
      picker = picker.child(
        div()
          .id(value.label())
          .px_3()
          .py_1()
          .rounded(px(7.))
          .text_size(px(12.))
          .text_color(rgb(if selected { TEXT } else { MUTED }))
          .cursor_pointer()
          .border_1()
          .border_color(if selected {
            rgb(ACCENT)
          } else {
            transparent_black().into()
          })
          .when(selected, |option| option.bg(rgb(ACCENT_BG)))
          .on_click(move |_, _, cx| Settings::update(cx, |settings| settings.rewrite = value))
          .child(value.label()),
      );
    }
    picker
  }

  /// A message style choice: a small drawing of the rows over its radio button, as in Google Chat.
  fn message_style_option(value: MessageStyle, current: MessageStyle) -> Stateful<Div> {
    let selected = value == current;
    let (id, label) = match value {
      MessageStyle::Comfortable => ("comfortable", "Comfortable"),
      MessageStyle::Compact => ("compact", "Compact"),
      MessageStyle::Irc => ("irc", "IRC"),
    };
    let line = |width: f32, strong: bool| {
      div()
        .h(px(7.))
        .w(px(width))
        .rounded_full()
        .bg(rgb(if strong { TEXT } else { MUTED }))
        .opacity(if strong { 0.85 } else { 0.4 })
    };
    let left = |width: f32, strong: bool| {
      h_flex()
        .gap_1p5()
        .items_center()
        .child(line(7., strong))
        .child(line(width, strong))
    };
    let right = |width: f32| h_flex().justify_end().child(line(width, false));
    let frame = v_flex()
      .w(px(140.))
      .h(px(104.))
      .p_3()
      .gap_1p5()
      .justify_center()
      .rounded(px(10.))
      .border_2()
      .border_color(rgb(if selected { ACCENT } else { BORDER }))
      .bg(rgb(BG));
    let drawing = match value {
      MessageStyle::Comfortable => frame
        .child(left(52., true))
        .child(right(64.))
        .child(right(40.))
        .child(left(44., true))
        .child(right(48.)),
      MessageStyle::Compact => frame
        .child(left(20., false))
        .child(left(52., true))
        .child(left(64., false))
        .child(left(44., true))
        .child(left(28., false)),
      // Time, the sender right-aligned against a rule that runs down the rows, then the text.
      MessageStyle::Irc => {
        let column = |widths: [f32; 5], strong: bool, end: bool| {
          v_flex()
            .flex_shrink_0()
            .gap_1p5()
            .when(end, gpui_kit::Styled::items_end)
            .children(widths.map(|width| line(width, strong)))
        };
        frame.child(
          h_flex()
            .gap_1p5()
            .child(column([12.; 5], false, false))
            .child(
              div()
                .w(px(22.))
                .flex_shrink_0()
                .child(column([18., 12., 22., 18., 14.], true, true)),
            )
            .child(div().w(px(1.5)).flex_shrink_0().self_stretch().bg(rgb(MUTED)).opacity(0.6))
            .child(column([44., 30., 50., 38., 46.], false, false)),
        )
      },
    };
    let radio = div()
      .size(px(16.))
      .flex()
      .items_center()
      .justify_center()
      .rounded_full()
      .border_2()
      .border_color(rgb(if selected { ACCENT } else { MUTED }))
      .when(selected, |radio| radio.child(div().size(px(8.)).rounded_full().bg(rgb(ACCENT))));
    v_flex()
      .id(id)
      .gap_2()
      .cursor_pointer()
      .on_click(move |_, _, cx| Settings::update(cx, |settings| settings.message_style = value))
      .child(drawing)
      .child(
        h_flex()
          .gap_2()
          .items_center()
          .child(radio)
          .child(div().text_size(px(13.)).child(label)),
      )
  }

  /// A layout choice: a schematic of the window arrangement over its radio button.
  fn layout_option(value: Layout, current: Layout) -> Stateful<Div> {
    let selected = value == current;
    let id = match value {
      Layout::GoogleChat => "layout-google-chat",
      Layout::Messages => "layout-messages",
      Layout::Rail => "layout-rail",
      Layout::Irc => "layout-irc",
    };
    let line = |width: f32, strong: bool| {
      div()
        .h(px(5.))
        .w(px(width))
        .rounded_full()
        .flex_shrink_0()
        .bg(rgb(if strong { TEXT } else { MUTED }))
        .opacity(if strong { 0.85 } else { 0.4 })
    };
    // One list row: an avatar dot and two text lines.
    let row = || {
      h_flex()
        .gap_1()
        .items_center()
        .child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(rgb(MUTED)).opacity(0.5))
        .child(v_flex().gap(px(2.)).child(line(22., true)).child(line(14., false)))
    };
    let sidebar = || {
      v_flex()
        .w(px(36.))
        .flex_shrink_0()
        .gap_1p5()
        .child(line(30., true))
        .child(line(22., false))
        .child(line(28., false))
        .child(line(20., false))
    };
    let list = || {
      v_flex()
        .w(px(40.))
        .flex_shrink_0()
        .gap_1()
        .child(row())
        .child(row())
        .child(row())
    };
    let rail = || {
      v_flex()
        .w(px(10.))
        .flex_shrink_0()
        .gap_1p5()
        .items_center()
        .children((0..4).map(|_| div().size(px(7.)).rounded(px(2.)).bg(rgb(MUTED)).opacity(0.5)))
    };
    // IRC: a column of single short lines, no avatars; the buffer list is wider than the nick list.
    let names = |width: f32, lengths: [f32; 5]| {
      v_flex()
        .w(px(width))
        .flex_shrink_0()
        .gap(px(4.))
        .children(lengths.into_iter().enumerate().map(|(ix, length)| line(length, ix == 1)))
    };
    let panel = || {
      v_flex()
        .flex_1()
        .h_full()
        .p_1p5()
        .gap_1p5()
        .rounded(px(4.))
        .bg(rgb(BUBBLE))
        .child(line(30., true))
        .child(line(46., false))
        .child(line(38., false))
    };
    let frame = h_flex()
      .w(px(150.))
      .h(px(92.))
      .p_2()
      .gap_1p5()
      .rounded(px(10.))
      .border_2()
      .border_color(rgb(if selected { ACCENT } else { BORDER }))
      .bg(rgb(BG));
    let drawing = match value {
      Layout::GoogleChat => frame.child(sidebar()).child(panel()),
      Layout::Messages => frame.child(list()).child(panel()),
      Layout::Rail => frame.child(rail()).child(list()).child(panel()),
      Layout::Irc => frame
        .child(names(26., [20., 26., 16., 22., 18.]))
        .child(panel())
        .child(names(18., [16., 12., 18., 14., 10.])),
    };
    let radio = div()
      .size(px(16.))
      .flex()
      .items_center()
      .justify_center()
      .rounded_full()
      .border_2()
      .border_color(rgb(if selected { ACCENT } else { MUTED }))
      .when(selected, |radio| radio.child(div().size(px(8.)).rounded_full().bg(rgb(ACCENT))));
    v_flex()
      .id(id)
      .gap_2()
      .cursor_pointer()
      .on_click(move |_, _, cx| Settings::update(cx, |settings| settings.layout = value))
      .child(drawing)
      .child(
        h_flex()
          .gap_2()
          .items_center()
          .child(radio)
          .child(div().text_size(px(13.)).child(value.label())),
      )
  }

  /// A line of the permissions list: its state, and a button that asks macOS or opens the right pane of
  /// System Settings.
  #[cfg(target_os = "macos")]
  fn permission(
    id: &'static str,
    title: &'static str,
    detail: &'static str,
    status: Option<Status>,
    first: bool,
    act: impl Fn(&mut App) + 'static,
  ) -> Div {
    let (color, state) = match status {
      Some(Status::Allowed) => (GREEN, "Allowed"),
      Some(Status::Denied) => (RED, "Not allowed"),
      Some(Status::NotAsked) => (MUTED, "Not asked yet"),
      None => (MUTED, "Unavailable"),
    };
    let button = status.map(|status| {
      div()
        .id(id)
        .px_3()
        .py_1()
        .flex_shrink_0()
        .rounded_full()
        .bg(control_fill())
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|button| button.opacity(0.85))
        .on_click(move |_, _, cx| act(cx))
        .child(if status == Status::NotAsked {
          "Ask"
        } else {
          "Open Settings"
        })
    });
    Self::row(title, detail, first)
      .child(
        h_flex()
          .w(px(110.))
          .flex_shrink_0()
          .gap_1p5()
          .items_center()
          .child(if status == Some(Status::Allowed) {
            icon(Icon::Check, 14., color).into_any_element()
          } else {
            div().size(px(8.)).rounded_full().bg(rgb(color)).into_any_element()
          })
          .child(div().text_size(px(12.)).text_color(rgb(color)).child(state)),
      )
      .child(div().w(px(108.)).flex_shrink_0().flex().justify_end().children(button))
  }

  /// The themes of one appearance, the saved one selected.
  fn theme_select(dark: bool, window: &mut Window, cx: &mut Context<Self>) -> Entity<SelectState<Vec<ThemeChoice>>> {
    let choices = ThemeChoice::of(dark);
    let selected = Self::saved_theme_row(&choices, dark, cx);
    cx.new(|cx| SelectState::new(choices, selected, window, cx))
  }

  /// Where the saved theme of one appearance sits in its select.
  fn saved_theme_row(choices: &[ThemeChoice], dark: bool, cx: &App) -> Option<IndexPath> {
    let settings = cx.global::<Settings>();
    let saved = crate::theme::find(
      if dark {
        &settings.dark_theme
      } else {
        &settings.light_theme
      },
      dark,
    );
    choices.iter().position(|choice| choice.index == saved).map(IndexPath::new)
  }

  fn sync_theme_selects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    for (select, dark) in [(self.dark_theme.clone(), true), (self.light_theme.clone(), false)] {
      let saved = Self::saved_theme_row(&ThemeChoice::of(dark), dark, cx);
      if select.read(cx).selected_index(cx) != saved {
        select.update(cx, |select, cx| select.set_selected_index(saved, window, cx));
      }
    }
  }

  /// The interface fonts, or the system monospaced font, with the saved family selected. A saved family that is no
  /// longer installed shows System, as the windows draw it.
  fn font_select(
    mono: bool,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Entity<SelectState<SearchableVec<FontChoice>>> {
    let settings = cx.global::<Settings>();
    let shown = if mono {
      settings.shown_mono_font(cx)
    } else {
      settings.shown_font(cx)
    };
    let choices = FontChoice::all(cx);
    let selected = choices
      .iter()
      .position(|choice| choice.family.as_deref() == shown.as_deref())
      .map(IndexPath::new);
    cx.new(|cx| SelectState::new(SearchableVec::new(choices), selected, window, cx).searchable(true))
  }

  fn fonts(&self) -> Div {
    let field = |select: &Entity<SelectState<SearchableVec<FontChoice>>>, id: &'static str| {
      Self::select_field(
        Select::new(select)
          .id(id)
          .search_placeholder("Search fonts")
          .menu_width(px(260.)),
      )
    };
    Self::group(
      "Fonts",
      Self::card()
        .child(Self::row("Font", "System is the system's interface font.", true).child(field(&self.font, "font")))
        .child(
          Self::row(
            "Monospace font",
            "Code in messages. System is the system's monospaced font.",
            false,
          )
          .child(field(&self.mono_font, "mono-font")),
        ),
    )
  }

  /// The windows of activity the conversation list keeps, the saved one selected, and what picking one writes.
  fn list_window_select(
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> (Entity<SelectState<Vec<ListWindow>>>, Subscription) {
    let saved = cx.global::<Settings>().list_window;
    let selected = ListWindow::ALL.iter().position(|choice| *choice == saved).map(IndexPath::new);
    let select = cx.new(|cx| SelectState::new(ListWindow::ALL.to_vec(), selected, window, cx));
    let sub = cx.subscribe(&select, |_, _, event: &SelectEvent<Vec<ListWindow>>, cx| {
      let SelectEvent::Confirm(Some(choice)) = event else {
        return;
      };
      let choice = *choice;
      Settings::update(cx, |settings| settings.list_window = choice);
    });
    (select, sub)
  }

  /// Where a thread opens, the saved one selected, and what picking one writes.
  fn thread_view_select(
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> (Entity<SelectState<Vec<ThreadView>>>, Subscription) {
    let saved = cx.global::<Settings>().thread_view;
    let selected = ThreadView::ALL.iter().position(|choice| *choice == saved).map(IndexPath::new);
    let select = cx.new(|cx| SelectState::new(ThreadView::ALL.to_vec(), selected, window, cx));
    let sub = cx.subscribe(&select, |_, _, event: &SelectEvent<Vec<ThreadView>>, cx| {
      let SelectEvent::Confirm(Some(choice)) = event else {
        return;
      };
      let choice = *choice;
      Settings::update(cx, |settings| settings.thread_view = choice);
    });
    (select, sub)
  }

  /// Where a thread opens.
  fn threads_row(&self) -> Div {
    Self::row(
      "Threads",
      "Side panel keeps the thread beside the conversation. Pop-up opens it over the conversation; a click outside \
       closes it.",
      true,
    )
    .child(Self::select_field(Select::new(&self.thread_view).id("thread-view")))
  }

  fn appearance(&self, settings: Settings) -> Div {
    v_flex()
      .gap_5()
      .child(Self::group(
        "Theme",
        Self::card()
          .child(
            Self::row("Color mode", "System follows the system appearance.", true)
              .child(Self::appearance_picker(settings.appearance)),
          )
          .child(
            Self::row(
              "Dark theme",
              format!("{} previews every theme before you pick.", keys::text("secondary-shift-t")),
              false,
            )
            .child(Self::select_field(Select::new(&self.dark_theme).id("dark-theme"))),
          )
          .child(
            Self::row("Light theme", "Shown when the color mode is light.", false)
              .child(Self::select_field(Select::new(&self.light_theme).id("light-theme"))),
          ),
      ))
      .child(self.fonts())
  }

  /// How messages are drawn, and where a thread opens.
  fn messages(&self, settings: Settings) -> Div {
    v_flex()
      .gap_5()
      .child(Self::group(
        "Message style",
        Self::card().py_4().child(
          h_flex()
            .gap_4()
            .child(Self::message_style_option(MessageStyle::Comfortable, settings.message_style))
            .child(Self::message_style_option(MessageStyle::Compact, settings.message_style))
            .child(Self::message_style_option(MessageStyle::Irc, settings.message_style)),
        ),
      ))
      .child(Self::group("Threads", Self::card().child(self.threads_row())))
  }

  /// How the window is arranged: the layout and its sidebar, which conversations the list shows, and the view buttons.
  fn layout(&self, settings: Settings) -> Div {
    v_flex()
      .gap_5()
      .child(Self::group(
        "Layout",
        Self::card()
          // Rows of two: a wrapping row is measured one line tall inside the scrolling pane, which cut the
          // later lines and the switches below them off.
          .child(
            v_flex().gap_4().py_4().children(Layout::ALL.chunks(2).map(|pair| {
              h_flex()
                .gap_6()
                .children(pair.iter().map(|value| Self::layout_option(*value, settings.layout)))
            })),
          )
          .child(
            Self::row(
              "Auto-hide sidebar",
              "Keep the sidebar closed until the pointer reaches the window's edge. Right-clicking the top bar's \
               sidebar button does the same.",
              false,
            )
            .child(Self::toggle("sidebar-autohide", settings.sidebar == SidebarMode::AutoHide).on_click(|_, _, cx| {
              Settings::update(cx, |settings| {
                settings.sidebar = if settings.sidebar == SidebarMode::AutoHide {
                  SidebarMode::Open
                } else {
                  SidebarMode::AutoHide
                };
              });
            })),
          )
          .child(
            Self::row(
              "Sidebar position",
              "Which side of the window the sidebar, or the list, sits on. Drag its inner edge to resize it.",
              false,
            )
            .child(Self::side_picker(settings.sidebar_side)),
          ),
      ))
      .child(Self::group(
        "Conversation list",
        Self::card().child(
          Self::row(
            "Spaces and group chats",
            "Show the ones active in this period. Older ones wait behind a button at the end of the list.",
            true,
          )
          .child(Self::select_field(Select::new(&self.list_window).id("list-window"))),
        ),
      ))
      .child(Self::group(
        "View buttons",
        Self::card()
          .child(
            Self::row(
              "Show Mentions",
              "The Mentions button in the sidebar, the Chats header, or the rail.",
              true,
            )
            .child(
              Self::toggle("show-mentions", settings.show_mentions)
                .on_click(|_, _, cx| Settings::update(cx, |settings| settings.show_mentions.toggle())),
            ),
          )
          .child(
            Self::row(
              "Show Starred",
              "The Starred button in the sidebar, the Chats header, or the rail.",
              false,
            )
            .child(
              Self::toggle("show-starred", settings.show_starred)
                .on_click(|_, _, cx| Settings::update(cx, |settings| settings.show_starred.toggle())),
            ),
          ),
      ))
  }

  /// Linux: the switch for the app's own desktop entry, while the app is an `AppImage` that may add one or has one to
  /// remove (`desktop_entry.rs`).
  #[cfg(target_os = "linux")]
  fn desktop(cx: &App) -> Option<Div> {
    if !cx.try_global::<crate::desktop_entry::Integration>()?.switch() {
      return None;
    }
    let on = cx.global::<Settings>().desktop_entry == crate::desktop_entry::Choice::Yes;
    Some(Self::group(
      "Desktop",
      Self::card().child(
        Self::row(
          "Show in apps menu",
          "Working's icon and name in the dock and the app launcher. Off removes them.",
          true,
        )
        .child(Self::toggle("desktop-entry", on).on_click(move |_, _, cx| crate::desktop_entry::set(!on, cx))),
      ),
    ))
  }

  /// Only an `AppImage` on Linux installs a desktop entry of its own.
  #[cfg(not(target_os = "linux"))]
  const fn desktop(_: &App) -> Option<Div> {
    None
  }

  fn calls(settings: Settings) -> Div {
    // Linux has no call window (meetings open in the browser there), so these switches do nothing on it.
    let call_window = cfg!(not(target_os = "linux"));
    v_flex()
      .gap_5()
      .child(Self::group(
        "Meet",
        Self::card()
          .when(call_window, |card| {
            card.child(
              Self::row(
                "Built-in Google Meet",
                if settings.meet {
                  "Open meetings in Working. Off opens them in the browser.".to_string()
                } else {
                  format!("Meetings open in the browser. {}", feature_off("Meet"))
                },
                true,
              )
              .child(Self::gated_toggle(
                "meet-builtin",
                settings.meet_builtin,
                settings.meet,
                |settings| {
                  settings.meet_builtin.toggle();
                },
              )),
            )
          })
          .child(
            Self::row(
              "Open meetings automatically",
              "After you start a meeting, join it. Off only posts the link.",
              !call_window,
            )
            .child(
              Self::toggle("meet-auto-open", settings.meet_auto_open)
                .on_click(|_, _, cx| Settings::update(cx, |settings| settings.meet_auto_open.toggle())),
            ),
          ),
      ))
      .when(call_window, |calls| {
        calls.child(Self::group(
          "Camera",
          Self::card().child(
            Self::row(
              "Camera always off",
              if settings.meet {
                "Calls never get the camera, and have no camera button.".to_string()
              } else {
                format!("Calls never get the camera. {}", feature_off("Meet"))
              },
              true,
            )
            .child(Self::gated_toggle(
              "camera-off",
              settings.camera_off,
              settings.meet,
              |settings| {
                settings.camera_off.toggle();
              },
            )),
          ),
        ))
      })
  }

  fn radio(selected: bool) -> Div {
    div()
      .size(px(16.))
      .flex()
      .flex_shrink_0()
      .items_center()
      .justify_center()
      .rounded_full()
      .border_2()
      .border_color(rgb(if selected { ACCENT } else { MUTED }))
      .when(selected, |radio| radio.child(div().size(px(8.)).rounded_full().bg(rgb(ACCENT))))
  }

  /// A time of day with a button on each side that moves it half an hour.
  fn stepper(id: &str, minute: u16, change: impl Fn(u16, &mut App) + Clone + 'static) -> Div {
    let step = |id: String, label: &'static str, to: u16| {
      let change = change.clone();
      div()
        .id(SharedString::from(id))
        .size(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(control_fill())
        .text_size(px(14.))
        .cursor_pointer()
        .hover(|button| button.opacity(0.85))
        .on_click(move |_, _, cx| change(to, cx))
        .child(label)
    };
    h_flex()
      .flex_shrink_0()
      .gap_2()
      .items_center()
      .child(step(format!("{id}-earlier"), "−", (minute + 1440 - 30) % 1440))
      .child(div().w(px(76.)).text_size(px(13.)).text_center().child(status::clock(minute)))
      .child(step(format!("{id}-later"), "+", (minute + 30) % 1440))
  }

  fn status(settings: Settings) -> Div {
    let options = [
      (
        ActivityStatus::Automatic,
        "status-automatic",
        "Automatic",
        "Online while you use this computer, away after 10 minutes without touching it.",
      ),
      (
        ActivityStatus::AutomaticInHours,
        "status-automatic-hours",
        "Automatic during set hours",
        "As Automatic inside the hours below. Away outside them, no matter what.",
      ),
      (
        ActivityStatus::OnlineInHours,
        "status-online-hours",
        "Always online during set hours",
        "Online for the whole of the hours below. Away outside them, no matter what.",
      ),
      (
        ActivityStatus::AlwaysOnline,
        "status-online",
        "Always online",
        "Online for as long as Working is running.",
      ),
      (
        ActivityStatus::AlwaysOffline,
        "status-offline",
        "Always offline",
        "Shown as away, even while you chat.",
      ),
    ];
    let mut choices = Self::card();
    for (ix, (value, id, title, detail)) in options.into_iter().enumerate() {
      choices = choices.child(
        Self::row(title, detail, ix == 0)
          .id(id)
          .cursor_pointer()
          .on_click(move |_, _, cx| Settings::update(cx, |settings| settings.status = value))
          .child(Self::radio(value == settings.status)),
      );
    }
    let out = v_flex().gap_5().child(Self::group("Show me as", choices));
    if !matches!(
      settings.status,
      ActivityStatus::AutomaticInHours | ActivityStatus::OnlineInHours
    ) {
      return out;
    }
    let hours = Self::card()
      .child(
        Self::row("From", "Every day, in this computer's time.", true).child(Self::stepper(
          "hours-start",
          settings.hours_start,
          |minute, cx| Settings::update(cx, |settings| settings.hours_start = minute),
        )),
      )
      .child(Self::row("To", "After this you show as away.", false).child(Self::stepper(
        "hours-end",
        settings.hours_end,
        |minute, cx| Settings::update(cx, |settings| settings.hours_end = minute),
      )));
    out.child(Self::group("Set hours", hours))
  }

  /// Changes the do-not-disturb schedules here and in Google Chat.
  fn change_dnd(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut DndSchedules)) {
    let Some(mut schedules) = cx.global::<DndState>().0.clone() else {
      return;
    };
    change(&mut schedules);
    cx.set_global(DndState(Some(schedules.clone())));
    self.core.send(Command::SaveDnd(schedules));
  }

  fn notifications(&self, settings: Settings, cx: &mut Context<Self>) -> Div {
    let mut sounds = h_flex().flex_wrap().gap_2().py_3();
    for sound in Sound::ALL {
      let selected = sound == settings.sound.resolved();
      sounds = sounds.child(
        div()
          .id(sound.label())
          .px_3()
          .py_1()
          .rounded_full()
          .border_1()
          .border_color(if selected {
            rgb(ACCENT)
          } else {
            transparent_black().into()
          })
          .bg(if selected {
            rgb(ACCENT_BG)
          } else {
            control_fill()
          })
          .text_size(px(12.))
          .cursor_pointer()
          .hover(|button| button.opacity(0.85))
          .on_click(move |_, _, cx| {
            Settings::update(cx, |settings| settings.sound = sound);
            #[cfg(target_os = "macos")]
            sound.preview();
          })
          .child(sound.label()),
      );
    }
    let mail = Self::row(
      "New mail",
      if settings.mail {
        "Notify when an unread email reaches the inbox. Do not disturb quiets it too.".to_string()
      } else {
        format!("Notify when an unread email reaches the inbox. {}", feature_off("Mail"))
      },
      true,
    )
    .child(Self::gated_toggle(
      "mail-notify",
      settings.mail_notify,
      settings.mail,
      |settings| {
        settings.mail_notify.toggle();
      },
    ));
    let out = v_flex()
      .gap_5()
      .child(Self::group("Notification sound", Self::card().child(sounds)))
      .child(Self::group("Mail", Self::card().child(mail)))
      .child(Self::group(
        "Calendar",
        Self::card().child(
          Self::row(
            "Event reminders",
            if settings.calendar {
              "Notify when an event's reminder time comes, as Google Calendar sets it.".to_string()
            } else {
              format!(
                "Notify when an event's reminder time comes, as Google Calendar sets it. {}",
                feature_off("Calendar")
              )
            },
            true,
          )
          .child(Self::gated_toggle(
            "event-reminders",
            settings.event_reminders,
            settings.calendar,
            |settings| {
              settings.event_reminders.toggle();
            },
          )),
        ),
      ));

    let Some(dnd) = cx.global::<DndState>().0.clone() else {
      let waiting = Self::row(
        "Reading your schedules from Google Chat",
        "They show once you are connected.",
        true,
      );
      return out.child(Self::group("Do not disturb schedules", Self::card().child(waiting)));
    };
    let mut list = Self::card();
    for (ix, schedule) in dnd.schedules.iter().enumerate() {
      let editing = self.editing == Some(ix);
      let times = format!("{} - {}", status::clock(schedule.start), status::clock(schedule.end));
      list = list.child(
        Self::row(times, status::days_text(&schedule.days), ix == 0)
          .child(
            Self::pill(("dnd-edit", ix), if editing { "Done" } else { "Edit" }).on_click(cx.listener(
              move |this, _, _, cx| {
                this.editing = (!editing).then_some(ix);
                cx.notify();
              },
            )),
          )
          .child(
            Self::pill(("dnd-delete", ix), "Delete").on_click(cx.listener(move |this, _, _, cx| {
              this.editing = None;
              this.change_dnd(cx, |dnd| {
                if ix < dnd.schedules.len() {
                  dnd.schedules.remove(ix);
                }
              });
            })),
          )
          .child(
            Self::toggle(("dnd-on", ix), schedule.enabled).on_click(cx.listener(move |this, _, _, cx| {
              this.change_dnd(cx, |dnd| {
                if let Some(schedule) = dnd.schedules.get_mut(ix) {
                  schedule.enabled.toggle();
                }
              });
            })),
          ),
      );
      if editing {
        list = list.child(self.schedule_editor(ix, schedule, cx));
      }
    }
    list = list
      .child(
        Self::row(
          "New schedule",
          "Turns on do not disturb automatically at the times you set.",
          dnd.schedules.is_empty(),
        )
        .child(Self::pill("dnd-new", "Create new").on_click(cx.listener(|this, _, _, cx| {
          let mut added = None;
          this.change_dnd(cx, |dnd| {
            dnd.schedules.push(DndSchedule {
              days: vec![1, 2, 3, 4, 5],
              start: 19 * 60,
              end: 7 * 60,
              enabled: true,
            });
            added = Some(dnd.schedules.len() - 1);
          });
          this.editing = added;
        }))),
      )
      .child(
        Self::row(
          "Match do not disturb schedule to time zone",
          format!("Device's time zone: {}", gchat_core::device_time_zone()),
          false,
        )
        .child(
          Self::toggle("dnd-zone", dnd.match_time_zone).on_click(cx.listener(|this, _, _, cx| {
            this.change_dnd(cx, |dnd| dnd.match_time_zone.toggle());
          })),
        ),
      );
    out.child(Self::group("Do not disturb schedules", list))
  }

  /// The days and the two times of one schedule, under its row.
  fn schedule_editor(&self, ix: usize, schedule: &DndSchedule, cx: &mut Context<Self>) -> Div {
    let mut days = h_flex().gap_1p5();
    for (day, name) in status::WEEK {
      let on = schedule.days.contains(&day);
      days = days.child(
        div()
          .id(("dnd-day", ix * 10 + day as usize))
          .w(px(44.))
          .py_1()
          .flex()
          .justify_center()
          .rounded_full()
          .border_1()
          .border_color(if on {
            rgb(ACCENT)
          } else {
            transparent_black().into()
          })
          .bg(if on { rgb(ACCENT_BG) } else { control_fill() })
          .text_size(px(12.))
          .text_color(rgb(if on { TEXT } else { MUTED }))
          .cursor_pointer()
          .on_click(cx.listener(move |this, _, _, cx| {
            this.change_dnd(cx, |dnd| {
              let Some(schedule) = dnd.schedules.get_mut(ix) else {
                return;
              };
              match schedule.days.iter().position(|held| *held == day) {
                // A schedule keeps at least one day.
                Some(_) if schedule.days.len() == 1 => {},
                Some(at) => drop(schedule.days.remove(at)),
                None => schedule.days.push(day),
              }
            });
          }))
          .child(name),
      );
    }
    let time = |id: String, start: bool, minute: u16| {
      let view = cx.weak_entity();
      Self::stepper(&id, minute, move |minute, cx| {
        view
          .update(cx, |this, cx| {
            this.change_dnd(cx, |dnd| {
              let Some(schedule) = dnd.schedules.get_mut(ix) else {
                return;
              };
              *(if start {
                &mut schedule.start
              } else {
                &mut schedule.end
              }) = minute;
            });
          })
          .ok();
      })
    };
    let label = |text: &'static str| div().text_size(px(12.)).text_color(rgb(MUTED)).child(text);
    v_flex().pb_3().gap_3().child(days).child(
      h_flex()
        .gap_3()
        .items_center()
        .child(label("From"))
        .child(time(format!("dnd-start-{ix}"), true, schedule.start))
        .child(label("to"))
        .child(time(format!("dnd-end-{ix}"), false, schedule.end)),
    )
  }

  #[cfg(target_os = "macos")]
  fn permissions(&self) -> Div {
    let Allowed { camera, microphone, notifications } = self.allowed;
    let media = |media: Media, status: Status, pane: &'static str| {
      move |cx: &mut App| match status {
        Status::NotAsked => permissions::request_media(media),
        _ => cx.open_url(pane),
      }
    };
    Self::group(
      "macOS permissions",
      Self::card()
        .child(Self::permission(
          "camera",
          "Camera",
          "Your video in calls.",
          Some(camera),
          true,
          media(Media::Camera, camera, CAMERA_PANE),
        ))
        .child(Self::permission(
          "microphone",
          "Microphone",
          "Your voice in calls.",
          Some(microphone),
          false,
          media(Media::Microphone, microphone, MICROPHONE_PANE),
        ))
        .child(Self::permission(
          "notifications",
          "Notifications",
          "Alerts for new messages.",
          notifications,
          false,
          move |cx| match notifications {
            Some(Status::NotAsked) => notify::request(),
            _ => cx.open_url(NOTIFICATIONS_PANE),
          },
        )),
    )
  }

  /// The reset button's two clicks: the first arms it for a few seconds, the second puts the default back.
  fn reset_instruction(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if !self.reset_armed {
      self.reset_armed = true;
      self._reset_disarm = Some(cx.spawn(async move |this, cx| {
        cx.background_executor().timer(Duration::from_secs(4)).await;
        let _ = this.update(cx, |this, cx| {
          this.reset_armed = false;
          cx.notify();
        });
      }));
      cx.notify();
      return;
    }
    self.reset_armed = false;
    self._reset_disarm = None;
    self
      .rewrite_instruction
      .update(cx, |state, cx| state.set_value(DEFAULT_REWRITE_INSTRUCTION, window, cx));
    Settings::update(cx, |settings| settings.rewrite_instruction = DEFAULT_REWRITE_INSTRUCTION.into());
    cx.notify();
  }

  /// Saves the instructions once typing has paused for a moment, instead of on every keystroke.
  fn save_instruction(&mut self, cx: &mut Context<Self>) {
    self.instruction_save = Some(cx.spawn(async move |this, cx| {
      cx.background_executor().timer(INSTRUCTION_SAVE_DELAY).await;
      let _ = this.update(cx, |this, cx| {
        this.instruction_save = None;
        Settings::commit(cx);
      });
    }));
  }

  /// Rewrite's Model and Effort selects, holding only Harness default until `sync_rewrite` fills them, and what
  /// picking one writes. A new model drops a saved effort it does not offer.
  fn rewrite_selects(
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> (
    Entity<SelectState<SearchableVec<ModelChoice>>>,
    Entity<SelectState<Vec<EffortChoice>>>,
    [Subscription; 2],
  ) {
    let model = cx.new(|cx| {
      SelectState::new(
        SearchableVec::new(vec![ModelChoice::harness_default()]),
        Some(IndexPath::new(0)),
        window,
        cx,
      )
      .searchable(true)
    });
    let effort = cx.new(|cx| SelectState::new(vec![EffortChoice::new(None)], Some(IndexPath::new(0)), window, cx));
    let subs = [
      cx.subscribe(&model, |this, _, event: &SelectEvent<SearchableVec<ModelChoice>>, cx| {
        let SelectEvent::Confirm(Some(model)) = event else {
          return;
        };
        let model = model.clone();
        let tool = cx.global::<Settings>().rewrite;
        let efforts = rewrite::efforts(tool, this.listed_models(tool), model.as_deref());
        Settings::update(cx, |settings| {
          let choice = settings.rewrite_choices.entry(tool).or_default();
          if choice.effort.as_ref().is_some_and(|effort| !efforts.contains(effort)) {
            choice.effort = None;
          }
          choice.model = model;
        });
      }),
      cx.subscribe(&effort, |_, _, event: &SelectEvent<Vec<EffortChoice>>, cx| {
        let SelectEvent::Confirm(Some(effort)) = event else {
          return;
        };
        let effort = effort.clone();
        Settings::update(cx, |settings| {
          let tool = settings.rewrite;
          settings.rewrite_choices.entry(tool).or_default().effort = effort;
        });
      }),
    ];
    (model, effort, subs)
  }

  /// What listing `tool`'s models answered, if it has.
  fn rewrite_listing(&self, tool: RewriteTool) -> Option<&Result<Arc<Vec<Model>>, String>> {
    self
      .rewrite_models
      .as_ref()
      .filter(|(listed, _)| *listed == tool)
      .map(|(_, models)| models)
  }

  /// `tool`'s models, none while they are not listed or listing them failed.
  fn listed_models(&self, tool: RewriteTool) -> &[Model] {
    match self.rewrite_listing(tool) {
      Some(Ok(models)) => models,
      _ => &[],
    }
  }

  /// Lists the current tool's models off the UI thread, unless they are listed or on their way. `retry` lists them
  /// again after a failure, for coming back to the section.
  fn load_rewrite_models(&mut self, retry: bool, window: &mut Window, cx: &mut Context<Self>) {
    let tool = cx.global::<Settings>().rewrite;
    let listed = match self.rewrite_listing(tool) {
      Some(Ok(_)) => true,
      Some(Err(_)) => !retry,
      None => false,
    };
    let on_its_way = self.rewrite_load.as_ref().is_some_and(|(loading, _)| *loading == tool);
    if tool == RewriteTool::Off || listed || on_its_way {
      return;
    }
    let task = cx.spawn_in(window, async move |this, cx| {
      let listed = cx.background_executor().spawn(async move { rewrite::models(tool) }).await;
      let _ = this.update_in(cx, |this, window, cx| {
        this.rewrite_load = None;
        this.rewrite_models = Some((tool, listed.map(Arc::new)));
        this.sync_rewrite(window, cx);
        cx.notify();
      });
    });
    self.rewrite_load = Some((tool, task));
  }

  /// Keeps the Model and Effort selects on the current tool's models and saved choice, and lists the models while
  /// the section is on screen and the tool has none yet. The items are made again only when one of those changed.
  fn sync_rewrite(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.section == Section::Rewrite {
      self.load_rewrite_models(false, window, cx);
    }
    let settings = cx.global::<Settings>();
    let tool = settings.rewrite;
    let shown = RewriteShown {
      tool,
      models: self.rewrite_listing(tool).cloned(),
      choice: settings.rewrite_choice(),
    };
    if self.rewrite_shown.as_ref() == Some(&shown) {
      return;
    }
    let models = self.listed_models(tool);
    let RewriteChoice { model, effort } = &shown.choice;
    // A saved model the tool no longer lists still shows, by its id, instead of reading as Harness default.
    let mut model_items = vec![ModelChoice::harness_default()];
    if let Some(id) = model
      && !models.iter().any(|listed| listed.id == *id)
    {
      model_items.push(ModelChoice { id: Some(id.clone()), label: id.clone() });
    }
    model_items.extend(models.iter().map(|listed| ModelChoice {
      id: Some(listed.id.clone()),
      label: listed.label.clone(),
    }));
    let efforts = rewrite::efforts(tool, models, model.as_deref());
    self.rewrite_has_efforts = !efforts.is_empty();
    let mut effort_items = vec![EffortChoice::new(None)];
    if let Some(saved) = effort
      && self.rewrite_has_efforts
      && !efforts.contains(saved)
    {
      effort_items.push(EffortChoice::new(Some(saved.clone())));
    }
    effort_items.extend(efforts.into_iter().map(|effort| EffortChoice::new(Some(effort))));
    self.rewrite_model.update(cx, |select, cx| {
      select.set_items(SearchableVec::new(model_items), window, cx);
      select.set_selected_value(model, window, cx);
    });
    self.rewrite_effort.update(cx, |select, cx| {
      select.set_items(effort_items, window, cx);
      select.set_selected_value(effort, window, cx);
    });
    self.rewrite_shown = Some(shown);
  }

  /// The Model row: what listing the models is doing, or why it failed, under its title.
  fn rewrite_model_row(&self, tool: RewriteTool) -> Div {
    let loading = self.rewrite_load.as_ref().is_some_and(|(loading, _)| *loading == tool);
    let (detail, tone): (SharedString, Tone) = match self.rewrite_listing(tool) {
      _ if loading => ("Loading models...".into(), MUTED),
      None => ("Loading models...".into(), MUTED),
      Some(Err(error)) => (error.clone().into(), RED),
      Some(Ok(_)) => (
        "The model the tool runs. Harness default leaves the choice to the tool.".into(),
        MUTED,
      ),
    };
    Self::tinted_row("Model", detail, tone, false).child(Self::select_field(
      Select::new(&self.rewrite_model)
        .id("rewrite-model")
        .search_placeholder("Search models")
        .menu_width(px(320.)),
    ))
  }

  fn rewrite(&self, settings: Settings, cx: &mut Context<Self>) -> Div {
    let customized = settings.rewrite_instruction != DEFAULT_REWRITE_INSTRUCTION;
    let armed = self.reset_armed;
    Self::card()
      .child(
        Self::row(
          "Tool",
          "The CLI on this computer that rewrites a draft in Chat and in Mail. Off hides the button.",
          true,
        )
        .child(Self::rewrite_picker(settings.rewrite)),
      )
      .when(settings.rewrite != RewriteTool::Off, |card| {
        card.child(self.rewrite_model_row(settings.rewrite)).when(self.rewrite_has_efforts, |card| {
          card.child(
            Self::row("Effort", "How hard the model thinks. Harness default leaves it to the tool.", false)
              .child(Self::select_field(Select::new(&self.rewrite_effort).id("rewrite-effort"))),
          )
        })
      })
      .child(
        Self::row(
          "Always rewrite",
          "Rewrite every Chat message or email before it sends, except one that is only links. Start one with ! to \
           send it as written. Right-click the rewrite button to switch.",
          false,
        )
        .child(
          Self::toggle("rewrite-always", settings.rewrite_always)
            .on_click(|_, _, cx| Settings::update(cx, |settings| settings.rewrite_always.toggle())),
        ),
      )
      .child(
        Self::row(
          "Conversation context",
          "Rewrite reads the latest messages of the conversation and writes in its language, unless the instructions or the draft ask for another.",
          false,
        )
        .child(
          Self::toggle("rewrite-context", settings.rewrite_context)
            .on_click(|_, _, cx| Settings::update(cx, |settings| settings.rewrite_context.toggle())),
        ),
      )
      .child(
        v_flex()
          .py_3()
          .gap_1()
          .border_t_1()
          .border_color(rgb(BORDER))
          .child(
            h_flex()
              .gap_4()
              .items_start()
              .child(
                v_flex()
                  .flex_1()
                  .min_w_0()
                  .gap_1()
                  .child(div().text_size(px(14.)).child("Instructions"))
                  .child(
                    div()
                      .text_size(px(12.))
                      .text_color(rgb(MUTED))
                      .child("Added to the rewrite prompt."),
                  ),
              )
              .when(customized, |header| {
                header.child(
                  div()
                    .id("reset-instruction")
                    .h(px(28.))
                    .px_3()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .rounded(px(8.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .when(armed, |button| button.bg(rgb(RED)).text_color(rgb(WHITE)))
                    .when(!armed, |button| {
                      button
                        .bg(control_fill())
                        .text_color(rgb(MUTED))
                        .hover(|style| style.bg(rgb(ROW_HOVER)))
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.reset_instruction(window, cx)))
                    .child(if armed {
                      "Click again to reset"
                    } else {
                      "Reset to default"
                    }),
                )
              }),
          )
          .child(
            div()
              .mt_1()
              .px_2()
              .py_1()
              .rounded(px(9.))
              .bg(control_fill())
              .child(Textarea::new(&self.rewrite_instruction).appearance(false).bordered(false)),
          ),
      )
  }

  fn agents(&self, settings: Settings, cx: &mut Context<Self>) -> Div {
    let out = v_flex()
      .gap_5()
      .child(Self::tools_group(
        &settings,
        Tools::Chat,
        "Chat",
        "Agents catch up on unread conversations and your recent activity, and find, read, and search conversations."
          .into(),
        Some((
          "Let agents send messages",
          "Adds a tool that posts as you. Without it agents only read.",
        )),
      ))
      .child(Self::tools_group(
        &settings,
        Tools::Mail,
        "Mail",
        "Agents list, search, and read your mail and labels. They change nothing unless you let them below.".into(),
        Some((
          "Let agents send and change mail",
          "Adds tools that send new emails, replies, and forwards from your address, save drafts, archive, mark read, \
           star, and label. What they send is real email. Without it agents only read.",
        )),
      ))
      .child(Self::tools_group(
        &settings,
        Tools::Calendar,
        "Calendar",
        "Agents list your calendars and read their events.".into(),
        Some((
          "Let agents change events",
          "Adds tools that create, change, and delete events and answer invitations. Without it agents only read.",
        )),
      ))
      .child(Self::tools_group(
        &settings,
        Tools::Meet,
        "Meet",
        "Agents make new meetings and list your events with a Meet link in the next 7 days.".into(),
        None,
      ));
    if !settings.any_mcp() {
      return out;
    }

    let (color, state) = match &cx.global::<McpState>().0 {
      McpStatus::Running { address } => (
        GREEN,
        if address.ip().is_loopback() {
          format!("Running at http://{address}/mcp, for this computer only.")
        } else {
          format!("Running at http://{address}/mcp.")
        },
      ),
      McpStatus::Failed(error) => (
        RED,
        format!(
          "Could not listen on {}: {error}. Change the address or port below.",
          SocketAddr::new(settings.mcp_address, settings.mcp_port)
        ),
      ),
      McpStatus::Off => (MUTED, "Starts once you are signed in.".to_string()),
    };
    let listen = SocketAddr::new(settings.mcp_address, settings.mcp_port);
    let listen = if settings.mcp_address.is_loopback() {
      format!("{listen}. This computer only.")
    } else {
      listen.to_string()
    };
    let reach = settings.mcp_reach();
    let copy = |access: Access, id: &'static str, label: &'static str, cx: &mut Context<Self>| {
      Self::pill(
        id,
        if self.copied == Some(access) {
          "Copied"
        } else {
          label
        },
      )
      .on_click(cx.listener(move |this, _, _, cx| this.copy(access, cx)))
    };
    let status = Self::card()
      .child(Self::row("Status", state, true).child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(rgb(color))))
      .child(
        Self::row("Address", listen, false).child(Self::pill("change-listen", "Change").on_click(cx.listener(
          |this, _, window, cx| {
            this.open_listen(window, cx);
          },
        ))),
      )
      .child(
        Self::row(
          "Link",
          format!("http://{reach}/mcp with the token inside, for an agent that only takes a URL."),
          false,
        )
        .child(copy(Access::Link, "copy-link", "Copy link", cx)),
      )
      .child(
        Self::row(
          "Add to an agent",
          "The command or settings that add the server to an agent, for every project.",
          false,
        )
        .child(Self::pill("add-agent", "Choose").on_click(cx.listener(|this, _, window, cx| {
          this.open_agent(window, cx);
        }))),
      )
      .child(
        Self::row(
          "Token",
          "The server takes it, for an agent that takes the address and an Authorization: Bearer header.",
          false,
        )
        .child(copy(Access::Token, "copy-token", "Copy token", cx)),
      )
      .child(
        Self::row("New token", "Cuts off every agent that has the current one.", false)
          .child(Self::pill("new-token", "Replace").on_click(|_, _, cx| settings::replace_mcp_token(cx))),
      );
    out.child(Self::group("Server", status))
  }

  /// One group of the MCP server's tools: its switch with what agents get, and its write switch when it has one
  /// (off while the group is).
  fn tools_group(
    settings: &Settings,
    tools: Tools,
    title: &'static str,
    detail: String,
    write: Option<(&'static str, &'static str)>,
  ) -> Div {
    let on = match tools {
      Tools::Chat => settings.mcp_chat,
      Tools::Mail => settings.mcp_mail,
      Tools::Calendar => settings.mcp_calendar,
      Tools::Meet => settings.mcp_meet,
    };
    // A group whose feature is off is not served, whatever its switches say.
    let feature = match tools {
      Tools::Chat => true,
      Tools::Mail => settings.mail,
      Tools::Calendar => settings.calendar,
      Tools::Meet => settings.meet,
    };
    let detail = if feature {
      detail
    } else {
      format!("{detail} {}", feature_off(title))
    };
    let flip = move |settings: &mut Settings| match tools {
      Tools::Chat => settings.mcp_chat.toggle(),
      Tools::Mail => settings.mcp_mail.toggle(),
      Tools::Calendar => settings.mcp_calendar.toggle(),
      Tools::Meet => settings.mcp_meet.toggle(),
    };
    let mut card = Self::card().child(Self::row(format!("{title} tools"), detail, true).child(Self::gated_toggle(
      ("mcp-tools", tools.ix()),
      on,
      feature,
      flip,
    )));
    if let Some((label, what)) = write {
      let allowed = match tools {
        Tools::Calendar => settings.mcp_calendar_write,
        Tools::Mail => settings.mcp_mail_write,
        _ => settings.mcp_send,
      };
      // Off while its group is, or its feature: there is nothing to allow.
      let switch = Self::gated_toggle(("mcp-write", tools.ix()), allowed, on && feature, move |settings| match tools {
        Tools::Calendar => settings.mcp_calendar_write.toggle(),
        Tools::Mail => settings.mcp_mail_write.toggle(),
        _ => settings.mcp_send.toggle(),
      });
      card = card.child(Self::row(label, what, false).child(switch));
    }
    Self::group(title, card)
  }
}

impl Render for SettingsView {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let settings = cx.global::<Settings>().clone();
    let (width, height) = crate::conversation::modal_size(window.viewport_size(), SETTINGS_SIZE, (0.62, 0.78));

    let mut sections = v_flex().w(px(200.)).h_full().flex_shrink_0().p_3().gap_1().bg(rgb(BG)).child(
      div()
        .id("close-settings")
        .size(px(26.))
        .mb_2()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .hover(|button| button.bg(rgb(ROW_HOVER)))
        .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close)))
        .child(icon(Icon::X, 15., MUTED)),
    );
    for section in Section::ALL {
      let selected = section == self.section;
      sections = sections.child(
        h_flex()
          .id(section.title())
          .h(px(36.))
          .px_3()
          .gap_2p5()
          .items_center()
          .rounded(px(9.))
          .text_size(px(14.))
          .cursor_pointer()
          .when(selected, |item| item.bg(rgb(ACCENT_BG)))
          .when(!selected, |item| item.hover(|item| item.bg(rgb(ROW_HOVER))))
          .on_click(cx.listener(move |this, _, window, cx| {
            this.section = section;
            if section == Section::Rewrite {
              this.load_rewrite_models(true, window, cx);
              this.sync_rewrite(window, cx);
            }
            cx.notify();
          }))
          .child(icon(section.icon(), 16., if selected { TEXT } else { MUTED }))
          .child(section.title()),
      );
    }
    // The version opens What's New, which the window root draws over Settings.
    sections = sections.child(div().flex_1()).child(
      div()
        .id("settings-version")
        .px_3()
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .cursor_pointer()
        .hover(|version| version.text_color(rgb(TEXT)))
        .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::root::OpenWhatsNew), cx))
        .child(concat!("Working ", env!("CARGO_PKG_VERSION"))),
    );

    let body = match self.section {
      Section::Features => Self::features(&settings),
      Section::Appearance => self.appearance(settings).children(Self::desktop(cx)),
      Section::Layout => self.layout(settings),
      Section::Messages => self.messages(settings),
      Section::Status => Self::status(settings),
      Section::Notifications => self.notifications(settings, cx),
      Section::Calls => Self::calls(settings),
      Section::Rewrite => self.rewrite(settings, cx),
      Section::Agents => self.agents(settings, cx),
      #[cfg(target_os = "macos")]
      Section::Permissions => self.permissions(),
    };
    let content = v_flex()
      .id("settings-content")
      .flex_1()
      .h_full()
      .min_w_0()
      .p_6()
      .gap_5()
      .overflow_y_scroll()
      .child(
        div()
          .text_size(px(18.))
          .font_weight(FontWeight::SEMIBOLD)
          .child(self.section.title()),
      )
      .child(body);

    div()
      .absolute()
      .top_0()
      .left_0()
      .size_full()
      .flex()
      .items_center()
      .justify_center()
      .bg(rgba(0x00000073))
      .occlude()
      .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close)))
      .child(
        h_flex()
          .w(width)
          .h(height)
          .rounded(px(14.))
          .bg(rgb(POPOVER))
          .border_1()
          .border_color(rgb(BORDER))
          .overflow_hidden()
          .text_color(rgb(TEXT))
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .child(sections)
          .child(content),
      )
      .children(self.listen.as_ref().map(|(dialog, _)| dialog.clone().into_any_element()))
      .children(self.agent.as_ref().map(|(dialog, _)| dialog.clone().into_any_element()))
      .focus_trap("settings-trap", &self.focus)
  }
}

/// One theme in a theme select; its value is the index into [`THEMES`].
#[derive(Clone)]
struct ThemeChoice {
  index: usize,
  name: &'static str,
}

impl ThemeChoice {
  fn of(dark: bool) -> Vec<Self> {
    THEMES
      .iter()
      .enumerate()
      .filter(|(_, theme)| theme.dark == dark)
      .map(|(index, theme)| Self { index, name: theme.name })
      .collect()
  }
}

impl SelectItem for ThemeChoice {
  type Value = usize;

  fn title(&self) -> SharedString {
    self.name.into()
  }

  fn value(&self) -> &usize {
    &self.index
  }
}

impl SelectItem for ListWindow {
  type Value = Self;

  fn title(&self) -> SharedString {
    self.label().into()
  }

  fn value(&self) -> &Self {
    self
  }
}

impl SelectItem for ThreadView {
  type Value = Self;

  fn title(&self) -> SharedString {
    self.label().into()
  }

  fn value(&self) -> &Self {
    self
  }
}

/// One family in a font select; `None` is System. Titles show in the interface font: no preview of each family.
#[derive(Clone)]
struct FontChoice {
  family: Option<String>,
}

impl FontChoice {
  /// System, then every installed family.
  fn all(cx: &App) -> Vec<Self> {
    std::iter::once(None)
      .chain(settings::installed_fonts(cx).iter().cloned().map(Some))
      .map(|family| Self { family })
      .collect()
  }
}

impl SelectItem for FontChoice {
  type Value = Option<String>;

  fn title(&self) -> SharedString {
    self
      .family
      .as_ref()
      .map_or_else(|| "System".into(), |family| family.clone().into())
  }

  fn value(&self) -> &Option<String> {
    &self.family
  }
}

/// What Rewrite's Model and Effort selects were filled from.
#[derive(PartialEq)]
struct RewriteShown {
  tool: RewriteTool,
  /// `Arc` compares by pointer first, so a list already shown costs nothing to compare.
  models: Option<Result<Arc<Vec<Model>>, String>>,
  choice: RewriteChoice,
}

/// One model in Rewrite's Model select; `None` is Harness default.
#[derive(Clone)]
struct ModelChoice {
  id: Option<String>,
  label: String,
}

impl ModelChoice {
  fn harness_default() -> Self {
    Self { id: None, label: HARNESS_DEFAULT.into() }
  }
}

impl SelectItem for ModelChoice {
  type Value = Option<String>;

  fn title(&self) -> SharedString {
    self.label.clone().into()
  }

  fn value(&self) -> &Option<String> {
    &self.id
  }
}

/// One effort in Rewrite's Effort select, shown with its first letter upper-cased; `None` is Harness default.
#[derive(Clone)]
struct EffortChoice {
  effort: Option<String>,
  label: SharedString,
}

impl EffortChoice {
  fn new(effort: Option<String>) -> Self {
    let label = effort.as_deref().map_or_else(
      || HARNESS_DEFAULT.into(),
      |effort| {
        let mut chars = effort.chars();
        let first = chars.next().into_iter().flat_map(char::to_uppercase);
        first.chain(chars).collect::<String>().into()
      },
    );
    Self { effort, label }
  }
}

impl SelectItem for EffortChoice {
  type Value = Option<String>;

  fn title(&self) -> SharedString {
    self.label.clone()
  }

  fn value(&self) -> &Option<String> {
    &self.effort
  }
}
