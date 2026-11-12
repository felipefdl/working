//! The Settings modal over the main window: sections on the left, their options on the right.

use std::time::Duration;

use gchat_core::{Command, Core, DndSchedule, DndSchedules, McpStatus};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::notify;
use crate::permissions::{self, Media, Status};
use crate::screen;
use crate::settings::{self, ActivityStatus, Appearance, Density, McpState, RewriteTool, Settings, Sound};
use crate::status::{self, DndState};
use crate::theme::{ACCENT, ACCENT_BG, BG, BORDER, BUBBLE, FIELD, GREEN, MUTED, POPOVER, RED, ROW_HOVER, TEXT, rgb};
use crate::ui::{Icon, icon};

const CAMERA_PANE: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera";
const MICROPHONE_PANE: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";
const SCREEN_PANE: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
const NOTIFICATIONS_PANE: &str = "x-apple.systempreferences:com.apple.Notifications-Settings.extension";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
  Appearance,
  Status,
  Notifications,
  Calls,
  Rewrite,
  Agents,
  Permissions,
}

impl Section {
  const ALL: [Self; 7] = [
    Self::Appearance,
    Self::Status,
    Self::Notifications,
    Self::Calls,
    Self::Rewrite,
    Self::Agents,
    Self::Permissions,
  ];

  const fn title(self) -> &'static str {
    match self {
      Self::Appearance => "Appearance",
      Self::Status => "Activity status",
      Self::Notifications => "Notifications",
      Self::Calls => "Calls",
      Self::Rewrite => "Rewrite",
      Self::Agents => "Agents",
      Self::Permissions => "Permissions",
    }
  }

  const fn icon(self) -> Icon {
    match self {
      Self::Appearance => Icon::Monitor,
      Self::Status => Icon::Activity,
      Self::Notifications => Icon::Bell,
      Self::Calls => Icon::Video,
      Self::Rewrite => Icon::Sparkles,
      Self::Agents => Icon::Bot,
      Self::Permissions => Icon::Shield,
    }
  }
}

/// What an agent's settings need to reach the MCP server, each copied by a button of its own.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
  /// One address with the token in it.
  Link,
  /// The shell command that adds the server to Claude Code.
  ClaudeCode,
  Token,
}

impl Access {
  fn text(self, port: u16) -> std::io::Result<String> {
    let token = gchat_core::mcp::token()?;
    let endpoint = format!("http://127.0.0.1:{port}/mcp");
    Ok(match self {
      Self::Link => format!("{endpoint}/{token}"),
      Self::ClaudeCode => {
        format!("claude mcp add --transport http working {endpoint} --header \"Authorization: Bearer {token}\"")
      },
      Self::Token => token,
    })
  }
}

/// What macOS allows right now. `notifications` is `None` when the app runs outside its bundle, where macOS
/// offers no notifications at all.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Allowed {
  camera: Status,
  microphone: Status,
  screen: Status,
  notifications: Option<Status>,
}

pub enum SettingsEvent {
  Close,
}

pub struct SettingsView {
  core: Core,
  section: Section,
  allowed: Allowed,
  /// What was just copied: its button says so for a moment.
  copied: Option<Access>,
  /// The do-not-disturb schedule whose days and times are open for changing.
  editing: Option<usize>,
  rewrite_instruction: Entity<TextareaState>,
  _copied_reset: Option<Task<()>>,
  _refresh: Task<()>,
  _globals: [Subscription; 2],
  _rewrite_sub: Subscription,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
  pub fn new(core: Core, window: &mut Window, cx: &mut Context<Self>) -> Self {
    // Google Chat is where the schedules live: show what it has now.
    core.send(Command::LoadDnd);
    // The user changes these in System Settings while this is open: keep the list current.
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
    let rewrite_instruction =
      cx.new(|cx| TextareaState::new(window, cx).auto_grow(6, 16).placeholder("How to rewrite"));
    rewrite_instruction.update(cx, |state, cx| state.set_value(instruction, window, cx));
    let rewrite_sub = cx.subscribe_in(&rewrite_instruction, window, |_, field, event: &InputEvent, _, cx| {
      if matches!(event, InputEvent::Change) {
        let text = field.read(cx).value().to_string();
        Settings::update(cx, |settings| settings.rewrite_instruction = text);
      }
    });
    Self {
      core,
      section: Section::Appearance,
      allowed: Self::read_allowed(),
      copied: None,
      editing: None,
      rewrite_instruction,
      _copied_reset: None,
      _refresh: refresh,
      _globals: [
        cx.observe_global::<McpState>(|_, cx| cx.notify()),
        cx.observe_global::<DndState>(|_, cx| cx.notify()),
      ],
      _rewrite_sub: rewrite_sub,
    }
  }

