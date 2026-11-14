//! Google Chat's own text markup, read when a message is sent.
//!
//! The composer cannot show styled text, so a person types the markers and the core turns them into formatting
//! here: `*bold*`, `_italic_`, `~strike~`, `` `code` ``, three backticks around a code block, and `- ` or `* ` at the
//! start of a line for a bullet. A pair of markers never reaches over a line break, and nothing inside code is read.
//! The app sends the text exactly as typed; this is the one place that reads the markers.

use std::ops::Range;

use crate::model::{Chip, Span, SpanKind};

/// A typed message with its markers read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
  /// The text without the markers.
  pub text: String,
  /// The chips of the typed text, their ranges moved to `text`. A chip whose range the typed text cannot hold is
  /// left out.
  pub chips: Vec<Chip>,
  /// The formatting over `text`, by start and then end: bold, italic, strike, code, code blocks, and list items.
  /// A code block covers its code and a list item its line, without the line break, as on a message that was
  /// received.
  pub spans: Vec<Span>,
}

/// Reads the markers in `text`, the message as typed, and leaves them out of the result.
///
/// `chips` hold byte ranges into `text`. A chip is never read for markers and never split: the markers around it
/// go, and its range follows the text.
pub fn parse(text: &str, chips: &[Chip]) -> Parsed {
  let chips: Vec<&Chip> = chips.iter().filter(|chip| text.get(chip.range.clone()).is_some()).collect();
  let mut scan = Scan::new(text, &chips);
  scan.code();
  scan.bullets();
  scan.emphasis();
  scan.finish(&chips)
}

/// One character of the typed text.
struct Cell {
  /// Byte offset in the typed text.
  byte: usize,
  ch: char,
  /// Part of a chip: it stays, and is never a marker.
  chip: bool,
  /// Inside code: never a marker.
  literal: bool,
  /// A marker the result leaves out.
  gone: bool,
}

impl Cell {
  /// Whether the cell can still be read: not in a chip, not inside code, not left out already.
  const fn free(&self) -> bool {
    !(self.chip || self.literal || self.gone)
  }

  /// Whether the cell is a `*`, `_` or `~` that can still be read.
  const fn marker(&self) -> bool {
    self.free() && matches!(self.ch, '*' | '_' | '~')
  }

  /// What a `*`, `_` or `~` that can still be read stands for.
  const fn emphasis(&self) -> Option<SpanKind> {
    if !self.free() {
      return None;
    }
    match self.ch {
      '*' => Some(SpanKind::Bold),
      '_' => Some(SpanKind::Italic),
      '~' => Some(SpanKind::Strike),
      _ => None,
    }
  }
}

/// Formatting over a range of cells.
struct Mark {
  kind: SpanKind,
  cells: Range<usize>,
}

/// The typed text, a cell per character, while its markers are found.
struct Scan {
  cells: Vec<Cell>,
  /// The formatting found, in the order it was found.
  marks: Vec<Mark>,
}

impl Scan {
  fn new(text: &str, chips: &[&Chip]) -> Self {
    let mut cells: Vec<Cell> = text
      .char_indices()
      .map(|(byte, ch)| Cell {
        byte,
        ch,
        chip: false,
        literal: false,
        gone: false,
      })
      .collect();
    for chip in chips {
      let first = cells.partition_point(|cell| cell.byte < chip.range.start);
      let last = cells.partition_point(|cell| cell.byte < chip.range.end);
      for cell in cells.iter_mut().take(last).skip(first) {
        cell.chip = true;
      }
    }
    Self { cells, marks: Vec::new() }
  }

  /// Whether the cell is a backtick that is not in a chip.
  fn tick(&self, index: usize) -> bool {
    self.cells.get(index).is_some_and(|cell| !cell.chip && cell.ch == '`')
  }

  /// Whether a fence starts at the cell: exactly three backticks, none in a chip. A longer run is only text.
  fn fence(&self, index: usize) -> bool {
    (index..index + 3).all(|at| self.tick(at))
      && !self.tick(index + 3)
      && index.checked_sub(1).is_none_or(|before| !self.tick(before))
  }

  /// Whether the cell is a line break that is not in a chip.
  fn line_break(&self, index: usize) -> bool {
    self.cells.get(index).is_some_and(|cell| !cell.chip && cell.ch == '\n')
  }

