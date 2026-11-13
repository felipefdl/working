//! Shared small widgets: avatars, icons, and switches.

use gchat_core::{Group, GroupKind, PersonStatus};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::images::Images;
use crate::theme::{ACCENT, BG, FIELD, GREEN, MUTED, RED, TEXT, Tone, WHITE, rgb, switch_off};
use crate::util;

/// Lucide icons embedded as SVG so no asset bundle is needed at runtime.
#[derive(Clone, Copy)]
pub enum Icon {
  ArrowDownAZ,
  User,
  Users,
  PanelLeft,
  PanelLeftDashed,
  PanelRight,
  PanelRightDashed,
  ArrowLeft,
  History,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  ChevronsUpDown,
  Search,
  House,
  Pin,
  File,
  X,
  ZoomIn,
  ZoomOut,
  Fit,
  Download,
  Copy,
  ExternalLink,
  Play,
  Phone,
  Video,
  VideoOff,
  Mic,
  MicOff,
  ScreenShare,
  Hand,
  PhoneOff,
  Monitor,
  Shield,
  Bot,
  Activity,
  Bell,
  Check,
  SmilePlus,
  Pencil,
  Reply,
  Thread,
  More,
  Plus,
  Sparkles,
  AtSign,
  Star,
  Calendar,
  ChevronLeft,
  Clock,
  MapPin,
  Text,
  Repeat,
  Trash,
  List,
  Link,
}

macro_rules! lucide {
  ($body:literal) => {
    concat!(
      r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="black" "#,
      r#"stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#,
      $body,
      "</svg>"
    )
    .as_bytes()
  };
}

