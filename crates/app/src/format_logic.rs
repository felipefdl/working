//! What the composer's formatting buttons and keys do to a draft, as plain code. Each format takes the text and the
//! selection, as byte offsets, and answers with the edits to make and where the selection ends up. The markers are Google
//! Chat's own: `*bold*`, `_italic_`, `~strike~`, and `` `code` `` inline, `- ` before a bulleted line, and three
//! backticks on the line above and the line below a code block. The core turns them into real formatting when the
//! message is sent. No GPUI here.
//!
//! A button takes a format off what has it, and what has it is what the core reads: `Reading` is the core's own rule
//! for which markers pair up (`crates/core/src/markup.rs`), so text the core would send as literal markers
//! (`snake_case_name`, `see:*this*`) is never taken for formatted. A pair that goes in where the core would not read it
//! still goes in, as asked, and stays selected with its text: the next press finds it and takes it off.
//!
//! An edit is a few small changes, not one rewrite of the selection: the caller applies each as a range edit, so the
//! chips (mentions) between the markers stay in the text.

use std::ops::Range;

/// The line of three backticks that opens and closes a code block.
const FENCE: &str = "```";

/// The start of a bulleted line.
const BULLET: &str = "- ";

/// A format the composer applies to its selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
  Bold,
  Italic,
  Strikethrough,
  /// A bulleted list, one bullet for each line.
  Bullets,
  /// Inline code.
  Code,
  CodeBlock,
}

impl Format {
  /// The edit that applies this format to `selection` of `text`: it wraps what is not in the format yet, and takes the
  /// format off what is, as its button does.
  pub fn apply(self, text: &str, selection: Range<usize>) -> Edit {
    match self {
      Self::Bold => toggle_inline(text, selection, '*'),
      Self::Italic => toggle_inline(text, selection, '_'),
      Self::Strikethrough => toggle_inline(text, selection, '~'),
      Self::Code => toggle_inline(text, selection, '`'),
      Self::Bullets => toggle_bullets(text, selection),
      Self::CodeBlock => wrap_code_block(text, selection),
    }
  }
}

/// One replacement in the text as it was before the edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
  /// The bytes that go.
  pub range: Range<usize>,
  /// What takes their place: nothing for a removal, and `range` is empty for an insertion.
  pub text: String,
}

impl Change {
  fn insert(at: usize, text: impl Into<String>) -> Self {
    Self { range: at..at, text: text.into() }
  }

  const fn remove(range: Range<usize>) -> Self {
    Self { range, text: String::new() }
  }
}

/// What a format does to a draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
  /// Where they are in the text before the edit, from the first to the last, none overlapping another. Applied from
  /// the last to the first, each leaves the offsets of the ones before it as they were.
  pub changes: Vec<Change>,
  /// The selection after the edit, as offsets into the new text.
  pub selection: Range<usize>,
}

/// `text` with `changes` made.
fn applied(text: &str, changes: &[Change]) -> String {
  let mut text = text.to_string();
  for change in changes.iter().rev() {
    text.replace_range(change.range.clone(), &change.text);
  }
  text
}

/// `selection` held to the text: inside it, in order, and on character boundaries.
fn clamp(text: &str, selection: Range<usize>) -> Range<usize> {
  let boundary = |offset: usize| {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
      offset -= 1;
    }
    offset
  };
  let (start, end) = (boundary(selection.start), boundary(selection.end));
  start.min(end)..start.max(end)
}

/// Where `offset` of the old text is in the new one. Each change before it moves it by what the change adds or takes
/// away. An insertion exactly at `offset` is before it when `past` is true and after it otherwise, and a removal that
/// covers it puts it where the removal began.
fn moved(changes: &[Change], offset: usize, past: bool) -> usize {
  let mut grown = 0;
  let mut shrunk = 0;
  for change in changes {
    let range = &change.range;
    if range.start < offset && offset < range.end {
      return (range.start + grown).saturating_sub(shrunk);
    }
    let before = range.end < offset || (range.end == offset && (past || !range.is_empty()));
    if before {
      grown += change.text.len();
      shrunk += range.len();
    }
  }
  (offset + grown).saturating_sub(shrunk)
}

/// The start of the line `offset` is on.
fn line_start(text: &str, offset: usize) -> usize {
  text
    .get(..offset)
    .and_then(|before| before.rfind('\n'))
    .map_or(0, |newline| newline + 1)
}

/// The end of the line `offset` is on: where its line break is, or the end of the text.
fn line_end(text: &str, offset: usize) -> usize {
  text
    .get(offset..)
    .and_then(|after| after.find('\n'))
    .map_or(text.len(), |newline| offset + newline)
}

/// The lines the selection touches, each from its start to just before its line break. A selection that ends right
/// after a line break leaves the line it ends on alone.
fn touched_lines(text: &str, selection: &Range<usize>) -> Vec<Range<usize>> {
  let ends_after_break =
    selection.end > selection.start && text.get(..selection.end).is_some_and(|before| before.ends_with('\n'));
  let last = if ends_after_break {
    selection.end - 1
  } else {
    selection.end
  };
  let mut lines = Vec::new();
  let mut start = line_start(text, selection.start);
  loop {
    let end = line_end(text, start);
    lines.push(start..end);
    if end >= last {
      return lines;
    }
    start = end + 1;
  }
}