  /// The character before the cell.
  fn before(&self, index: usize) -> Option<char> {
    index
      .checked_sub(1)
      .and_then(|before| self.cells.get(before))
      .map(|cell| cell.ch)
  }

  /// The character after the cell.
  fn after(&self, index: usize) -> Option<char> {
    self.cells.get(index + 1).map(|cell| cell.ch)
  }

  /// Whether anything but white space stands in `cells`. A chip counts.
  fn has_text(&self, cells: Range<usize>) -> bool {
    self
      .cells
      .iter()
      .take(cells.end)
      .skip(cells.start)
      .any(|cell| cell.chip || !cell.ch.is_whitespace())
  }

  /// Leaves the cell out of the result, unless it is part of a chip.
  fn take(&mut self, index: usize) {
    if let Some(cell) = self.cells.get_mut(index).filter(|cell| !cell.chip) {
      cell.gone = true;
    }
  }

  /// Makes the cells code: no marker in them is read.
  fn literal(&mut self, cells: Range<usize>) {
    for cell in self.cells.iter_mut().take(cells.end).skip(cells.start) {
      cell.literal = true;
    }
  }

  /// Finds the code blocks and the inline code. What is inside them is text, and nothing else reads it.
  fn code(&mut self) {
    let mut at = 0;
    while at < self.cells.len() {
      at = if self.fence(at) {
        self.block(at)
      } else if self.tick(at) {
        self.inline(at)
      } else {
        at + 1
      };
    }
  }

  /// A code block opened by the fence at `open`: three backticks, the code, three backticks. Answers the cell to
  /// go on from. A fence that closes nothing, or around nothing but white space, is text.
  fn block(&mut self, open: usize) -> usize {
    let start = open + 3;
    let Some(close) = (start..self.cells.len()).find(|&index| self.fence(index)) else {
      return start;
    };
    // The line breaks that set the fences apart from the code are not part of it.
    let first = if self.line_break(start) {
      start + 1
    } else {
      start
    };
    let last = if close > first && self.line_break(close - 1) {
      close - 1
    } else {
      close
    };
    if !self.has_text(first..last) {
      return start;
    }
    for index in (open..first).chain(last..close + 3) {
      self.take(index);
    }
    self.literal(first..last);
    self.marks.push(Mark {
      kind: SpanKind::CodeBlock,
      cells: first..last,
    });
    close + 3
  }

  /// Inline code opened by the backtick at `open`. Answers the cell to go on from. The first backtick on the line
  /// decides: it closes the code, or the opening one is only a character.
  fn inline(&mut self, open: usize) -> usize {
    let first = open + 1;
    if !self.starts_code(open) {
      return first;
    }
    let close = (first..self.cells.len()).find(|&index| self.tick(index) || self.line_break(index));
    let Some(close) = close.filter(|&close| close > first && self.tick(close) && self.ends_code(close)) else {
      return first;
    };
    self.take(open);
    self.take(close);
    self.literal(first..close);
    self.marks.push(Mark {
      kind: SpanKind::Code,
      cells: first..close,
    });
    close + 1
  }

  /// Whether the backtick may open inline code: it starts a word, and a character that is not white space or
  /// another backtick follows.
  fn starts_code(&self, index: usize) -> bool {
    self
      .before(index)
      .is_none_or(|before| opens_after(before) || matches!(before, '*' | '_' | '~'))
      && self.after(index).is_some_and(|after| !after.is_whitespace() && after != '`')
  }

  /// Whether the backtick may close inline code: it follows a character that is not white space or another
  /// backtick, and ends a word.
  fn ends_code(&self, index: usize) -> bool {
    self
      .before(index)
      .is_some_and(|before| !before.is_whitespace() && before != '`')
      && self.after(index).is_none_or(|after| closes_before(after) && after != '`')
  }

  /// Turns each line that starts with `- ` or `* ` and goes on into a list item: the bullet and its space go.
  fn bullets(&mut self) {
    let mut start = 0;
    while start < self.cells.len() {
      let end = (start..self.cells.len())
        .find(|&index| self.line_break(index))
        .unwrap_or(self.cells.len());
      let bullet = self
        .cells
        .get(start)
        .is_some_and(|cell| cell.free() && matches!(cell.ch, '-' | '*'))
        && self.cells.get(start + 1).is_some_and(|cell| cell.free() && cell.ch == ' ')
        && self.has_text(start + 2..end);
      if bullet {
        self.take(start);
        self.take(start + 1);
        self.marks.push(Mark {
          kind: SpanKind::ListItem,
          cells: start + 2..end,
        });
      }
      start = end + 1;
    }
  }

