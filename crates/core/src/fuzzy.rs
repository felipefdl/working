//! fzf-style matching of short names (conversations, people), shared by the app's search and the MCP tools.
//!
//! A query's words must all match, in any order. A word matches as a subsequence ("fbn" finds "Fabiano"),
//! ignoring case and accents ("joao" finds "João"). `'word` wants the exact letters together, `^word` at the
//! start, `word$` at the end, and `!word` leaves matches out.
#![allow(clippy::unnecessary_sort_by, reason = "stable order for equal scores")]

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// Fuzzy matcher for conversation and people names.
pub struct Fuzzy {
  pattern: Pattern,
  matcher: Matcher,
  buffer: Vec<char>,
}

impl Fuzzy {
  /// Parses `query` in fzf syntax.
  pub fn new(query: &str) -> Self {
    // A name that starts with what was typed beats one that only holds it: "fab" is Fabiano before
    // "Vitor, Fabio".
    let mut config = Config::DEFAULT;
    config.prefer_prefix = true;
    Self {
      pattern: Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart),
      matcher: Matcher::new(config),
      buffer: Vec::new(),
    }
  }

  /// How well `text` matches; `None` when it does not. Higher is better.
  pub fn score(&mut self, text: &str) -> Option<u32> {
    self.pattern.score(Utf32Str::new(text, &mut self.buffer), &mut self.matcher)
  }

  /// The items that match, best first; equal scores keep the order they came in. A match scoring under four
  /// fifths of the best one is left out: next to a real hit, those are letters scattered over a long name
  /// ("fab eg" in "Front-End Team Meeting - Feb 23").
  pub fn rank<T>(&mut self, items: impl IntoIterator<Item = T>, text: impl Fn(&T) -> &str) -> Vec<(u32, T)> {
    let mut hits: Vec<(u32, T)> = items
      .into_iter()
      .filter_map(|item| self.score(text(&item)).map(|score| (score, item)))
      .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    let floor = hits.first().map_or(0, |best| best.0 * 4 / 5);
    hits.retain(|hit| hit.0 >= floor);
    hits
  }
}

#[cfg(test)]
mod tests {
  use super::Fuzzy;

  #[test]
  fn subsequence_and_case() {
    let mut fuzzy = Fuzzy::new("fbn");
    assert!(fuzzy.score("Fabiano").is_some());
    assert!(fuzzy.score("fbn").is_some());
    assert!(fuzzy.score("Ana").is_none());
  }

  #[test]
  fn accents_are_ignored() {
    let mut fuzzy = Fuzzy::new("joao");
    assert!(fuzzy.score("João").is_some());
  }

  #[test]
  fn words_match_in_any_order() {
    let mut fuzzy = Fuzzy::new("lima ana");
    assert!(fuzzy.score("Ana Lima").is_some());
    assert!(fuzzy.score("Ana").is_none());
  }

  #[test]
  fn prefix_beats_a_later_hit() {
    let mut fuzzy = Fuzzy::new("fab");
    let ranked = fuzzy.rank(["Vitor, Fabio", "Fabiano"], |name| *name);
    assert_eq!(
      ranked.iter().map(|(_, name)| *name).collect::<Vec<_>>(),
      vec!["Fabiano", "Vitor, Fabio"]
    );
  }

  #[test]
  fn weak_hits_drop_next_to_a_real_one() {
    let mut fuzzy = Fuzzy::new("fab eg");
    let ranked = fuzzy.rank(["Fabiano Eger", "Front-End Team Meeting - Feb 23"], |name| *name);
    assert_eq!(ranked.iter().map(|(_, name)| *name).collect::<Vec<_>>(), vec!["Fabiano Eger"]);
  }

  #[test]
  fn anchors_and_negation() {
    let mut start = Fuzzy::new("^Ana");
    assert!(start.score("Ana Lima").is_some());
    assert!(start.score("Lima Ana").is_none());
    let mut no = Fuzzy::new("ana !lima");
    assert!(no.score("Ana Costa").is_some());
    assert!(no.score("Ana Lima").is_none());
  }
}