/// How many bytes a line's bullet takes: `- ` and `* ` both start a bulleted line. Zero when it has none.
fn bullet_width(line: &str) -> usize {
  if line.starts_with(BULLET) || line.starts_with("* ") {
    BULLET.len()
  } else {
    0
  }
}

// How the core reads the markers.

/// Whether a marker straight after `c` starts a word: after white space, an opening bracket or quote, or a symbol
/// such as an emoji.
fn opens_after(c: char) -> bool {
  c.is_whitespace() || matches!(c, '(' | '[' | '{' | '<' | '"' | '\'') || !(c.is_ascii() || c.is_alphanumeric())
}

/// Whether a marker straight before `c` ends a word: before anything that is not a letter or a digit.
fn closes_before(c: char) -> bool {
  !c.is_alphanumeric()
}

/// A character of the draft, as the core reads it.
struct Cell {
  /// Where the character starts, in bytes.
  byte: usize,
  ch: char,
  /// Inside code: no marker in it is read.
  literal: bool,
  /// A marker or a bullet that the core takes out of the text.
  gone: bool,
}

impl Cell {
  /// Whether the core can still read the cell: it is not inside code, and not taken out already.
  const fn free(&self) -> bool {
    !(self.literal || self.gone)
  }

  /// Whether the cell is a `*`, `_` or `~` that the core can still read.
  const fn marker(&self) -> bool {
    self.free() && matches!(self.ch, '*' | '_' | '~')
  }
}

/// A pair of markers that the core reads as formatting: where the two are, as byte offsets.
struct Pair {
  marker: char,
  open: usize,
  close: usize,
}

/// A draft as the core reads it: which markers it takes out, and which of them pair up to format text. This is the
/// reading of `crates/core/src/markup.rs` again, rule for rule, but for the chips, which the text alone does not show:
/// when one changes, so does the other.
struct Reading {
  cells: Vec<Cell>,
  pairs: Vec<Pair>,
}

impl Reading {
  fn of(text: &str) -> Self {
    let cells = text
      .char_indices()
      .map(|(byte, ch)| Cell { byte, ch, literal: false, gone: false })
      .collect();
    let mut reading = Self { cells, pairs: Vec::new() };
    reading.code();
    reading.bullets();
    reading.emphasis();
    reading
  }

  /// Whether the core reads the characters at the byte offsets `open` and `close` as the two `marker`s of a pair.
  fn reads(&self, marker: char, open: usize, close: usize) -> bool {
    self
      .pairs
      .iter()
      .any(|pair| pair.marker == marker && pair.open == open && pair.close == close)
  }

  /// Whether the cell is a backtick.
  fn tick(&self, index: usize) -> bool {
    self.cells.get(index).is_some_and(|cell| cell.ch == '`')
  }

  /// Whether the cell is a line break.
  fn line_break(&self, index: usize) -> bool {
    self.cells.get(index).is_some_and(|cell| cell.ch == '\n')
  }

  /// Whether a fence starts at the cell: exactly three backticks. A longer run is only text.
  fn fence(&self, index: usize) -> bool {
    (index..index + 3).all(|at| self.tick(at))
      && !self.tick(index + 3)
      && index.checked_sub(1).is_none_or(|before| !self.tick(before))
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

  /// Whether anything but white space stands in `cells`.
  fn has_text(&self, cells: Range<usize>) -> bool {
    self
      .cells
      .iter()
      .take(cells.end)
      .skip(cells.start)
      .any(|cell| !cell.ch.is_whitespace())
  }

  /// Leaves the cell out of the text.
  fn take(&mut self, index: usize) {
    if let Some(cell) = self.cells.get_mut(index) {
      cell.gone = true;
    }
  }

  /// Makes the cells code: no marker in them is read.
  fn literal(&mut self, cells: Range<usize>) {
    for cell in self.cells.iter_mut().take(cells.end).skip(cells.start) {
      cell.literal = true;
    }
  }

  /// Takes out the two markers of a pair, at the cells `open` and `close`, and notes the pair.
  fn pair(&mut self, marker: char, open: usize, close: usize) {
    self.take(open);
    self.take(close);
    if let (Some(open), Some(close)) = (self.cells.get(open), self.cells.get(close)) {
      let pair = Pair {
        marker,
        open: open.byte,
        close: close.byte,
      };
      self.pairs.push(pair);
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

  /// A code block opened by the fence at `open`: three backticks, the code, three backticks. Answers the cell to go
  /// on from. A fence that closes nothing, or around nothing but white space, is text.
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
    self.pair('`', open, close);
    self.literal(first..close);
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

  /// Takes out the bullet of each line that starts with `- ` or `* ` and goes on.
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
      }
      start = end + 1;
    }
  }