  /// Finds the pairs of `*`, `_` and `~`, and leaves their markers out.
  fn emphasis(&mut self) {
    for mark in self.pairs() {
      self.take(mark.cells.start.saturating_sub(1));
      self.take(mark.cells.end);
      self.marks.push(mark);
    }
  }

  /// The formatting between pairs of markers. A closing marker pairs with the nearest opening one of its kind
  /// before it on the line; the openers in between stay as typed.
  fn pairs(&self) -> Vec<Mark> {
    let mut open: Vec<(char, usize)> = Vec::new();
    let mut pairs = Vec::new();
    for ((index, cell), (can_open, can_close)) in self.cells.iter().enumerate().zip(self.reach()) {
      if cell.free() && cell.ch == '\n' {
        open.clear();
        continue;
      }
      let Some(kind) = cell.emphasis() else {
        continue;
      };
      // A pair needs something between its markers.
      let partner = open
        .iter()
        .rposition(|(marker, _)| *marker == cell.ch)
        .and_then(|at| open.get(at).map(|(_, start)| (at, *start)))
        .filter(|(_, start)| can_close && start + 1 < index);
      if let Some((at, start)) = partner {
        open.truncate(at);
        pairs.push(Mark { kind, cells: start + 1..index });
      } else if can_open {
        open.push((cell.ch, index));
      }
    }
    pairs
  }

  /// For each cell, whether a marker there may open a pair and whether it may close one. Markers that touch are
  /// judged together, by what stands around them: they open when they start a word with text after them, and
  /// close when text comes before them and they end a word.
  fn reach(&self) -> Vec<(bool, bool)> {
    let before = self.cells.iter().scan((true, false), |state, cell| {
      let seen = *state;
      if !cell.marker() {
        *state = (opens_after(cell.ch), !cell.ch.is_whitespace());
      }
      Some(seen)
    });
    let mut after: Vec<(bool, bool)> = self
      .cells
      .iter()
      .rev()
      .scan((true, false), |state, cell| {
        let seen = *state;
        if !cell.marker() {
          *state = (closes_before(cell.ch), !cell.ch.is_whitespace());
        }
        Some(seen)
      })
      .collect();
    after.reverse();
    before
      .zip(after)
      .map(|((starts, text_before), (ends, text_after))| (starts && text_after, text_before && ends))
      .collect()
  }

  /// The result: the text without the cells that were left out, with chips and formatting at their new places.
  fn finish(self, chips: &[&Chip]) -> Parsed {
    let mut text = String::with_capacity(self.cells.len());
    let mut offsets = Vec::with_capacity(self.cells.len() + 1);
    for cell in &self.cells {
      offsets.push(text.len());
      if !cell.gone {
        text.push(cell.ch);
      }
    }
    offsets.push(text.len());
    // Where the cell starts in the result; past the last one, where the result ends.
    let place = |index: usize| offsets.get(index).copied().unwrap_or(text.len());
    let moved = |offset: usize| place(self.cells.partition_point(|cell| cell.byte < offset));
    let chips = chips
      .iter()
      .map(|chip| Chip {
        range: moved(chip.range.start)..moved(chip.range.end),
        kind: chip.kind.clone(),
      })
      .collect();
    let mut spans: Vec<Span> = self
      .marks
      .into_iter()
      .map(|mark| Span {
        range: place(mark.cells.start)..place(mark.cells.end),
        kind: mark.kind,
      })
      .filter(|span| !span.range.is_empty())
      .collect();
    spans.sort_by_key(|span| (span.range.start, span.range.end, rank(&span.kind)));
    spans.dedup();
    Parsed { text, chips, spans }
  }
}

/// Whether a marker straight after `c` starts a word: after white space, an opening bracket or quote, or a symbol
/// such as an emoji.
fn opens_after(c: char) -> bool {
  c.is_whitespace() || matches!(c, '(' | '[' | '{' | '<' | '"' | '\'') || !(c.is_ascii() || c.is_alphanumeric())
}

/// Whether a marker straight before `c` ends a word: before anything that is not a letter or a digit.
fn closes_before(c: char) -> bool {
  !c.is_alphanumeric()
}