  /// Opens on Notifications, for the status button.
  pub fn at_notifications(core: Core, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let mut view = Self::new(core, window, cx);
    view.section = Section::Notifications;
    view
  }

  fn copy(&mut self, access: Access, cx: &mut Context<Self>) {
    match access.text(cx.global::<Settings>().mcp_port) {
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

  /// A small button at the end of a row.
  fn pill(id: impl Into<ElementId>, label: &'static str) -> Stateful<Div> {
    div()
      .id(id)
      .px_3()
      .py_1()
      .flex_shrink_0()
      .rounded_full()
      .bg(rgb(FIELD))
      .text_size(px(12.))
      .cursor_pointer()
      .hover(|button| button.opacity(0.85))
      .child(label)
  }

  /// Everything that can be read at once; notifications answer through a callback.
  fn read_allowed() -> Allowed {
    Allowed {
      camera: permissions::media(Media::Camera),
      microphone: permissions::media(Media::Microphone),
      screen: permissions::screen(),
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
          .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(detail.into())),
      )
  }

  fn toggle(id: impl Into<ElementId>, on: bool) -> Stateful<Div> {
    div()
      .id(id)
      .w(px(40.))
      .h(px(24.))
      .px(px(3.))
      .flex()
      .flex_shrink_0()
      .items_center()
      .when(on, gpui_kit::Styled::justify_end)
      .rounded_full()
      .cursor_pointer()
      .bg(rgb(if on { ACCENT } else { BORDER }))
      .child(div().size(px(18.)).rounded_full().bg(gpui_kit::rgb(0xffffff)))
  }

  fn appearance_picker(current: Appearance) -> Div {
    let mut picker = h_flex().flex_shrink_0().p(px(3.)).gap_1().rounded(px(9.)).bg(rgb(BG));
    for (label, value) in [
      ("System", Appearance::System),
      ("Light", Appearance::Light),
      ("Dark", Appearance::Dark),
    ] {
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
          .when(selected, |option| option.bg(rgb(ACCENT_BG)))
          .on_click(move |_, _, cx| Settings::update(cx, |settings| settings.appearance = value))
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
      .bg(rgb(BG));
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
          .when(selected, |option| option.bg(rgb(ACCENT_BG)))
          .on_click(move |_, _, cx| Settings::update(cx, |settings| settings.rewrite = value))
          .child(value.label()),
      );
    }
    picker
  }

