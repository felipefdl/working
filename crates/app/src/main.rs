//! Working: native macOS Google Chat client.
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

mod call;
mod conversation;
mod emoji_picker;
mod home;
mod images;
mod menu;
mod message;
mod new_chat;
mod notify;
mod permissions;
mod rewrite;
mod root;
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
mod theme;
mod theme_picker;
mod ui;
mod updater;
mod util;
mod viewer;
mod welcome;

use gchat_core::Core;
use gpui_kit::assets::Assets;
use gpui_kit::component::TitleBar;
use gpui_kit::*;

use crate::images::Images;
use crate::root::{
  FindInChat, FindNext, FindPrevious, GoBack, GoForward, GoHome, Hide, HideOthers, Minimize, OpenSettings,
  OpenSwitcher, OpenThemePicker, Quit, RewriteDraft, RootView, ShowAll, TOP_BAR_HEIGHT, Zoom, menus,
};
use crate::settings::Settings;
use crate::store::Store;

/// What it takes to bring the main window back: the window itself, and what a new one is made from.
struct Main {
  window: AnyWindowHandle,
  core: Core,
  store: Entity<Store>,
}

impl Global for Main {}

/// A click on the Dock icon: the main window comes forward, or opens again after it was closed.
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

fn main() {
  let filter = if std::env::var("WORKING_TRACE").is_ok_and(|value| value == "1") {
    "debug"
  } else {
    "warn"
  };
  let _ = tracing_subscriber::fmt()
    .with_env_filter(filter)
    .with_writer(std::io::stderr)
    .try_init();
  let _ = std::fs::remove_dir_all(conversation::pasted_dir());
  let core = Core::start();
  let app = application().with_assets(Assets);
  app.on_reopen(reopen);
  app.run(move |cx| {
    init(cx);
    cx.set_global(Settings::load());
    settings::apply_appearance(cx);
    settings::keep_mcp(core.clone(), cx);
    cx.set_global(status::DndState::default());
    status::keep_presence(core.clone(), cx);
    cx.set_global(status::Mine::default());

    cx.bind_keys([
      KeyBinding::new("cmd-k", OpenSwitcher, None),
      KeyBinding::new("cmd-shift-t", OpenThemePicker, None),
      KeyBinding::new("cmd-n", crate::new_chat::OpenNewChat, None),
      KeyBinding::new("cmd-1", GoHome, None),
      // Xcode's pair: Cmd+[ and Cmd+] belong to the text field (outdent, indent).
      KeyBinding::new("ctrl-cmd-left", GoBack, None),
      KeyBinding::new("ctrl-cmd-right", GoForward, None),
      KeyBinding::new("cmd-,", OpenSettings, None),
      KeyBinding::new("cmd-shift-r", RewriteDraft, None),
      KeyBinding::new("cmd-f", FindInChat, None),
      KeyBinding::new("cmd-g", FindNext, None),
      KeyBinding::new("cmd-shift-g", FindPrevious, None),
      KeyBinding::new("cmd-q", Quit, None),
      KeyBinding::new("cmd-h", Hide, None),
      KeyBinding::new("alt-cmd-h", HideOthers, None),
      KeyBinding::new("cmd-m", Minimize, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
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
    cx.set_menus(menus());
    updater::start(cx);

    let images = Images::new(core.clone(), cx);
    let store = cx.new(|_| Store::new(core.clone(), images));
    let window = open_main(core.clone(), store.clone(), cx);
    cx.set_global(Main { window, core, store });
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
  options.titlebar = Some(TitlebarOptions {
    title: Some("Working".into()),
    appears_transparent: true,
    // The three 12px buttons, centered vertically in the top bar.
    traffic_light_position: Some(point(px(16.), px((TOP_BAR_HEIGHT - 12.) / 2.))),
  });
  options.window_bounds = Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(800.)), cx)));
  options.window_min_size = Some(size(px(800.), px(500.)));
  open_window(options, cx, |window, cx| cx.new(|cx| RootView::new(core, store, window, cx)))
    .unwrap_or_else(|_| {
      tracing::error!("failed to open the main window");
      std::process::exit(1);
    })
    .0
}
