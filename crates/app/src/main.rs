//! Working: native Google Chat client for macOS, Linux, and Windows.
// The release build is a GUI program on Windows, so no console window opens beside it; `console::attach` hands its
// log to the terminal it was started from.
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![allow(
  clippy::unreadable_literal,
  clippy::needless_pass_by_ref_mut,
  clippy::needless_pass_by_value,
  clippy::too_many_lines,
  clippy::too_many_arguments,
  clippy::as_conversions,
  clippy::cast_precision_loss,
  clippy::cast_possible_truncation,
  clippy::cast_sign_loss,
  clippy::indexing_slicing,
  clippy::string_slice,
  clippy::used_underscore_binding,
  clippy::unused_self,
  clippy::type_complexity,
  clippy::struct_excessive_bools,
  clippy::similar_names,
  clippy::semicolon_if_nothing_returned,
  clippy::option_if_let_else,
  clippy::derive_partial_eq_without_eq,
  clippy::unnecessary_sort_by,
  clippy::cast_possible_wrap,
  clippy::assigning_clones,
  clippy::match_same_arms,
  clippy::or_fun_call,
  reason = "GPUI view signatures, theme hex colors, and layout math"
)]

mod calendar;
mod calendar_grid;
mod calendar_layout;
#[cfg(not(target_os = "linux"))]
mod call;
#[cfg(not(target_os = "linux"))]
mod call_chat;
/// `WebKitGTK` as Fedora builds it has no WebRTC (`RTCPeerConnection` stays undefined with the setting on), so a
/// call window could never join: on Linux a meeting opens in the default browser.
#[cfg(target_os = "linux")]
mod call {
  pub fn open(url: &str, cx: &mut gpui_kit::App) {
    cx.open_url(url);
  }

  /// No call window, so no notification is a call's.
  pub const fn show_chat(_: &str, _: &mut gpui_kit::App) -> bool {
    false
  }
}
mod chrome;
mod clock;
mod completion;
#[cfg(target_os = "windows")]
mod console;
mod conv_list;
mod conversation;
#[cfg(target_os = "linux")]
mod desktop_entry;
mod emoji_font;
mod emoji_picker;
mod event_card;
mod event_editor;
mod format_logic;
mod home;
mod images;
mod keys;
mod mail;
mod mail_body;
mod mail_compose;
mod mail_labels;
mod mail_lists;
mod mail_logic;
mod mail_page;
mod mail_write;
mod meet;
mod meet_body;
mod meet_logic;
mod menu;
mod message;
mod new_chat;
mod notify;
#[cfg(target_os = "macos")]
mod permissions;
mod rail;
mod receipt_logic;
mod rewrite;
mod root;
#[cfg(target_os = "macos")]
mod screen;
mod session;
mod settings;
mod settings_view;
mod shortcut;
mod sidebar;
mod status;
mod status_menu;
mod store;
mod switcher;
mod text_panel;
mod theme;
mod theme_picker;
mod ui;
mod updater;
mod util;
mod viewer;
mod webview;
mod welcome;

use gchat_core::Core;
use gpui_kit::assets::Assets;
use gpui_kit::component::TitleBar;
use gpui_kit::*;

use crate::images::Images;
use crate::root::{Quit, RootView, TOP_BAR_HEIGHT};
use crate::settings::Settings;
use crate::store::Store;

/// What it takes to bring the main window back: the window itself, and what a new one is made from.
#[cfg(target_os = "macos")]
struct Main {
  window: AnyWindowHandle,
  core: Core,
  store: Entity<Store>,
}

#[cfg(target_os = "macos")]
impl Global for Main {}

/// A click on the Dock icon: the main window comes forward, or opens again after it was closed.
#[cfg(target_os = "macos")]
fn reopen(cx: &mut App) {
  let Some(main) = cx.try_global::<Main>() else {
    return;
  };
  let (window, core, store) = (main.window, main.core.clone(), main.store.clone());
  if window.update(cx, |_, window, _| window.activate_window()).is_err() {
    // The open conversation belonged to the window that is gone.
    store.update(cx, store::Store::go_home);
    let window = open_main(core.clone(), store.clone(), cx);
    cx.set_global(Main { window, core, store });
  }
  cx.activate(true);
}

/// Runs a menu bar command on the main window whatever has the focus, bringing the window forward (or back) first.
#[cfg(target_os = "macos")]
fn on_main(cx: &mut App, run: root::MenuCommand) {
  reopen(cx);
  let Some(window) = cx.try_global::<Main>().and_then(|main| main.window.downcast::<RootView>()) else {
    return;
  };
  let _ = window.update(cx, run);
}

