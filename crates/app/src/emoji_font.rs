//! The font emoji are drawn with. On Linux GPUI draws text with cosmic-text and swash, which cannot draw `COLRv1` color
//! fonts, Fedora's only color emoji font: the app bundles Noto Color Emoji (CBDT bitmaps, which swash draws) as the
//! family `Working Emoji` and names that family on emoji itself (`docs/workarounds.md`). Elsewhere every function
//! here leaves the text as it is, and the system's text stack picks the emoji font.

use std::ops::Range;

use gpui_kit::*;

/// The bundled emoji font's family: Noto Color Emoji renamed by `scripts/emoji-font.sh`, so an installed font cannot
/// shadow it, with a glyph for `m`, without which GPUI does not load a family by name.
#[cfg(target_os = "linux")]
const FAMILY: &str = "Working Emoji";

#[cfg(target_os = "linux")]
static FONT: &[u8] = include_bytes!("../assets/fonts/WorkingEmoji.ttf");

/// Adds the bundled emoji font to the text system, before the first window lays out text.
#[cfg(target_os = "linux")]
pub fn register(cx: &App) {
  if let Err(error) = cx.text_system().add_fonts(vec![std::borrow::Cow::Borrowed(FONT)]) {
    tracing::warn!("the emoji font did not load: {error:#}");
  }
}

/// Whether `family` is the bundled emoji font, which the font settings do not offer.
#[cfg(target_os = "linux")]
pub fn is_bundled(family: &str) -> bool {
  family == FAMILY
}

/// Whether `family` is the bundled emoji font, which the font settings do not offer.
#[cfg(not(target_os = "linux"))]
pub const fn is_bundled(_family: &str) -> bool {
  false
}

/// `text` as an element whose emoji are drawn with the emoji font: avatars, reactions, and other short labels.
#[cfg(target_os = "linux")]
pub fn text(text: impl Into<SharedString>) -> StyledText {
  let text = text.into();
  let (highlights, fonts) = runs(&text, Vec::new(), Vec::new());
  StyledText::new(text)
    .with_highlights(highlights)
    .with_font_family_overrides(fonts)
}

/// `text` as an element whose emoji are drawn with the emoji font: avatars, reactions, and other short labels.
#[cfg(not(target_os = "linux"))]
pub fn text(text: impl Into<SharedString>) -> SharedString {
  text.into()
}

/// Gives the text under `element` the emoji font as a fallback, for text this app does not lay out itself (inputs,
/// the composer): a character the font lacks is drawn with the emoji font when it has it. The interface font's own
/// glyph still wins, so a heart with the emoji selector (❤️) draws as the font's black heart there.
#[cfg(target_os = "linux")]
pub fn fallback<E: Styled>(mut element: E) -> E {
  element.text_style().font_fallbacks = Some(FontFallbacks::from_fonts(vec![FAMILY.to_string()]));
  element
}

/// Gives the text under `element` the emoji font as a fallback, for text this app does not lay out itself.
#[cfg(not(target_os = "linux"))]
pub const fn fallback<E: Styled>(element: E) -> E {
  element
}

/// Text runs that draw every emoji of `text` with the emoji font: `highlights` (sorted, not overlapping) split so
/// each emoji is a run of its own, since GPUI changes the family of whole runs only, and `fonts` (the same) with the
/// emoji family over the emoji.
#[cfg(target_os = "linux")]
pub fn runs(
  text: &str,
  highlights: Vec<(Range<usize>, HighlightStyle)>,
  fonts: Vec<(Range<usize>, SharedString)>,
) -> (Vec<(Range<usize>, HighlightStyle)>, Vec<(Range<usize>, SharedString)>) {
  let emoji = ranges(text);
  if emoji.is_empty() {
    return (highlights, fonts);
  }
  let family = SharedString::new_static(FAMILY);
  (split(&highlights, &emoji), overlay(&fonts, &emoji, &family))
}

/// Text runs that draw every emoji of `text` with the emoji font; elsewhere than Linux, `highlights` and `fonts`.
#[cfg(not(target_os = "linux"))]
pub const fn runs(
  _text: &str,
  highlights: Vec<(Range<usize>, HighlightStyle)>,
  fonts: Vec<(Range<usize>, SharedString)>,
) -> (Vec<(Range<usize>, HighlightStyle)>, Vec<(Range<usize>, SharedString)>) {
  (highlights, fonts)
}

/// The byte ranges of the emoji in `text`, one per grapheme: a grapheme with the emoji selector (U+FE0F), or one
/// Unicode draws as an emoji without it. A character that is text unless selected (❤, ©, a digit) stays text.
#[cfg(target_os = "linux")]
fn ranges(text: &str) -> Vec<Range<usize>> {
  use unicode_segmentation::UnicodeSegmentation as _;
  text
    .grapheme_indices(true)
    .filter(|(_, grapheme)| {
      !grapheme.is_ascii()
        && (grapheme.contains('\u{FE0F}')
          || emojis::get(grapheme).is_some_and(|emoji| !emoji.as_str().contains('\u{FE0F}')))
    })
    .map(|(start, grapheme)| start..start + grapheme.len())
    .collect()
}