  /// Finds the pairs of `*`, `_` and `~`. A closing marker pairs with the nearest opening one of its kind before it
  /// on the line; the openers in between stay as typed.
  fn emphasis(&mut self) {
    let mut open: Vec<(char, usize)> = Vec::new();
    let mut found = Vec::new();
    for ((index, cell), (can_open, can_close)) in self.cells.iter().enumerate().zip(self.reach()) {
      if cell.free() && cell.ch == '\n' {
        open.clear();
      } else if cell.marker() {
        // A pair needs something between its markers.
        let partner = open
          .iter()
          .rposition(|(marker, _)| *marker == cell.ch)
          .and_then(|at| open.get(at).map(|(_, start)| (at, *start)))
          .filter(|(_, start)| can_close && start + 1 < index);
        if let Some((at, start)) = partner {
          open.truncate(at);
          found.push((cell.ch, start, index));
        } else if can_open {
          open.push((cell.ch, index));
        }
      }
    }
    for (marker, open, close) in found {
      self.pair(marker, open, close);
    }
  }

  /// For each cell, whether a marker there may open a pair and whether it may close one. Markers that touch are
  /// judged together, by what stands around them: they open when they start a word with text after them, and close
  /// when text comes before them and they end a word.
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
}

// Inline formats.

/// How a piece of text is already in a marker's pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wrapped {
  /// The piece is `*text*`: its own first and last characters are the markers.
  Within,
  /// The piece is the `text` of `*text*`: the markers are the characters just outside it.
  Around,
}

/// What the selection holds of each of its lines without the spaces at the ends of it, and without the bullet a line
/// starts with: the pieces an inline format wraps, one for each line, so that every marker has its text on its own
/// line.
fn pieces(text: &str, selection: &Range<usize>) -> Vec<Range<usize>> {
  touched_lines(text, selection)
    .into_iter()
    .filter_map(|line| {
      let start = line.start.max(selection.start);
      let held = text.get(start..line.end.min(selection.end))?;
      // The markers go after a bullet: in front of it the line would stop being a list item.
      let bullet = if start == line.start {
        bullet_width(held)
      } else {
        0
      };
      let held = held.get(bullet..)?;
      let trimmed = held.trim();
      let from = start + bullet + held.len() - held.trim_start().len();
      (!trimmed.is_empty()).then(|| from..from + trimmed.len())
    })
    .collect()
}

/// Whether `held` is a pair of `marker`s around text that has none of its own and no space at either end: what a
/// wrap leaves when its text and its markers are selected together, whether or not the core reads the pair. Text with
/// the marker in it (`~/dir`, for `~`) cannot be told from two spans, so it is not taken for a pair.
fn is_bare_pair(held: &str, marker: char) -> bool {
  held
    .strip_prefix(marker)
    .and_then(|rest| rest.strip_suffix(marker))
    .is_some_and(|inner| {
      !inner.contains(marker)
        && inner.chars().next().is_some_and(|first| !first.is_whitespace())
        && inner.chars().next_back().is_some_and(|last| !last.is_whitespace())
    })
}

/// Whether `piece` is already in a pair of `marker`s, and how. It is when the core reads a pair around it, so a
/// marker in the middle of a word (`snake_case_name`, `2*3*4`) or after `/` or `:` is a character, not a format. It is
/// also when the markers are its own first and last characters, because the selection then took them in on purpose:
/// that is how a wrap the core would not read is taken off again.
fn wrapped(reading: &Reading, text: &str, piece: &Range<usize>, marker: char) -> Option<Wrapped> {
  let width = marker.len_utf8();
  let held = text.get(piece.clone())?;
  let within =
    held.len() >= 2 * width && (reading.reads(marker, piece.start, piece.end - width) || is_bare_pair(held, marker));
  if within {
    return Some(Wrapped::Within);
  }
  let around = piece.start >= width && reading.reads(marker, piece.start - width, piece.end);
  around.then_some(Wrapped::Around)
}

/// Where the markers go in or come out of one piece. `unwrap` is true when every piece of the selection is wrapped: a
/// piece that is wrapped beside ones that are not stays as it is.
fn marker_changes(piece: &Range<usize>, marker: char, state: Option<Wrapped>, unwrap: bool) -> Vec<Change> {
  let width = marker.len_utf8();
  match state {
    None => vec![Change::insert(piece.start, marker), Change::insert(piece.end, marker)],
    Some(Wrapped::Within) if unwrap => vec![
      Change::remove(piece.start..piece.start + width),
      Change::remove(piece.end - width..piece.end),
    ],
    Some(Wrapped::Around) if unwrap => vec![
      Change::remove(piece.start - width..piece.start),
      Change::remove(piece.end..piece.end + width),
    ],
    Some(_) => Vec::new(),
  }
}

/// A pair of markers at `at` with the caret between them.
fn insert_pair(at: usize, marker: char) -> Edit {
  let caret = at + marker.len_utf8();
  Edit {
    changes: vec![Change::insert(at, format!("{marker}{marker}"))],
    selection: caret..caret,
  }
}

/// Whether the core reads the pair of `marker`s that `changes` put around `piece` of `text`.
fn reads_pair(text: &str, changes: &[Change], piece: &Range<usize>, marker: char) -> bool {
  let open = moved(changes, piece.start, false);
  let close = moved(changes, piece.end, false);
  Reading::of(&applied(text, changes)).reads(marker, open, close)
}