fn main() {
  #[cfg(target_os = "windows")]
  {
    console::attach();
    notify::set_app_id();
  }
  // Only the app's own crates trace: the HTTP stack's debug lines print `Set-Cookie` values.
  let filter = if std::env::var("WORKING_TRACE").is_ok_and(|value| value == "1") {
    "warn,working=debug,gchat_core=debug"
  } else {
    "warn"
  };
  let _ = tracing_subscriber::fmt()
    .with_env_filter(filter)
    .with_writer(std::io::stderr)
    .try_init();
  let _ = std::fs::remove_dir_all(conversation::pasted_dir());
  clock::warm();
  let core = Core::start();
  // The webviews (sign-in and Meet) are WebKitGTK, which runs on X11 only: a Wayland session runs the app through
  // XWayland.
  #[cfg(target_os = "linux")]
  let app = gpui_kit::platform::linux(WindowingModes::X11).with_assets(Assets);
  #[cfg(not(target_os = "linux"))]
  let app = application().with_assets(Assets);
  #[cfg(target_os = "macos")]
  app.on_reopen(reopen);
  app.run(move |cx| {
    init(cx);
    #[cfg(target_os = "linux")]
    emoji_font::register(cx);
    cx.set_global(Settings::load());
    #[cfg(target_os = "linux")]
    desktop_entry::start(cx);
    settings::apply_appearance(cx);
    settings::keep_mcp(core.clone(), cx);
    cx.set_global(status::DndState::default());
    status::keep_presence(core.clone(), cx);
    cx.set_global(status::Mine::default());
    // Quitting stops the core, which writes the history changes it still holds back to the disk cache. The wait sits
    // before GPUI's own short wait on quit handlers, and is bounded so a stuck disk never holds the quit up.
    cx.on_app_quit({
      let core = core.clone();
      move |_| {
        core.shutdown(std::time::Duration::from_millis(1500));
        async {}
      }
    })
    .detach();

    keys::bind(cx);
    cx.on_action(|_: &Quit, cx| cx.quit());
    #[cfg(target_os = "macos")]
    {
      use crate::root::{Hide, HideOthers, Minimize, ShowAll, Zoom, bind_menu_commands, menus};
      cx.on_action(|_: &Hide, cx| cx.hide());
      cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
      cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
      cx.on_action(|_: &Minimize, cx| {
        if let Some(window) = cx.active_window() {
          let _ = window.update(cx, |_, window, _| window.minimize_window());
        }
      });
      cx.on_action(|_: &Zoom, cx| {
        if let Some(window) = cx.active_window() {
          let _ = window.update(cx, |_, window, _| window.zoom_window());
        }
      });
      bind_menu_commands(cx, on_main);
      // Only macOS has a menu bar of its own: elsewhere the main window's top bar has a button with these menus.
      cx.set_menus(menus(cx));
      // The menu bar shows each view's number key, and Calendar, Meet, and Mail only while each is on: it is written
      // again when the keys change hands or a feature goes on or off.
      let shown = |settings: &Settings| (settings.hotkeys.clone(), settings.features());
      let mut written = shown(cx.global::<Settings>());
      cx.observe_global::<Settings>(move |cx| {
        let now = shown(cx.global::<Settings>());
        if now != written {
          written = now;
          cx.set_menus(menus(cx));
        }
      })
      .detach();
    }
    updater::start(cx);

    let images = Images::new(core.clone(), cx);
    let store = cx.new(|_| Store::new(core.clone(), images));
    #[cfg(target_os = "macos")]
    {
      let window = open_main(core.clone(), store.clone(), cx);
      cx.set_global(Main { window, core, store });
    }
    // Only macOS brings a closed main window back (from the Dock): elsewhere closing it quits.
    #[cfg(not(target_os = "macos"))]
    {
      let window = open_main(core, store, cx);
      cx.on_window_closed(move |cx, closed| {
        if closed == window.window_id() {
          cx.quit();
        }
      })
      .detach();
    }
    // `Working https://meet.google.com/<code>` also opens that meeting.
    if let Some(url) = std::env::args()
      .nth(1)
      .filter(|arg| arg.starts_with("https://meet.google.com/"))
    {
      call::open(&url, cx);
    }
    cx.activate(true);
  });
}

fn open_main(core: Core, store: Entity<Store>, cx: &mut App) -> AnyWindowHandle {
  let mut options = TitleBar::window_options();
  chrome::set_title_bar(&mut options, "Working", TOP_BAR_HEIGHT);
  options.window_bounds = Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(800.)), cx)));
  options.window_min_size = Some(size(px(800.), px(500.)));
  open_window(options, cx, |window, cx| cx.new(|cx| RootView::new(core, store, window, cx)))
    .unwrap_or_else(|_| {
      tracing::error!("failed to open the main window");
      std::process::exit(1);
    })
    .0
}