  /// A density choice: a small drawing of the layout over its radio button, as in Google Chat.
  fn density_option(value: Density, current: Density) -> Stateful<Div> {
    let selected = value == current;
    let (id, label) = match value {
      Density::Comfortable => ("comfortable", "Comfortable"),
      Density::Compact => ("compact", "Compact"),
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
      .w(px(170.))
      .h(px(104.))
      .p_3()
      .gap_1p5()
      .justify_center()
      .rounded(px(10.))
      .border_2()
      .border_color(rgb(if selected { ACCENT } else { BORDER }))
      .bg(rgb(BG));
    let drawing = match value {
      Density::Comfortable => frame
        .child(left(52., true))
        .child(right(64.))
        .child(right(40.))
        .child(left(44., true))
        .child(right(48.)),
      Density::Compact => frame
        .child(left(20., false))
        .child(left(52., true))
        .child(left(64., false))
        .child(left(44., true))
        .child(left(28., false)),
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
      .on_click(move |_, _, cx| Settings::update(cx, |settings| settings.density = value))
      .child(drawing)
      .child(
        h_flex()
          .gap_2()
          .items_center()
          .child(radio)
          .child(div().text_size(px(13.)).child(label)),
      )
  }

  /// A line of the permissions list: its state, and a button that asks macOS or opens the right pane of
  /// System Settings.
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
        .bg(rgb(FIELD))
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

  fn appearance(settings: Settings) -> Div {
    v_flex()
      .gap_5()
      .child(Self::group(
        "Theme",
        Self::card().child(
          Self::row("Color mode", "System follows the macOS appearance.", true)
            .child(Self::appearance_picker(settings.appearance)),
        ),
      ))
      .child(Self::group(
        "Density",
        Self::card().py_4().child(
          h_flex()
            .gap_8()
            .child(Self::density_option(Density::Comfortable, settings.density))
            .child(Self::density_option(Density::Compact, settings.density)),
        ),
      ))
  }

  fn calls(settings: Settings) -> Div {
    v_flex()
      .gap_5()
      .child(Self::group(
        "Meet",
        Self::card()
          .child(
            Self::row(
              "Built-in Google Meet",
              "Open meetings in Working. Off opens them in the browser.",
              true,
            )
            .child(
              Self::toggle("meet-builtin", settings.meet_builtin)
                .on_click(|_, _, cx| Settings::update(cx, |settings| settings.meet_builtin = !settings.meet_builtin)),
            ),
          )
          .child(
            Self::row(
              "Open meetings automatically",
              "After you start a meeting, join it. Off only posts the link.",
              false,
            )
            .child(
              Self::toggle("meet-auto-open", settings.meet_auto_open).on_click(|_, _, cx| {
                Settings::update(cx, |settings| settings.meet_auto_open = !settings.meet_auto_open)
              }),
            ),
          ),
      ))
      .child(Self::group(
        "Camera",
        Self::card().child(
          Self::row(
            "Camera always off",
            "Calls never get the camera, and have no camera button.",
            true,
          )
          .child(
            Self::toggle("camera-off", settings.camera_off)
              .on_click(|_, _, cx| Settings::update(cx, |settings| settings.camera_off = !settings.camera_off)),
          ),
        ),
      ))
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
        .bg(rgb(FIELD))
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
        "Online while you use this Mac, away after 10 minutes without touching it.",
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
      .child(Self::row("From", "Every day, in this Mac's time.", true).child(Self::stepper(
        "hours-start",
        settings.hours_start,
        |minute, cx| Settings::update(cx, |settings| settings.hours_start = minute),
      )))
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
      let selected = sound == settings.sound;
      sounds = sounds.child(
        div()
          .id(sound.label())
          .px_3()
          .py_1()
          .rounded_full()
          .bg(rgb(if selected { ACCENT_BG } else { FIELD }))
          .text_size(px(12.))
          .cursor_pointer()
          .hover(|button| button.opacity(0.85))
          .on_click(move |_, _, cx| {
            Settings::update(cx, |settings| settings.sound = sound);
            sound.preview();
          })
          .child(sound.label()),
      );
    }
    let out = v_flex()
      .gap_5()
      .child(Self::group("Notification sound", Self::card().child(sounds)));

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
                  schedule.enabled = !schedule.enabled;
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
            this.change_dnd(cx, |dnd| dnd.match_time_zone = !dnd.match_time_zone);
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
          .bg(rgb(if on { ACCENT_BG } else { FIELD }))
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

  fn permissions(&self) -> Div {
    let Allowed {
      camera,
      microphone,
      screen,
      notifications,
    } = self.allowed;
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
          "screen",
          "Screen & System Audio Recording",
          "Sharing your screen at its full sharpness.",
          Some(screen),
          false,
          move |cx| {
            if screen != Status::Allowed {
              screen::request_permission();
            }
            cx.open_url(SCREEN_PANE);
          },
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

  fn rewrite(&self, settings: Settings) -> Div {
    Self::card()
      .child(
        Self::row("Tool", "The CLI on this Mac that rewrites a draft. Off hides the button.", true)
          .child(Self::rewrite_picker(settings.rewrite)),
      )
      .child(
        v_flex()
          .py_3()
          .gap_1()
          .border_t_1()
          .border_color(rgb(BORDER))
          .child(div().text_size(px(14.)).child("Instructions"))
          .child(
            div()
              .text_size(px(12.))
              .text_color(rgb(MUTED))
              .child("Added to the rewrite prompt."),
          )
          .child(
            div()
              .mt_1()
              .px_2()
              .py_1()
              .rounded(px(9.))
              .bg(rgb(BG))
              .child(Textarea::new(&self.rewrite_instruction).appearance(false).bordered(false)),
          ),
      )
  }

  fn agents(&self, settings: Settings, cx: &mut Context<Self>) -> Div {
    let server = Self::card()
      .child(
        Self::row(
          "MCP server",
          "Lets AI agents on this Mac read your Google Chat through Working.",
          true,
        )
        .child(
          Self::toggle("mcp", settings.mcp)
            .on_click(|_, _, cx| Settings::update(cx, |settings| settings.mcp = !settings.mcp)),
        ),
      )
      .child(
        Self::row(
          "Let agents send messages",
          "Adds a tool that posts as you. Without it agents only read.",
          false,
        )
        .child(
          Self::toggle("mcp-send", settings.mcp_send)
            .on_click(|_, _, cx| Settings::update(cx, |settings| settings.mcp_send = !settings.mcp_send)),
        ),
      );
    let out = v_flex().gap_5().child(Self::group("Server", server));
    if !settings.mcp {
      return out;
    }

    let (color, state) = match &cx.global::<McpState>().0 {
      McpStatus::Running { port } => (GREEN, format!("Running at http://127.0.0.1:{port}/mcp, for this Mac only.")),
      McpStatus::Failed(error) => {
        let port = settings.mcp_port;
        (
          RED,
          format!(
            "Could not listen on port {port}: {error}. Set another \"mcp_port\" in ~/.config/working/settings.json."
          ),
        )
      },
      McpStatus::Off => (MUTED, "Starts once you are signed in.".to_string()),
    };
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
    let connect = Self::card()
      .child(Self::row("Status", state, true).child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(rgb(color))))
      .child(
        Self::row(
          "Link",
          "One address with the token inside, for an agent that only takes a URL.",
          false,
        )
        .child(copy(Access::Link, "copy-link", "Copy link", cx)),
      )
      .child(
        Self::row("Claude Code", "The command that adds this server to Claude Code.", false).child(copy(
          Access::ClaudeCode,
          "copy-claude",
          "Copy command",
          cx,
        )),
      )
      .child(
        Self::row(
          "Token",
          "For an agent that takes the address and an Authorization: Bearer header.",
          false,
        )
        .child(copy(Access::Token, "copy-token", "Copy token", cx)),
      )
      .child(
        Self::row("New token", "Cuts off every agent that has the current one.", false)
          .child(Self::pill("new-token", "Replace").on_click(|_, _, cx| settings::replace_mcp_token(cx))),
      );
    out.child(Self::group("Connect an agent", connect))
  }
}

impl Render for SettingsView {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let settings = cx.global::<Settings>().clone();

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
          .on_click(cx.listener(move |this, _, _, cx| {
            this.section = section;
            cx.notify();
          }))
          .child(icon(section.icon(), 16., if selected { TEXT } else { MUTED }))
          .child(section.title()),
      );
    }

    let body = match self.section {
      Section::Appearance => Self::appearance(settings),
      Section::Status => Self::status(settings),
      Section::Notifications => self.notifications(settings, cx),
      Section::Calls => Self::calls(settings),
      Section::Rewrite => self.rewrite(settings),
      Section::Agents => self.agents(settings, cx),
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
          .w(px(740.))
          .h(px(460.))
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
  }
}
