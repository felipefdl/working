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
    // A name that starts with what was typed beats one that only holds it: "jor" is Jordan before
    // "Brooks, Jorah".
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
    let mut fuzzy = Fuzzy::new("jdn");
    assert!(fuzzy.score("Jordan").is_some());
    assert!(fuzzy.score("jdn").is_some());
    assert!(fuzzy.score("Riley").is_none());
  }

  #[test]
  fn accents_are_ignored() {
    let mut fuzzy = Fuzzy::new("renee");
    assert!(fuzzy.score("Renée").is_some());
  }

  #[test]
  fn words_match_in_any_order() {
    let mut fuzzy = Fuzzy::new("park jordan");
    assert!(fuzzy.score("Jordan Park").is_some());
    assert!(fuzzy.score("Jordan").is_none());
  }

  #[test]
  fn prefix_beats_a_later_hit() {
    let mut fuzzy = Fuzzy::new("jor");
    let ranked = fuzzy.rank(["Brooks, Jorah", "Jordan"], |name| *name);
    assert_eq!(
      ranked.iter().map(|(_, name)| *name).collect::<Vec<_>>(),
      vec!["Jordan", "Brooks, Jorah"]
    );
  }

  #[test]
  fn weak_hits_drop_next_to_a_real_one() {
    let mut fuzzy = Fuzzy::new("jor pa");
    let ranked = fuzzy.rank(["Jordan Park", "Jira Sync for Design Review Group - Apr 9"], |name| *name);
    assert_eq!(ranked.iter().map(|(_, name)| *name).collect::<Vec<_>>(), vec!["Jordan Park"]);
  }

  #[test]
  fn anchors_and_negation() {
    let mut start = Fuzzy::new("^Jordan");
    assert!(start.score("Jordan Park").is_some());
    assert!(start.score("Park Jordan").is_none());
    let mut no = Fuzzy::new("jordan !park");
    assert!(no.score("Jordan Stone").is_some());
    assert!(no.score("Jordan Park").is_none());
  }
}