/// Bold, italic, strikethrough, or code. The pieces of the selection that have no pair of `marker`s around them get
/// one; when every piece has one, they all lose it. With only a caret, or only spaces, selected, a pair goes in at the
/// caret (or after the spaces). A pair the core would send as text does not count as one around a piece (see
/// `wrapped`): the piece gets a pair of its own, as asked, wherever it is.
///
/// What is selected afterwards is the text the markers are around, so that the same button undoes the edit, and what is
/// typed over it keeps the format. A selection over several lines takes its markers along instead: each line has its
/// own pair, and the text of the lines alone would be left without them. So does a pair the core would not read: with
/// only its text selected, the next press would find no pair to take off.
fn toggle_inline(text: &str, selection: Range<usize>, marker: char) -> Edit {
  let selection = clamp(text, selection);
  let pieces = pieces(text, &selection);
  let (Some(first), Some(last)) = (pieces.first(), pieces.last()) else {
    return insert_pair(selection.end, marker);
  };
  let reading = Reading::of(text);
  let states: Vec<Option<Wrapped>> = pieces.iter().map(|piece| wrapped(&reading, text, piece, marker)).collect();
  let unwrap = states.iter().all(Option::is_some);
  let changes: Vec<Change> = pieces
    .iter()
    .zip(&states)
    .flat_map(|(piece, state)| marker_changes(piece, marker, *state, unwrap))
    .collect();
  let text_only = pieces.len() == 1 && (unwrap || reads_pair(text, &changes, first, marker));
  let selection = moved(&changes, first.start, text_only)..moved(&changes, last.end, !text_only);
  Edit { changes, selection }
}

// Lists.

/// A bulleted list. Every line the selection touches gets `- ` in front; when they all have a bullet already, they
/// lose it. A line that is empty gets no bullet, unless they all are: a caret on an empty line starts a list.
fn toggle_bullets(text: &str, selection: Range<usize>) -> Edit {
  let selection = clamp(text, selection);
  let lines = touched_lines(text, &selection);
  let held = |line: &Range<usize>| text.get(line.clone()).unwrap_or_default();
  let mut targets: Vec<Range<usize>> = lines.iter().filter(|line| !held(line).trim().is_empty()).cloned().collect();
  if targets.is_empty() {
    targets = lines;
  }
  let remove = targets.iter().all(|line| bullet_width(held(line)) > 0);
  let changes: Vec<Change> = targets
    .iter()
    .filter_map(|line| {
      let width = bullet_width(held(line));
      if remove {
        Some(Change::remove(line.start..line.start + width))
      } else {
        (width == 0).then(|| Change::insert(line.start, BULLET))
      }
    })
    .collect();
  let selection = if selection.is_empty() {
    // A caret ends up after the bullet it got, not before it.
    let caret = moved(&changes, selection.start, true);
    caret..caret
  } else {
    // A selection that began at the start of a line keeps that line's bullet inside.
    moved(&changes, selection.start, false)..moved(&changes, selection.end, true)
  };
  Edit { changes, selection }
}

// Code blocks.

/// Whether a line break has to go in at `at` for what comes next to start a line of its own.
fn needs_break_before(text: &str, at: usize) -> bool {
  text
    .get(..at)
    .is_some_and(|before| !before.is_empty() && !before.ends_with('\n'))
}

/// Whether a line break has to go in at `at` for what came before to end a line of its own.
fn needs_break_after(text: &str, at: usize) -> bool {
  text
    .get(at..)
    .is_some_and(|after| !after.is_empty() && !after.starts_with('\n'))
}

/// A code block around the selection, its two fences on lines of their own: the selection stays as it is, apart from
/// the line breaks at its ends, which stay outside the block. With nothing selected, an empty block goes in at the
/// caret, and the caret on the empty line inside it.
fn wrap_code_block(text: &str, selection: Range<usize>) -> Edit {
  let selection = clamp(text, selection);
  let held = text.get(selection.clone()).unwrap_or_default();
  let code = held.trim_matches('\n');
  if code.is_empty() {
    return empty_code_block(text, selection.end);
  }
  let start = selection.start + held.len() - held.trim_start_matches('\n').len();
  let end = start + code.len();
  let before = if needs_break_before(text, start) {
    "\n"
  } else {
    ""
  };
  let after = if needs_break_after(text, end) {
    "\n"
  } else {
    ""
  };
  let changes = vec![
    Change::insert(start, format!("{before}{FENCE}\n")),
    Change::insert(end, format!("\n{FENCE}{after}")),
  ];
  let selection = moved(&changes, start, true)..moved(&changes, end, false);
  Edit { changes, selection }
}

/// An empty code block at `at`, with the caret on its middle line.
fn empty_code_block(text: &str, at: usize) -> Edit {
  let before = if needs_break_before(text, at) {
    "\n"
  } else {
    ""
  };
  let after = if needs_break_after(text, at) {
    "\n"
  } else {
    ""
  };
  let caret = at + before.len() + FENCE.len() + 1;
  Edit {
    changes: vec![Change::insert(at, format!("{before}{FENCE}\n\n{FENCE}{after}"))],
    selection: caret..caret,
  }
}

