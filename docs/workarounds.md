# Workarounds

App code that makes up for something a dependency does not do yet. Each entry stays only until the library can do it; when it can, delete the workaround and its entry.

An entry lists what the workaround does, why the feature is needed, where the code lives, and the upstream change that would remove it.

## Padding around mention highlights

- **What:** a narrow no-break space (U+202F) goes inside both ends of every mention's highlight. A regular space goes outside a mention when a letter or digit touches it ("up@Freddy") and between two mentions that touch. Every highlight, font, and link range is shifted to match.
- **Why:** GPUI paints a text run's background as a box exactly as wide as its glyphs, with no padding and no corner radius. It also joins touching runs that share a background color, so "@Ana@Bia" rendered as one block. Mentions have to read as separate names.
- **Where:** `crates/app/src/message.rs`, `MENTION_PAD`, `mention_marks`, `pad_at`, `shift_start`, `shift_end`, called from `text_segment`.
- **Remove when:** GPUI's `HighlightStyle` (or the text run decoration) supports horizontal padding for backgrounds and stops joining separate runs. Rounded corners need a corner radius on the same background; nothing in the app does that today.