impl Icon {
  const fn svg(self) -> &'static [u8] {
    match self {
      Self::ArrowLeft => lucide!(r#"<path d="m12 19-7-7 7-7"/><path d="M19 12H5"/>"#),
      Self::ChevronDown => lucide!(r#"<path d="m6 9 6 6 6-6"/>"#),
      Self::ChevronRight => lucide!(r#"<path d="m9 18 6-6-6-6"/>"#),
      Self::ChevronUp => lucide!(r#"<path d="m18 15-6-6-6 6"/>"#),
      Self::ChevronsUpDown => lucide!(r#"<path d="m7 15 5 5 5-5"/><path d="m7 9 5-5 5 5"/>"#),
      Self::Search => lucide!(r#"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/>"#),
      Self::House => lucide!(
        r#"<path d="M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8"/><path d="M3 10a2 2 0 0 1 .709-1.528l7-5.999a2 2 0 0 1 2.582 0l7 5.999A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"#
      ),
      Self::Pin => lucide!(
        r#"<path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/>"#
      ),
      Self::File => lucide!(
        r#"<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/>"#
      ),
      Self::X => lucide!(r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#),
      Self::ZoomIn => lucide!(
        r#"<circle cx="11" cy="11" r="8"/><line x1="21" x2="16.65" y1="21" y2="16.65"/><line x1="11" x2="11" y1="8" y2="14"/><line x1="8" x2="14" y1="11" y2="11"/>"#
      ),
      Self::ZoomOut => lucide!(
        r#"<circle cx="11" cy="11" r="8"/><line x1="21" x2="16.65" y1="21" y2="16.65"/><line x1="8" x2="14" y1="11" y2="11"/>"#
      ),
      Self::Fit => lucide!(
        r#"<path d="M8 3H5a2 2 0 0 0-2 2v3"/><path d="M21 8V5a2 2 0 0 0-2-2h-3"/><path d="M3 16v3a2 2 0 0 0 2 2h3"/><path d="M16 21h3a2 2 0 0 0 2-2v-3"/>"#
      ),
      Self::Download => lucide!(
        r#"<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" x2="12" y1="15" y2="3"/>"#
      ),
      Self::Copy => lucide!(
        r#"<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#
      ),
      Self::ExternalLink => lucide!(
        r#"<path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>"#
      ),
      Self::Play => lucide!(r#"<polygon points="6 3 20 12 6 21 6 3" fill="black"/>"#),
      Self::Phone => lucide!(
        r#"<path d="M13.832 16.568a1 1 0 0 0 1.213-.303l.355-.465A2 2 0 0 1 17 15h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2A18 18 0 0 1 2 4a2 2 0 0 1 2-2h3a2 2 0 0 1 2 2v3a2 2 0 0 1-.8 1.6l-.468.351a1 1 0 0 0-.292 1.233 14 14 0 0 0 6.392 6.384"/>"#
      ),
      Self::Video => lucide!(
        r#"<path d="m16 13 5.223 3.482a.5.5 0 0 0 .777-.416V7.87a.5.5 0 0 0-.752-.432L16 10.5"/><rect x="2" y="6" width="14" height="12" rx="2"/>"#
      ),
      Self::VideoOff => lucide!(
        r#"<path d="M10.66 6H14a2 2 0 0 1 2 2v2.5l5.248-3.062A.5.5 0 0 1 22 7.87v8.196"/><path d="M16 16a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h2"/><path d="m2 2 20 20"/>"#
      ),
      Self::Mic => lucide!(
        r#"<path d="M12 19v3"/><path d="M19 10v2a7 7 0 0 1-14 0v-2"/><rect x="9" y="2" width="6" height="13" rx="3"/>"#
      ),
      Self::MicOff => lucide!(
        r#"<path d="M12 19v3"/><path d="M15 9.34V5a3 3 0 0 0-5.68-1.33"/><path d="M16.95 16.95A7 7 0 0 1 5 12v-2"/><path d="M18.89 13.23A7 7 0 0 0 19 12v-2"/><path d="m2 2 20 20"/><path d="M9 9v3a3 3 0 0 0 5.12 2.12"/>"#
      ),
      Self::ScreenShare => lucide!(
        r#"<path d="m9 10 3-3 3 3"/><path d="M12 13V7"/><rect width="20" height="14" x="2" y="3" rx="2"/><path d="M12 17v4"/><path d="M8 21h8"/>"#
      ),
      Self::Hand => lucide!(
        r#"<path d="M18 11V6a2 2 0 0 0-2-2a2 2 0 0 0-2 2"/><path d="M14 10V4a2 2 0 0 0-2-2a2 2 0 0 0-2 2v2"/><path d="M10 10.5V6a2 2 0 0 0-2-2a2 2 0 0 0-2 2v8"/><path d="M18 8a2 2 0 1 1 4 0v6a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15"/>"#
      ),
      Self::PhoneOff => lucide!(
        r#"<path d="M10.1 13.9a14 14 0 0 0 3.732 2.668 1 1 0 0 0 1.213-.303l.355-.465A2 2 0 0 1 17 15h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2 18 18 0 0 1-12.728-5.272"/><path d="M22 2 2 22"/><path d="M4.76 13.582A18 18 0 0 1 2 4a2 2 0 0 1 2-2h3a2 2 0 0 1 2 2v3a2 2 0 0 1-.8 1.6l-.468.351a1 1 0 0 0-.292 1.233 14 14 0 0 0 .244.473"/>"#
      ),
      Self::Monitor => lucide!(
        r#"<rect width="20" height="14" x="2" y="3" rx="2"/><line x1="8" x2="16" y1="21" y2="21"/><line x1="12" x2="12" y1="17" y2="21"/>"#
      ),
      Self::Shield => lucide!(
        r#"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/>"#
      ),
      Self::Bot => lucide!(
        r#"<path d="M12 8V4H8"/><rect width="16" height="12" x="4" y="8" rx="2"/><path d="M2 14h2"/><path d="M20 14h2"/><path d="M15 13v2"/><path d="M9 13v2"/>"#
      ),
      Self::Check => lucide!(r#"<path d="M20 6 9 17l-5-5"/>"#),
      Self::SmilePlus => lucide!(
        r#"<path d="M22 11v1a10 10 0 1 1-9-10"/><path d="M8 14s1.5 2 4 2 4-2 4-2"/><line x1="9" x2="9.01" y1="9" y2="9"/><line x1="15" x2="15.01" y1="9" y2="9"/><path d="M16 5h6"/><path d="M19 2v6"/>"#
      ),
      Self::Pencil => lucide!(
        r#"<path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"/><path d="m15 5 4 4"/>"#
      ),
      Self::Reply => lucide!(r#"<path d="M20 18v-2a4 4 0 0 0-4-4H4"/><path d="m9 17-5-5 5-5"/>"#),
      Self::Thread => lucide!(
        r#"<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/><path d="M13 8H7"/><path d="M17 12H7"/>"#
      ),
      Self::More => {
        lucide!(r#"<circle cx="12" cy="12" r="1"/><circle cx="12" cy="5" r="1"/><circle cx="12" cy="19" r="1"/>"#)
      },
      Self::Plus => lucide!(r#"<path d="M5 12h14"/><path d="M12 5v14"/>"#),
      Self::AtSign => lucide!(r#"<circle cx="12" cy="12" r="4"/><path d="M16 8v5a3 3 0 0 0 6 0v-1a10 10 0 1 0-4 8"/>"#),
      Self::Star => lucide!(
        r#"<path d="M11.525 2.295a.53.53 0 0 1 .95 0l2.31 4.679a2.123 2.123 0 0 0 1.595 1.16l5.166.756a.53.53 0 0 1 .294.904l-3.736 3.638a2.123 2.123 0 0 0-.611 1.878l.882 5.14a.53.53 0 0 1-.771.56l-4.618-2.428a2.122 2.122 0 0 0-1.973 0L6.396 21.01a.53.53 0 0 1-.77-.56l.881-5.139a2.122 2.122 0 0 0-.611-1.879L2.16 9.795a.53.53 0 0 1 .294-.906l5.165-.755a2.122 2.122 0 0 0 1.597-1.16z"/>"#
      ),
      Self::Sparkles => lucide!(
        r#"<path d="M11.017 2.814a1 1 0 0 1 1.966 0l1.051 5.558a2 2 0 0 0 1.594 1.594l5.558 1.051a1 1 0 0 1 0 1.966l-5.558 1.051a2 2 0 0 0-1.594 1.594l-1.051 5.558a1 1 0 0 1-1.966 0l-1.051-5.558a2 2 0 0 0-1.594-1.594l-5.558-1.051a1 1 0 0 1 0-1.966l5.558-1.051a2 2 0 0 0 1.594-1.594z"/><path d="M20 2v4"/><path d="M22 4h-4"/><circle cx="4" cy="20" r="2"/>"#
      ),
      Self::Activity => lucide!(
        r#"<path d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2"/>"#
      ),
      Self::Bell => lucide!(
        r#"<path d="M10.268 21a2 2 0 0 0 3.464 0"/><path d="M3.262 15.326A1 1 0 0 0 4 17h16a1 1 0 0 0 .74-1.673C19.41 13.956 18 12.499 18 8A6 6 0 0 0 6 8c0 4.499-1.411 5.956-2.738 7.326"/>"#
      ),
      Self::ArrowDownAZ => lucide!(
        r#"<path d="m3 16 4 4 4-4"/><path d="M7 20V4"/><path d="M20 8h-5"/><path d="M15 10V6.5a2.5 2.5 0 0 1 5 0V10"/><path d="M15 14h5l-5 6h5"/>"#
      ),
      Self::History => lucide!(
        r#"<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M12 7v5l4 2"/>"#
      ),
      Self::User => lucide!(r#"<path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>"#),
      Self::PanelLeft => lucide!(r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M9 3v18"/>"#),
      Self::PanelLeftDashed => lucide!(
        r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M9 14v1"/><path d="M9 19v2"/><path d="M9 3v2"/><path d="M9 9v1"/>"#
      ),
      Self::PanelRight => lucide!(r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M15 3v18"/>"#),
      Self::PanelRightDashed => lucide!(
        r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M15 14v1"/><path d="M15 19v2"/><path d="M15 3v2"/><path d="M15 9v1"/>"#
      ),
      Self::Calendar => lucide!(
        r#"<path d="M8 2v4"/><path d="M16 2v4"/><rect width="18" height="18" x="3" y="4" rx="2"/><path d="M3 10h18"/>"#
      ),
      Self::ChevronLeft => lucide!(r#"<path d="m15 18-6-6 6-6"/>"#),
      Self::Clock => lucide!(r#"<circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/>"#),
      Self::MapPin => lucide!(
        r#"<path d="M20 10c0 4.993-5.539 10.193-7.399 11.799a1 1 0 0 1-1.202 0C9.539 20.193 4 14.993 4 10a8 8 0 0 1 16 0"/><circle cx="12" cy="10" r="3"/>"#
      ),
      Self::Text => lucide!(r#"<path d="M15 12H3"/><path d="M17 18H3"/><path d="M21 6H3"/>"#),
      Self::Repeat => lucide!(
        r#"<path d="m17 2 4 4-4 4"/><path d="M3 11v-1a4 4 0 0 1 4-4h14"/><path d="m7 22-4-4 4-4"/><path d="M21 13v1a4 4 0 0 1-4 4H3"/>"#
      ),
      Self::Trash => lucide!(
        r#"<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/><line x1="10" x2="10" y1="11" y2="17"/><line x1="14" x2="14" y1="11" y2="17"/>"#
      ),
      Self::List => lucide!(
        r#"<path d="M3 12h.01"/><path d="M3 18h.01"/><path d="M3 6h.01"/><path d="M8 12h13"/><path d="M8 18h13"/><path d="M8 6h13"/>"#
      ),
      Self::Users => lucide!(
        r#"<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><path d="M16 3.128a4 4 0 0 1 0 7.744"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><circle cx="9" cy="7" r="4"/>"#
      ),
      Self::Link => lucide!(
        r#"<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>"#
      ),
    }
  }
}

pub fn icon(name: Icon, size: f32, color: Tone) -> Svg {
  svg().data(name.svg()).size(px(size)).flex_shrink_0().text_color(rgb(color))
}

/// An on/off switch `height` tall: a white knob on the accent when on, on a see-through track when off.
pub fn switch(on: bool, height: f32) -> Div {
  let inset = height / 8.;
  div()
    .w(px(height * 5. / 3.))
    .h(px(height))
    .px(px(inset))
    .flex()
    .flex_shrink_0()
    .items_center()
    .when(on, gpui_kit::Styled::justify_end)
    .rounded_full()
    .bg(if on { rgb(ACCENT) } else { switch_off() })
    .child(div().size(px(height * 0.75)).rounded_full().bg(rgb(WHITE)))
}

/// Round avatar: the emoji when there is one, else the image when it is ready, else initials on a colored circle.
pub fn avatar(images: &Images, cx: &App, url: Option<&str>, emoji: Option<&str>, name: &str, size: f32) -> AnyElement {
  let circle = div()
    .size(px(size))
    .flex_shrink_0()
    .rounded_full()
    .flex()
    .items_center()
    .justify_center();
  if let Some(emoji) = emoji.filter(|emoji| !emoji.is_empty()) {
    return circle
      .bg(rgb(FIELD))
      .text_size(px(size * 0.55))
      .child(emoji.to_string())
      .into_any_element();
  }
  if let Some(url) = url {
    let sized = util::sized_avatar(url, util::avatar_px(size));
    if let Some(image) = images.get(&sized, 256, cx) {
      return img(image)
        .size(px(size))
        .flex_shrink_0()
        .rounded_full()
        .object_fit(ObjectFit::Cover)
        .into_any_element();
    }
  }
  circle
    .bg(util::name_color(name))
    .text_size(px(size * 0.42))
    .text_color(rgb(TEXT))
    .font_weight(FontWeight::MEDIUM)
    .child(util::initials(name))
    .into_any_element()
}

/// How the people of the one-to-one conversations show right now, by user id.
#[derive(Default, PartialEq, Eq)]
pub struct People(pub std::collections::HashMap<String, PersonStatus>);

impl Global for People {}

/// The mark at the corner of a person's avatar: green when active, a hollow ring when away, red with a bar
/// for do not disturb.
fn status_badge(status: PersonStatus, avatar: f32) -> Div {
  let size = (avatar * 0.42).max(9.);
  let badge = div()
    .absolute()
    .bottom(px(-2.))
    .right(px(-2.))
    .size(px(size))
    .rounded_full()
    .border_2()
    .border_color(rgb(BG));
  match status {
    PersonStatus::Active => badge.bg(rgb(GREEN)),
    PersonStatus::Away => badge
      .bg(rgb(BG))
      .child(div().size_full().rounded_full().border(px(1.5)).border_color(rgb(MUTED))),
    PersonStatus::DoNotDisturb => badge
      .bg(rgb(RED))
      .flex()
      .items_center()
      .justify_center()
      .child(div().w(px(size * 0.4)).h(px(1.5)).rounded_full().bg(rgb(WHITE))),
  }
}

pub fn group_avatar(images: &Images, cx: &App, group: &Group, size: f32) -> AnyElement {
  let plain = group.emoji.is_some() || group.avatar_url.is_some() || group.member_avatars.len() < 2;
  if plain {
    let url = group.avatar_url.as_deref().or(group.member_avatars.first().map(String::as_str));
    let picture = avatar(images, cx, url, group.emoji.as_deref(), &group.name, size);
    // A one-to-one conversation shows how the other person is.
    let person = (group.kind == GroupKind::Dm).then(|| group.member_ids.first()).flatten();
    let status = person.and_then(|id| cx.try_global::<People>()?.0.get(id).copied());
    return match status {
      Some(status) => div()
        .size(px(size))
        .flex_shrink_0()
        .relative()
        .child(picture)
        .child(status_badge(status, size))
        .into_any_element(),
      None => picture,
    };
  }
  // Group DM: two participants, overlapping on the diagonal like the web app.
  let small = size * 0.68;
  div()
    .size(px(size))
    .flex_shrink_0()
    .relative()
    .child(div().absolute().top_0().left_0().child(avatar(
      images,
      cx,
      Some(&group.member_avatars[0]),
      None,
      &group.name,
      small,
    )))
    .child(div().absolute().bottom_0().right_0().child(avatar(
      images,
      cx,
      Some(&group.member_avatars[1]),
      None,
      &group.name,
      small,
    )))
    .into_any_element()
}