#[cfg(test)]
mod tests {
  use std::ops::Range;

  use super::{Edit, Format, Reading, applied};

  /// `text` with `edit` made.
  fn made(text: &str, edit: &Edit) -> String {
    applied(text, &edit.changes)
  }

  /// The text and the selection of `say [hello] there`: the selection in brackets, or `|` for a caret.
  fn parse(marked: &str) -> (String, Range<usize>) {
    if let Some(caret) = marked.find('|') {
      return (marked.replace('|', ""), caret..caret);
    }
    let start = marked.find('[').expect("a selection");
    let end = marked.find(']').expect("the end of the selection") - 1;
    (marked.replace(['[', ']'], ""), start..end)
  }

  /// `text` written the way `parse` reads it.
  fn show(text: &str, selection: &Range<usize>) -> String {
    let (before, rest) = text.split_at(selection.start);
    let (held, after) = rest.split_at(selection.len());
    if held.is_empty() {
      format!("{before}|{after}")
    } else {
      format!("{before}[{held}]{after}")
    }
  }

  /// What `format` makes of `selection` of `text`, written the way `parse` reads it.
  fn run_at(format: Format, text: &str, selection: Range<usize>) -> String {
    let edit = format.apply(text, selection);
    show(&made(text, &edit), &edit.selection)
  }

  /// What `format` makes of a draft written as `parse` reads it, written the same way.
  fn run(format: Format, marked: &str) -> String {
    let (text, selection) = parse(marked);
    run_at(format, &text, selection)
  }

  /// What the core leaves of `typed` once it has taken the markers out.
  fn read(typed: &str) -> String {
    Reading::of(typed)
      .cells
      .iter()
      .filter(|cell| !cell.gone)
      .map(|cell| cell.ch)
      .collect()
  }

  #[test]
  fn the_markers_are_read_as_the_core_reads_them() {
    // The cases of the core's own tests, in `crates/core/src/markup.rs`: the text it makes of each.
    let read_as = [
      ("*bold*", "bold"),
      ("_italic_", "italic"),
      ("~strike~", "strike"),
      ("`code`", "code"),
      ("say *two words* now", "say two words now"),
      ("*hi* _there_", "hi there"),
      ("*a* and _b_ and ~c~ and `d`", "a and b and c and d"),
      ("*_both_*", "both"),
      ("*bold _and italic_ text*", "bold and italic text"),
      ("~*_all_*~", "all"),
      ("*`code`*", "code"),
      ("_see `code` here_", "see code here"),
      ("**bold**", "bold"),
      ("__italic__", "italic"),
      ("~~strike~~", "strike"),
      ("(*bold*)", "(bold)"),
      ("\"_it_\"", "\"it\""),
      ("*bold*, then _it_. ~Gone~!", "bold, then it. Gone!"),
      ("*bold*'s", "bold's"),
      ("[`x`]", "[x]"),
      ("*a _b* c_", "a _b c_"),
      ("*one*\n*two*", "one\ntwo"),
      ("*a\n*b*", "*a\nb"),
      ("`*not bold*` and `a_b_c`", "*not bold* and a_b_c"),
      ("`- not a bullet`", "- not a bullet"),
      ("```*x* _y_ ~z~```", "*x* _y_ ~z~"),
      ("`a` *b*", "a b"),
      ("see (`a`), `b`.", "see (a), b."),
      ("```\nfn main() {\n  run();\n}\n```", "fn main() {\n  run();\n}"),
      ("```one line```", "one line"),
      ("before\n```\ncode\n```\nafter", "before\ncode\nafter"),
      ("a ```x``` b", "a x b"),
      ("```\n\ncode\n```", "\ncode"),
      ("```\n  indented\n```", "  indented"),
      ("```\n- a\n* b\n_c_\n```", "- a\n* b\n_c_"),
      ("- x\n```\n- y\n```\n- z", "x\n- y\nz"),
      ("- one\n- two", "one\ntwo"),
      ("* one\n* two", "one\ntwo"),
      ("List:\n- a\n- b\nafter", "List:\na\nb\nafter"),
      ("- one\n\n- two", "one\n\ntwo"),
      ("- *bold* item", "bold item"),
      ("* _a_\n* ~b~", "a\nb"),
      ("-  two spaces", " two spaces"),
      ("* *", "*"),
      ("café *olé* ñ", "café olé ñ"),
      ("日本語 *太字* です", "日本語 太字 です"),
      ("🎉*Release* notes", "🎉Release notes"),
      ("*é*", "é"),
      ("_café_", "café"),
      ("- é\n- 🎉", "é\n🎉"),
      ("```\ncafé\n```", "café"),
    ];
    for (typed, text) in read_as {
      assert_eq!(read(typed), text, "{typed:?}");
    }
  }

