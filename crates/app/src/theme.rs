//! Colors and metrics. Every color has a dark and a light value; `rgb` picks the one for the appearance in use.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::Rgba;

/// A named color of the app's look.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Tone {
  dark: u32,
  light: u32,
}

const fn tone(dark: u32, light: u32) -> Tone {
  Tone { dark, light }
}

static LIGHT: AtomicBool = AtomicBool::new(false);

/// Switches every `Tone` to its light or dark value. Windows need a refresh afterwards.
pub fn set_light(light: bool) {
  LIGHT.store(light, Ordering::Relaxed);
}

pub fn is_light() -> bool {
  LIGHT.load(Ordering::Relaxed)
}

pub fn rgb(tone: Tone) -> Rgba {
  gpui_kit::rgb(if LIGHT.load(Ordering::Relaxed) {
    tone.light
  } else {
    tone.dark
  })
}

pub const BG: Tone = tone(0x131314, 0xf0f4f9);
pub const PANEL: Tone = tone(0x1e1f20, 0xffffff);
/// Floating surfaces: menus, the quick switcher, the settings window.
pub const POPOVER: Tone = tone(0x232426, 0xffffff);
pub const ROW_HOVER: Tone = tone(0x2a2b2d, 0xe2e7ee);
pub const ROW_SELECTED: Tone = tone(0x2c3b52, 0xc2e7ff);
pub const FIELD: Tone = tone(0x2a2b2d, 0xe4e9f0);
pub const BORDER: Tone = tone(0x3c3d40, 0xd3d7dd);
pub const TEXT: Tone = tone(0xe3e3e3, 0x1f1f1f);
pub const MUTED: Tone = tone(0x9aa0a6, 0x5f6368);
pub const ACCENT: Tone = tone(0x8ab4f8, 0x0b57d0);
pub const ACCENT_BG: Tone = tone(0x2f4366, 0xd3e3fd);
pub const BUBBLE: Tone = tone(0x2d2e30, 0xf0f4f9);
pub const BUBBLE_OWN: Tone = tone(0x4b403c, 0xeadfd8);
pub const CODE_BG: Tone = tone(0x161617, 0xe4e7ec);
pub const PLACEHOLDER: Tone = tone(0x2a2b2d, 0xe1e3e8);
/// Background of the words a search matched.
pub const MARK: Tone = tone(0x5c4a12, 0xfde293);
/// Background of a microphone or camera button that is off.
pub const OFF_BG: Tone = tone(0x5c2b29, 0xf9dedc);
pub const GREEN: Tone = tone(0x34a853, 0x1e8e3e);
pub const AMBER: Tone = tone(0xfbbc04, 0xe37400);
pub const RED: Tone = tone(0xea4335, 0xd93025);
/// Sidebar names: unread or selected, and everything else.
pub const TEXT_STRONG: Tone = tone(0xffffff, 0x1f1f1f);
pub const TEXT_DIM: Tone = tone(0xb1b5ba, 0x444746);
/// Drawn over pictures and video, which stay dark in both appearances.
pub const WHITE: Tone = tone(0xffffff, 0xffffff);

pub const SIDEBAR_WIDTH: f32 = 264.0;
pub const MONO: &str = "Menlo";
