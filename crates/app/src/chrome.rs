//! Window chrome: every app window draws its own top bar in place of the system title bar. On macOS the traffic
//! lights sit in it; on Linux and Windows the kit's title bar carries it and ends it with the minimize, maximize, and
//! close buttons.

#[cfg(not(target_os = "macos"))]
use gpui_kit::component::TitleBar;
use gpui_kit::*;

/// Gives `options` a title bar of our own, `bar_height` tall: the system's is hidden and, on macOS, the three 12px
/// traffic lights are centered vertically in ours.
pub fn set_title_bar(options: &mut WindowOptions, title: &'static str, bar_height: f32) {
  options.titlebar = Some(TitlebarOptions {
    title: Some(title.into()),
    appears_transparent: true,
    traffic_light_position: cfg!(target_os = "macos").then(|| point(px(16.), px((bar_height - 12.) / 2.))),
  });
  // Linux defaults to the window manager's decorations, a second title bar above ours. Client-side ones let the kit's
  // title bar draw the window's buttons; where the compositor insists on its own, the kit leaves them out. The app id
  // (the X11 `WM_CLASS`) is the desktop entry's name, `working.desktop` (`scripts/package-linux.sh`), by which docks
  // and task bars put the window under the app's name and icon. An AppImage run straight from a folder has no
  // desktop entry installed, so the window carries the icon too.
  #[cfg(target_os = "linux")]
  {
    options.window_decorations = Some(WindowDecorations::Client);
    options.app_id = Some("working".into());
    options.icon = WINDOW_ICON.clone();
  }
}

/// The app icon as the X11 window icon, at the size docks draw it.
#[cfg(target_os = "linux")]
static WINDOW_ICON: std::sync::LazyLock<Option<std::sync::Arc<image::RgbaImage>>> = std::sync::LazyLock::new(|| {
  let icon = image::load_from_memory(include_bytes!("../../../logo/icon-light.png")).ok()?;
  Some(std::sync::Arc::new(icon.thumbnail(256, 256).into_rgba8()))
});

/// Where a top bar's content starts: `mac` on macOS, clear of the traffic lights, and a plain margin elsewhere.
pub const fn inset(mac: f32) -> f32 {
  if cfg!(target_os = "macos") { mac } else { 16. }
}

/// A window's top bar, `height` tall, from the row of its content (which starts at [`inset`]): dragging it moves the
/// window and a double click zooms it. Off macOS it sits in the kit's title bar, which adds the window's buttons;
/// there every control in the row has to `occlude()`, or Windows takes its clicks for the drag area.
pub fn top_bar<E: IntoElement + Styled + InteractiveElement>(row: E, height: f32) -> AnyElement {
  #[cfg(target_os = "macos")]
  {
    row
      .h(px(height))
      .on_mouse_down(MouseButton::Left, |event, window, _| {
        if event.click_count == 2 {
          window.titlebar_double_click();
        } else {
          window.start_window_move();
        }
      })
      .into_any_element()
  }
  #[cfg(not(target_os = "macos"))]
  {
    TitleBar::new()
      .h(px(height))
      .pl_0()
      .bg(transparent_black())
      .border_b_0()
      .child(row.h_full().flex_1())
      .into_any_element()
  }
}