  #[test]
  fn what_the_core_sends_as_it_was_typed_is_left_as_it_is() {
    let as_typed = [
      "",
      "plain text, 100% plain.\nTwo lines.",
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
      "*oops",
      "oops*",
      "_a",
      "a_",
      "~",
      "`",
      "`open",
      "close`",
      "*a _b",
      "* ",
      "_ x _",
      "**",
      "__",
      "~~",
      "``",
      "(**)",
      "` `",
      "*one\ntwo*",
      "`one\ntwo`",
      "`a`b`",
      "`a``b`",
      "` a`",
      "`a `",
      "```",
      "```x",
      "x```",
      "``` ```",
      "``````",
      "```\n\n```",
      "```code``",
      "````\ncode\n````",
      "```a````",
      "````a```",
      "`````",
      "-one",
      "*one",
      "- ",
      "-  ",
      "* \n- ",
      " - indented",
      "a - b",
      "--",
      "-",
      "---",
      "café_name_é",
    ];
    for typed in as_typed {
      assert_eq!(read(typed), typed, "{typed:?}");
    }
  }

  #[test]
  fn a_pair_is_where_the_core_reads_one() {
    let reading = Reading::of("say *two words* now");
    assert!(reading.reads('*', 4, 14));
    assert!(!reading.reads('_', 4, 14), "another marker");
    assert!(!reading.reads('*', 4, 13), "the other end");
    assert!(!Reading::of("a*b*").reads('*', 1, 3), "inside a word");
    assert!(!Reading::of("`*x*`").reads('*', 1, 3), "inside code");
    assert!(Reading::of("`x`").reads('`', 0, 2));
    let doubled = Reading::of("**bold**");
    assert!(
      doubled.reads('*', 1, 6) && doubled.reads('*', 0, 7),
      "doubled markers format once"
    );
  }

  #[test]
  fn an_opening_marker_must_start_a_word() {
    // What a marker may follow: the start of a text, white space, an opening bracket or quote, a symbol.
    for before in ["", " ", "\n", "(", "[", "{", "<", "\"", "'", "🎉"] {
      let text = format!("{before}x z");
      let at = before.len();
      assert_eq!(
        run_at(Format::Bold, &text, at..at + 1),
        format!("{before}*[x]* z"),
        "{before:?}"
      );
    }
    // What it may not: a letter, a digit, other punctuation. It still goes in, with its text, and is selected with it.
    for before in ["a", "7", "é", "日", "/", ":", ".", "-", ",", ")", ">", "="] {
      let text = format!("{before}x z");
      let at = before.len();
      assert_eq!(
        run_at(Format::Bold, &text, at..at + 1),
        format!("{before}[*x*] z"),
        "{before:?}"
      );
    }
  }

  #[test]
  fn a_closing_marker_must_end_a_word() {
    for after in ["", " ", "\n", ".", ",", "!", ")", "]", "\"", "'", ">", "-", "/", "🎉"] {
      let text = format!("z x{after}");
      assert_eq!(run_at(Format::Bold, &text, 2..3), format!("z *[x]*{after}"), "{after:?}");
    }
    for after in ["a", "7", "é", "日"] {
      let text = format!("z x{after}");
      assert_eq!(run_at(Format::Bold, &text, 2..3), format!("z [*x*]{after}"), "{after:?}");
    }
  }

  #[test]
  fn a_selection_is_wrapped_in_the_marker() {
    assert_eq!(run(Format::Bold, "say [hello] there"), "say *[hello]* there");
    assert_eq!(run(Format::Italic, "say [hello] there"), "say _[hello]_ there");
    assert_eq!(run(Format::Strikethrough, "say [hello] there"), "say ~[hello]~ there");
    assert_eq!(run(Format::Code, "say [hello] there"), "say `[hello]` there");
  }

  #[test]
  fn the_spaces_of_a_selection_stay_outside_the_markers() {
    assert_eq!(run(Format::Bold, "say[ hello ]there"), "say *[hello]* there");
    assert_eq!(run(Format::Bold, "[  hello world  ]"), "  *[hello world]*  ");
  }

  #[test]
  fn a_wrapped_selection_is_unwrapped() {
    assert_eq!(run(Format::Bold, "say [*hello*] there"), "say [hello] there");
    assert_eq!(run(Format::Bold, "say *[hello]* there"), "say [hello] there");
    assert_eq!(run(Format::Code, "say `[hello]` there"), "say [hello] there");
    assert_eq!(run(Format::Strikethrough, "say [ ~hello~ ] there"), "say  [hello]  there");
    assert_eq!(run(Format::Bold, "say *[a]* there"), "say [a] there");
    assert_eq!(run(Format::Bold, "(*[hello]*), (_x_)"), "([hello]), (_x_)");
  }

  #[test]
  fn doubled_markers_come_off_a_pair_at_a_time() {
    assert_eq!(run(Format::Bold, "**[bold]**"), "*[bold]*");
  }

  #[test]
  fn pressing_a_format_twice_leaves_the_draft_as_it_was() {
    for format in [Format::Bold, Format::Italic, Format::Strikethrough, Format::Code] {
      let once = run(format, "say [hello world] there");
      assert_ne!(once, "say [hello world] there");
      assert_eq!(run(format, &once), "say [hello world] there");
    }
  }