/// Where a kind goes among spans over the same text, so that equal spans sit together.
const fn rank(kind: &SpanKind) -> u8 {
  match kind {
    SpanKind::Bold => 0,
    SpanKind::Italic => 1,
    SpanKind::Strike => 2,
    SpanKind::Underline => 3,
    SpanKind::Code => 4,
    SpanKind::CodeBlock => 5,
    SpanKind::ListItem => 6,
    SpanKind::Link(_) | SpanKind::Mention(_) | SpanKind::MentionAll => 7,
  }
}

#[cfg(test)]
mod tests {
  use std::ops::Range;

  use super::{Parsed, parse};
  use crate::model::SpanKind::{Bold, Code, CodeBlock, Italic, ListItem, Strike};
  use crate::model::{Chip, ChipKind, SpanKind};

  fn covered<'a>(text: &'a str, range: &Range<usize>) -> &'a str {
    text.get(range.clone()).expect("a range inside the text, on char boundaries")
  }

  /// What each span covers, in the order `parse` gives them.
  fn marked(parsed: &Parsed) -> Vec<(&str, SpanKind)> {
    parsed
      .spans
      .iter()
      .map(|span| (covered(&parsed.text, &span.range), span.kind.clone()))
      .collect()
  }

  /// What each chip covers.
  fn held(parsed: &Parsed) -> Vec<&str> {
    parsed.chips.iter().map(|chip| covered(&parsed.text, &chip.range)).collect()
  }

  /// Reads `typed` without chips and checks the text and what the spans cover.
  #[track_caller]
  fn check(typed: &str, text: &str, spans: &[(&str, SpanKind)]) {
    let parsed = parse(typed, &[]);
    assert_eq!(parsed.text, text, "the text of {typed:?}");
    assert_eq!(marked(&parsed), spans, "the spans of {typed:?}");
  }

  /// A mention over the first `name` in `typed`.
  fn mention(typed: &str, name: &str) -> Chip {
    let start = typed.find(name).expect("the name is in the text");
    Chip {
      range: start..start + name.len(),
      kind: ChipKind::Mention { user_id: "riley".into(), bot: false },
    }
  }

  #[test]
  fn each_marker_formats_its_word() {
    check("*bold*", "bold", &[("bold", Bold)]);
    check("_italic_", "italic", &[("italic", Italic)]);
    check("~strike~", "strike", &[("strike", Strike)]);
    check("`code`", "code", &[("code", Code)]);
    check("say *two words* now", "say two words now", &[("two words", Bold)]);
    check("*hi* _there_", "hi there", &[("hi", Bold), ("there", Italic)]);
    check(
      "*a* and _b_ and ~c~ and `d`",
      "a and b and c and d",
      &[("a", Bold), ("b", Italic), ("c", Strike), ("d", Code)],
    );
  }

  #[test]
  fn text_without_markers_comes_back_as_it_was() {
    check("", "", &[]);
    check(
      "plain text, 100% plain.\nTwo lines.",
      "plain text, 100% plain.\nTwo lines.",
      &[],
    );
  }

  #[test]
  fn markers_nest() {
    check("*_both_*", "both", &[("both", Bold), ("both", Italic)]);
    check(
      "*bold _and italic_ text*",
      "bold and italic text",
      &[("bold and italic text", Bold), ("and italic", Italic)],
    );
    check("~*_all_*~", "all", &[("all", Bold), ("all", Italic), ("all", Strike)]);
    check("*`code`*", "code", &[("code", Bold), ("code", Code)]);
    check(
      "_see `code` here_",
      "see code here",
      &[("see code here", Italic), ("code", Code)],
    );
  }

  #[test]
  fn doubled_markers_format_once() {
    check("**bold**", "bold", &[("bold", Bold)]);
    check("__italic__", "italic", &[("italic", Italic)]);
    check("~~strike~~", "strike", &[("strike", Strike)]);
  }

  #[test]
  fn a_marker_inside_a_word_stays() {
    for typed in [
      "2*3*4",
      "snake_case_name",
      "a * b",
      "a*b*",
      "x_y_",
      "a~b~c",
      "*bold*s",
      "pre*fix*",
      "é_x_",
      "https://example.com/a_b_c",
      "~/a and ~/b",
      "*.rs and *.md",
      "a`b`c",
    ] {
      check(typed, typed, &[]);
    }
  }

  #[test]
  fn punctuation_and_quotes_may_touch_the_markers() {
    check("(*bold*)", "(bold)", &[("bold", Bold)]);
    check("\"_it_\"", "\"it\"", &[("it", Italic)]);
    check(
      "*bold*, then _it_. ~Gone~!",
      "bold, then it. Gone!",
      &[("bold", Bold), ("it", Italic), ("Gone", Strike)],
    );
    check("*bold*'s", "bold's", &[("bold", Bold)]);
    check("[`x`]", "[x]", &[("x", Code)]);
  }

  #[test]
  fn an_unmatched_marker_stays_as_typed() {
    for typed in [
      "*oops", "oops*", "_a", "a_", "~", "`", "`open", "close`", "*a _b", "* ", "_ x _",
    ] {
      check(typed, typed, &[]);
    }
    // The inner marker of two that cross stays; the pair that closes first wins.
    check("*a _b* c_", "a _b c_", &[("a _b", Bold)]);
  }

  #[test]
  fn nothing_between_the_markers_stays_as_typed() {
    for typed in ["**", "__", "~~", "``", "(**)", "` `"] {
      check(typed, typed, &[]);
    }
  }

  #[test]
  fn a_pair_never_reaches_over_a_line_break() {
    check("*one\ntwo*", "*one\ntwo*", &[]);
    check("`one\ntwo`", "`one\ntwo`", &[]);
    check("*one*\n*two*", "one\ntwo", &[("one", Bold), ("two", Bold)]);
    check("*a\n*b*", "*a\nb", &[("b", Bold)]);
  }

  #[test]
  fn nothing_inside_code_is_read() {
    check(
      "`*not bold*` and `a_b_c`",
      "*not bold* and a_b_c",
      &[("*not bold*", Code), ("a_b_c", Code)],
    );
    check("`- not a bullet`", "- not a bullet", &[("- not a bullet", Code)]);
    check("```*x* _y_ ~z~```", "*x* _y_ ~z~", &[("*x* _y_ ~z~", CodeBlock)]);
    check("`a` *b*", "a b", &[("a", Code), ("b", Bold)]);
  }

  #[test]
  fn inline_code_has_single_backticks_around_a_word_edge() {
    check("`a`b`", "`a`b`", &[]);
    check("`a``b`", "`a``b`", &[]);
    check("` a`", "` a`", &[]);
    check("`a `", "`a `", &[]);
    check("see (`a`), `b`.", "see (a), b.", &[("a", Code), ("b", Code)]);
  }

  #[test]
  fn a_code_block_may_span_lines() {
    check(
      "```\nfn main() {\n  run();\n}\n```",
      "fn main() {\n  run();\n}",
      &[("fn main() {\n  run();\n}", CodeBlock)],
    );
    check("```one line```", "one line", &[("one line", CodeBlock)]);
    check("before\n```\ncode\n```\nafter", "before\ncode\nafter", &[("code", CodeBlock)]);
    check("a ```x``` b", "a x b", &[("x", CodeBlock)]);
    check("```\n\ncode\n```", "\ncode", &[("\ncode", CodeBlock)]);
    check("```\n  indented\n```", "  indented", &[("  indented", CodeBlock)]);
  }

  #[test]
  fn a_fence_around_nothing_stays_as_typed() {
    for typed in ["```", "```x", "x```", "``` ```", "``````", "```\n\n```", "```code``"] {
      check(typed, typed, &[]);
    }
  }

  #[test]
  fn a_longer_run_of_backticks_is_only_text() {
    for typed in ["````\ncode\n````", "```a````", "````a```", "`````"] {
      check(typed, typed, &[]);
    }
  }

  #[test]
  fn code_block_lines_are_not_bullets_or_marks() {
    check("```\n- a\n* b\n_c_\n```", "- a\n* b\n_c_", &[("- a\n* b\n_c_", CodeBlock)]);
    check(
      "- x\n```\n- y\n```\n- z",
      "x\n- y\nz",
      &[("x", ListItem), ("- y", CodeBlock), ("z", ListItem)],
    );
  }

  #[test]
  fn bullets_become_list_items() {
    check("- one\n- two", "one\ntwo", &[("one", ListItem), ("two", ListItem)]);
    check("* one\n* two", "one\ntwo", &[("one", ListItem), ("two", ListItem)]);
    check(
      "List:\n- a\n- b\nafter",
      "List:\na\nb\nafter",
      &[("a", ListItem), ("b", ListItem)],
    );
    check("- one\n\n- two", "one\n\ntwo", &[("one", ListItem), ("two", ListItem)]);
    check("- *bold* item", "bold item", &[("bold", Bold), ("bold item", ListItem)]);
    check(
      "* _a_\n* ~b~",
      "a\nb",
      &[("a", Italic), ("a", ListItem), ("b", Strike), ("b", ListItem)],
    );
    check("-  two spaces", " two spaces", &[(" two spaces", ListItem)]);
    check("* *", "*", &[("*", ListItem)]);
  }

  #[test]
  fn a_line_that_only_looks_like_a_bullet_stays() {
    for typed in ["-one", "*one", "- ", "-  ", "* \n- ", " - indented", "a - b", "--", "-", "---"] {
      check(typed, typed, &[]);
    }
  }

  #[test]
  fn non_ascii_text_keeps_its_boundaries() {
    check("café *olé* ñ", "café olé ñ", &[("olé", Bold)]);
    check("日本語 *太字* です", "日本語 太字 です", &[("太字", Bold)]);
    check("🎉*Release* notes", "🎉Release notes", &[("Release", Bold)]);
    check("*é*", "é", &[("é", Bold)]);
    check("_café_", "café", &[("café", Italic)]);
    check("- é\n- 🎉", "é\n🎉", &[("é", ListItem), ("🎉", ListItem)]);
    check("café_name_é", "café_name_é", &[]);
    check("```\ncafé\n```", "café", &[("café", CodeBlock)]);
  }

  #[test]
  fn chips_follow_the_text() {
    let typed = "@Riley Brooks hi *there* and @Jordan Park, _bye_";
    let chips = [mention(typed, "@Riley Brooks"), mention(typed, "@Jordan Park")];
    let parsed = parse(typed, &chips);
    assert_eq!(parsed.text, "@Riley Brooks hi there and @Jordan Park, bye");
    assert_eq!(held(&parsed), ["@Riley Brooks", "@Jordan Park"]);
    let (riley, jordan) = (&parsed.chips[0], &parsed.chips[1]);
    assert_eq!(riley.range, 0..13, "before the first marker");
    assert_eq!(
      jordan.range,
      chips[1].range.start - 2..chips[1].range.end - 2,
      "after two markers"
    );
    assert_eq!(riley.kind, chips[0].kind);
    assert_eq!(marked(&parsed), [("there", Bold), ("bye", Italic)]);
  }

  #[test]
  fn markers_around_a_chip_do_not_split_it() {
    let typed = "*@Riley Brooks* and ~hi @Jordan Park~ and _x_@Riley Brooks";
    let chips = [
      mention(typed, "@Riley Brooks"),
      mention(typed, "@Jordan Park"),
      Chip {
        range: typed.rfind("@Riley Brooks").expect("last")..typed.len(),
        kind: ChipKind::MentionAll,
      },
    ];
    let parsed = parse(typed, &chips);
    assert_eq!(parsed.text, "@Riley Brooks and hi @Jordan Park and x@Riley Brooks");
    assert_eq!(held(&parsed), ["@Riley Brooks", "@Jordan Park", "@Riley Brooks"]);
    assert_eq!(
      marked(&parsed),
      [("@Riley Brooks", Bold), ("hi @Jordan Park", Strike), ("x", Italic)]
    );
    assert_eq!(parsed.chips[2].kind, ChipKind::MentionAll);
  }

  #[test]
  fn a_chip_is_never_read_for_markers() {
    let typed = "hi _Riley_ Brooks there _x_";
    let chip = Chip {
      range: 3..typed.find(" there").expect("name end"),
      kind: ChipKind::MentionAll,
    };
    let parsed = parse(typed, std::slice::from_ref(&chip));
    assert_eq!(parsed.text, "hi _Riley_ Brooks there x");
    assert_eq!(held(&parsed), ["_Riley_ Brooks"]);
    assert_eq!(marked(&parsed), [("x", Italic)]);
    assert_eq!(parse("hi _Riley_ Brooks", &[]).spans.len(), 1, "without the chip it is italic");
  }

  #[test]
  fn chips_inside_code_and_list_items_stay_whole() {
    let typed = "- @Riley Brooks\n```\nask @Jordan Park\n```";
    let chips = [mention(typed, "@Riley Brooks"), mention(typed, "@Jordan Park")];
    let parsed = parse(typed, &chips);
    assert_eq!(parsed.text, "@Riley Brooks\nask @Jordan Park");
    assert_eq!(held(&parsed), ["@Riley Brooks", "@Jordan Park"]);
    assert_eq!(marked(&parsed), [("@Riley Brooks", ListItem), ("ask @Jordan Park", CodeBlock)]);
  }

  #[test]
  fn a_slash_command_keeps_its_place() {
    let typed = "/deploy *now*";
    let command = Chip {
      range: 0..7,
      kind: ChipKind::Command {
        bot_id: "build-bot".into(),
        command_id: 2,
        name: "/deploy".into(),
      },
    };
    let parsed = parse(typed, std::slice::from_ref(&command));
    assert_eq!(parsed.text, "/deploy now");
    assert_eq!(parsed.chips, [command]);
    assert_eq!(marked(&parsed), [("now", Bold)]);
  }

  #[test]
  fn a_chip_the_text_cannot_hold_is_left_out() {
    // Past the end, inside the é, backwards, and the one the text can hold.
    let backwards = Range { start: 3, end: 1 };
    let chips = [0..99, 1..2, backwards, 4..5].map(|range| Chip { range, kind: ChipKind::MentionAll });
    let parsed = parse("é *a*", &chips);
    assert_eq!(parsed.text, "é a");
    assert_eq!(parsed.chips.len(), 1);
    assert_eq!(held(&parsed), ["a"]);
  }

  /// Everything `parse` promises, for one typed text.
  fn assert_promises(typed: &str, chips: &[Chip]) {
    let parsed = parse(typed, chips);
    // Only markers, bullets and the line breaks around a code block go.
    let mut kept = parsed.text.chars().peekable();
    for c in typed.chars() {
      if kept.peek() == Some(&c) {
        kept.next();
      } else {
        assert!("*_~`- \n".contains(c), "{typed:?} lost {c:?}, left {:?}", parsed.text);
      }
    }
    assert!(
      kept.next().is_none(),
      "{typed:?} gave {:?}, which is not part of it",
      parsed.text
    );
    if !typed.contains(['*', '_', '~', '`', '-']) {
      assert_eq!(parsed.text, typed);
      assert!(parsed.spans.is_empty(), "{typed:?}");
    }
    // A message with something in it still has something in it, unless a chip is all white space.
    if chips.is_empty() && !typed.trim().is_empty() {
      assert!(!parsed.text.trim().is_empty(), "{typed:?} became nothing");
    }
    // Spans are inside the text, on char boundaries, and in order.
    for span in &parsed.spans {
      assert!(span.range.start < span.range.end, "{typed:?}: {span:?} is empty");
      assert!(
        parsed.text.get(span.range.clone()).is_some(),
        "{typed:?}: {span:?} is not in {:?}",
        parsed.text
      );
    }
    assert!(
      parsed
        .spans
        .windows(2)
        .all(|pair| (pair[0].range.start, pair[0].range.end) <= (pair[1].range.start, pair[1].range.end)),
      "{typed:?}: spans are out of order"
    );
    // Chips keep their text, and no span starts or ends inside one.
    assert_eq!(parsed.chips.len(), chips.len(), "{typed:?}");
    for (chip, moved) in chips.iter().zip(&parsed.chips) {
      assert_eq!(covered(&parsed.text, &moved.range), covered(typed, &chip.range), "{typed:?}");
      for span in &parsed.spans {
        for edge in [span.range.start, span.range.end] {
          assert!(
            edge <= moved.range.start || edge >= moved.range.end,
            "{typed:?}: {span:?} splits {moved:?}"
          );
        }
      }
    }
  }

  #[test]
  fn short_texts_keep_the_promises() {
    const ALPHABET: [char; 8] = ['*', '_', '~', '`', '-', ' ', '\n', 'é'];
    let mut texts = vec![String::new()];
    let mut every = Vec::new();
    for _ in 0..5 {
      texts = texts
        .iter()
        .flat_map(|text| ALPHABET.iter().map(move |c| format!("{text}{c}")))
        .collect();
      every.extend(texts.iter().cloned());
    }
    for typed in &every {
      assert_promises(typed, &[]);
      if typed.chars().count() > 4 {
        continue;
      }
      // Every range a chip could take.
      let edges: Vec<usize> = typed.char_indices().map(|(at, _)| at).chain([typed.len()]).collect();
      for (index, &start) in edges.iter().enumerate() {
        for &end in edges.iter().skip(index) {
          let chip = Chip {
            range: start..end,
            kind: ChipKind::MentionAll,
          };
          assert_promises(typed, &[chip]);
        }
      }
    }
  }
}