/// Every boundary of `a` and `b`, sorted, as consecutive pairs.
#[cfg(target_os = "linux")]
fn pieces<T, U>(a: &[(Range<usize>, T)], b: &[(Range<usize>, U)]) -> Vec<Range<usize>> {
  let mut bounds: Vec<usize> = a
    .iter()
    .map(|(range, _)| range)
    .chain(b.iter().map(|(range, _)| range))
    .flat_map(|range| [range.start, range.end])
    .collect();
  bounds.sort_unstable();
  bounds.dedup();
  bounds.windows(2).map(|pair| pair[0]..pair[1]).collect()
}

/// The entry of `ranges` (sorted, not overlapping) that holds `piece`, which crosses none of their boundaries.
#[cfg(target_os = "linux")]
fn holder<'a, T>(ranges: &'a [(Range<usize>, T)], piece: &Range<usize>) -> Option<&'a T> {
  let ix = ranges.partition_point(|(range, _)| range.end <= piece.start);
  ranges
    .get(ix)
    .filter(|(range, _)| range.start <= piece.start && piece.end <= range.end)
    .map(|(_, value)| value)
}

/// `highlights` cut at every boundary of `emoji`, plus a plain highlight over each part of an emoji no highlight
/// covers.
#[cfg(target_os = "linux")]
fn split(highlights: &[(Range<usize>, HighlightStyle)], emoji: &[Range<usize>]) -> Vec<(Range<usize>, HighlightStyle)> {
  let emoji: Vec<(Range<usize>, ())> = emoji.iter().map(|range| (range.clone(), ())).collect();
  pieces(highlights, &emoji)
    .into_iter()
    .filter_map(|piece| match holder(highlights, &piece) {
      Some(style) => Some((piece, *style)),
      None => holder(&emoji, &piece).map(|()| (piece, HighlightStyle::default())),
    })
    .collect()
}

/// `fonts` with `family` over every range of `emoji`, adjacent ranges of one family joined.
#[cfg(target_os = "linux")]
fn overlay(
  fonts: &[(Range<usize>, SharedString)],
  emoji: &[Range<usize>],
  family: &SharedString,
) -> Vec<(Range<usize>, SharedString)> {
  let emoji: Vec<(Range<usize>, ())> = emoji.iter().map(|range| (range.clone(), ())).collect();
  let mut out: Vec<(Range<usize>, SharedString)> = Vec::new();
  for piece in pieces(fonts, &emoji) {
    let font = if holder(&emoji, &piece).is_some() {
      family
    } else if let Some(font) = holder(fonts, &piece) {
      font
    } else {
      continue;
    };
    let joins = out.last().map(|(last, last_font)| (last.end, last_font)) == Some((piece.start, font));
    match out.last_mut() {
      Some((last, _)) if joins => last.end = piece.end,
      _ => out.push((piece, font.clone())),
    }
  }
  out
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
  use gpui_kit::{FontWeight, HighlightStyle, SharedString};

  use super::{FAMILY, overlay, ranges, runs, split};

  #[test]
  fn emoji_are_whole_graphemes_and_text_hearts_stay_text() {
    let text = "a😀 ❤️ ❤ 👍🏽 👩‍💻 🇧🇷 1️⃣ © ☕ ok";
    let found: Vec<&str> = ranges(text).into_iter().map(|range| &text[range]).collect();
    assert_eq!(found, ["😀", "❤️", "👍🏽", "👩‍💻", "🇧🇷", "1️⃣", "☕"]);
    assert_eq!(ranges("Jordan Park, café"), Vec::<std::ops::Range<usize>>::new());
  }

  #[test]
  fn highlights_split_at_emoji_and_plain_runs_cover_the_rest() {
    let bold = HighlightStyle {
      font_weight: Some(FontWeight::BOLD),
      ..HighlightStyle::default()
    };
    // "hi 😀 there 🚀": bold over "hi 😀 t", the rocket outside any highlight.
    let highlights = vec![(0..8, bold)];
    let emoji = vec![3..7, 14..18];
    assert_eq!(
      split(&highlights, &emoji),
      vec![(0..3, bold), (3..7, bold), (7..8, bold), (14..18, HighlightStyle::default())]
    );
  }

  #[test]
  fn emoji_family_replaces_the_code_family_over_emoji() {
    let (mono, emoji_family) = (SharedString::from("Mono"), SharedString::from(FAMILY));
    let fonts = vec![(0..10, mono.clone())];
    assert_eq!(
      overlay(&fonts, &[4..8, 8..12, 20..24], &emoji_family),
      vec![(0..4, mono), (4..12, emoji_family.clone()), (20..24, emoji_family)]
    );
  }

  #[test]
  fn text_without_emoji_keeps_its_runs() {
    let fonts = vec![(0..4, SharedString::from("Mono"))];
    let (highlights, kept) = runs("code", Vec::new(), fonts.clone());
    assert_eq!(highlights, vec![]);
    assert_eq!(kept, fonts);
  }
}