  #[test]
  fn formats_nest() {
    assert_eq!(run(Format::Italic, "*[hello]*"), "*_[hello]_*");
    assert_eq!(run(Format::Bold, "_*[hello]*_"), "_[hello]_");
    assert_eq!(run(Format::Italic, "*[_hello_]*"), "*[hello]*");
    assert_eq!(run(Format::Bold, "_[hello]_"), "_*[hello]*_");
  }

  #[test]
  fn text_the_core_sends_as_markers_is_not_taken_for_formatted() {
    // In the middle of a word, or after `/` or `:`, the markers are characters: another pair goes in around the text.
    assert_eq!(run(Format::Italic, "snake_[case]_name"), "snake_[_case_]_name");
    assert_eq!(run(Format::Bold, "2*[3]*4"), "2*[*3*]*4");
    assert_eq!(run(Format::Bold, "see:*[bar]*"), "see:*[*bar*]*");
    assert_eq!(
      run(Format::Italic, "https://example.com/a_[b]_c"),
      "https://example.com/a_[_b_]_c"
    );
    assert_eq!(run(Format::Bold, "*[bold]*s"), "*[*bold*]*s");
    assert_eq!(run(Format::Bold, "`*[x]*`"), "`*[*x*]*`");
  }

  #[test]
  fn selecting_the_markers_with_the_text_takes_them_off_wherever_they_are() {
    assert_eq!(run(Format::Bold, "see:[*bar*]"), "see:[bar]");
    assert_eq!(run(Format::Bold, "[*bold*]s"), "[bold]s");
    assert_eq!(run(Format::Italic, "snake[_case_]name"), "snake[case]name");
  }

  #[test]
  fn a_wrap_the_core_would_not_read_still_goes_in_and_comes_off_again() {
    for (format, marked) in [
      (Format::Bold, "see:[bar]"),
      (Format::Bold, "see/[bar] now"),
      (Format::Italic, "foo[bar]"),
      (Format::Strikethrough, "[foo]bar"),
      (Format::Code, "a/[b]c"),
      (Format::Bold, "x[y\nz]w"),
      (Format::Italic, "2[3]4"),
    ] {
      let once = run(format, marked);
      assert_ne!(once, marked);
      assert_eq!(run(format, &once), marked, "{marked:?}");
    }
  }

  #[test]
  fn an_empty_selection_gets_a_pair_with_the_caret_between() {
    assert_eq!(run(Format::Bold, "say |there"), "say *|*there");
    assert_eq!(run(Format::Code, "|"), "`|`");
    assert_eq!(run(Format::Italic, "wörd|"), "wörd_|_");
  }

  #[test]
  fn a_selection_of_spaces_gets_the_pair_after_them() {
    assert_eq!(run(Format::Bold, "a[  ]b"), "a  *|*b");
  }

  #[test]
  fn each_line_of_a_selection_is_wrapped_on_its_own() {
    assert_eq!(run(Format::Bold, "[one\ntwo]"), "[*one*\n*two*]");
    assert_eq!(run(Format::Bold, "[one\n\ntwo]"), "[*one*\n\n*two*]");
    assert_eq!(run(Format::Italic, "o[ne\n  tw]o"), "o[_ne_\n  _tw_]o");
  }

  #[test]
  fn lines_are_unwrapped_when_all_of_them_are_wrapped() {
    assert_eq!(run(Format::Bold, "[*one*\n*two*]"), "[one\ntwo]");
    assert_eq!(run(Format::Bold, "[*one*\ntwo]"), "[*one*\n*two*]");
    assert_eq!(run(Format::Bold, &run(Format::Bold, "[one\ntwo]")), "[one\ntwo]");
  }

  #[test]
  fn the_markers_of_a_list_go_after_its_bullets() {
    assert_eq!(run(Format::Bold, "[- one\n* two]"), "- [*one*\n* *two*]");
    assert_eq!(run(Format::Bold, "- [one]"), "- *[one]*");
    assert_eq!(run(Format::Bold, "- [*one*]"), "- [one]");
  }

  #[test]
  fn wrapping_changes_only_the_two_places_of_the_markers() {
    let (text, selection) = parse("a [hello @Jordan Park] b");
    let edit = Format::Bold.apply(&text, selection.clone());
    let places: Vec<Range<usize>> = edit.changes.iter().map(|change| change.range.clone()).collect();
    assert_eq!(places, [selection.start..selection.start, selection.end..selection.end]);
  }

  #[test]
  fn a_selection_that_is_off_the_text_or_inside_a_character_is_held_to_it() {
    let edit = Format::Bold.apply("é", 1..99);
    assert_eq!(made("é", &edit), "*é*");
    let edit = Format::Bold.apply("", 3..7);
    assert_eq!(made("", &edit), "**");
    assert_eq!(edit.selection, 1..1);
  }

  #[test]
  fn every_line_the_selection_touches_gets_a_bullet() {
    assert_eq!(run(Format::Bullets, "[one\ntwo]"), "[- one\n- two]");
    assert_eq!(run(Format::Bullets, "o[ne\ntw]o"), "- o[ne\n- tw]o");
    assert_eq!(run(Format::Bullets, "a\n[b]\nc"), "a\n[- b]\nc");
  }

  #[test]
  fn a_caret_ends_up_after_the_bullet_it_got() {
    assert_eq!(run(Format::Bullets, "on|e"), "- on|e");
    assert_eq!(run(Format::Bullets, "|one"), "- |one");
    assert_eq!(run(Format::Bullets, "|"), "- |");
    assert_eq!(run(Format::Bullets, "one\n|"), "one\n- |");
  }

  #[test]
  fn bullets_come_off_when_every_line_has_one() {
    assert_eq!(run(Format::Bullets, "[- one\n- two]"), "[one\ntwo]");
    assert_eq!(run(Format::Bullets, "[* one\n- two]"), "[one\ntwo]");
    assert_eq!(run(Format::Bullets, "-| one"), "|one");
    assert_eq!(run(Format::Bullets, "- one|"), "one|");
  }

  #[test]
  fn lines_without_a_bullet_get_one_beside_lines_that_have_one() {
    assert_eq!(run(Format::Bullets, "[- one\ntwo]"), "[- one\n- two]");
  }

  #[test]
  fn empty_lines_of_a_selection_get_no_bullet() {
    assert_eq!(run(Format::Bullets, "[one\n\ntwo]"), "[- one\n\n- two]");
    assert_eq!(run(Format::Bullets, "[- one\n\n- two]"), "[one\n\ntwo]");
  }

  #[test]
  fn a_line_the_selection_only_reaches_the_start_of_is_left_alone() {
    assert_eq!(run(Format::Bullets, "[one\n]two"), "[- one\n]two");
  }

  #[test]
  fn a_bullet_with_no_space_after_it_is_text() {
    assert_eq!(run(Format::Bullets, "[*bold* and -5]"), "[- *bold* and -5]");
  }

  #[test]
  fn a_code_block_goes_around_the_selection_on_lines_of_its_own() {
    assert_eq!(run(Format::CodeBlock, "[npm test]"), "```\n[npm test]\n```");
    assert_eq!(
      run(Format::CodeBlock, "run:\n[npm test]\nnow"),
      "run:\n```\n[npm test]\n```\nnow"
    );
    assert_eq!(run(Format::CodeBlock, "[a\n  b]"), "```\n[a\n  b]\n```");
  }

  #[test]
  fn text_beside_the_selection_moves_to_lines_of_its_own() {
    assert_eq!(run(Format::CodeBlock, "run [npm test] now"), "run \n```\n[npm test]\n```\n now");
  }

  #[test]
  fn the_line_breaks_of_a_selected_line_stay_outside_the_block() {
    assert_eq!(run(Format::CodeBlock, "[npm test\n]next"), "```\n[npm test]\n```\nnext");
    assert_eq!(run(Format::CodeBlock, "x\n[\nnpm test]"), "x\n\n```\n[npm test]\n```");
  }

  #[test]
  fn an_empty_selection_gets_an_empty_block_with_the_caret_inside() {
    assert_eq!(run(Format::CodeBlock, "|"), "```\n|\n```");
    assert_eq!(run(Format::CodeBlock, "run:|"), "run:\n```\n|\n```");
    assert_eq!(run(Format::CodeBlock, "a|b"), "a\n```\n|\n```\nb");
    assert_eq!(run(Format::CodeBlock, "run:\n|\nnow"), "run:\n```\n|\n```\nnow");
  }

  #[test]
  fn every_selection_of_a_draft_makes_a_valid_edit() {
    let drafts = [
      "",
      "a",
      " a ",
      "\n\n",
      "hello world",
      "one\ntwo\n\nthree",
      "- one\n* two\nthree",
      "*bold* _it_ ~s~ `c` snake_case 2*3*4",
      "```\nx\n```\n- *y*",
      "wörd é\n  日本語 mixed",
      "see:*bar* (_x_) a/`b` **c** 🎉~d~",
    ];
    let formats = [
      Format::Bold,
      Format::Italic,
      Format::Strikethrough,
      Format::Bullets,
      Format::Code,
      Format::CodeBlock,
    ];
    for draft in drafts {
      let boundaries: Vec<usize> = draft.char_indices().map(|(at, _)| at).chain([draft.len()]).collect();
      for (ix, start) in boundaries.iter().enumerate() {
        for end in &boundaries[ix..] {
          for format in formats {
            let edit = format.apply(draft, *start..*end);
            let mut reached = 0;
            for change in &edit.changes {
              assert!(
                reached <= change.range.start && change.range.end <= draft.len(),
                "{format:?} {draft:?}"
              );
              reached = change.range.end;
            }
            let text = made(draft, &edit);
            let selection = &edit.selection;
            assert!(
              selection.start <= selection.end && selection.end <= text.len(),
              "{format:?} {draft:?}"
            );
            assert!(text.is_char_boundary(selection.start) && text.is_char_boundary(selection.end));
          }
        }
      }
    }
  }
}
