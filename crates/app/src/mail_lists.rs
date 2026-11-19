//! The mail lists the store holds, one per mailbox asked for, and how each moves between asked, answered, and failed:
//! plain code with tests that replay the core's answers in the orders they come. A list the view shows is never left
//! neither loaded, nor on its way, nor failed (`MailLists::needs_asking` says when it must be asked for again).

use std::collections::HashMap;
use std::sync::Arc;

use gchat_core::{MailThread, Mailbox};

/// One mailbox's threads, as the core last answered them.
#[derive(Clone, Debug, Default)]
pub struct MailList {
  /// Newest first.
  pub threads: Arc<Vec<MailThread>>,
  /// Gmail has older threads than `threads`.
  pub more: bool,
  /// `threads` answers a load: false until the first answer.
  pub loaded: bool,
  /// A page is on its way: the newest, or the next older one.
  pub loading: bool,
  /// Why the last page asked for did not come, until the next answer or the next ask.
  pub failed: Option<String>,
}

/// The lists of the mailboxes asked for.
#[derive(Debug, Default)]
pub struct MailLists(HashMap<Mailbox, MailList>);

impl MailLists {
  /// The list of `mailbox`, if it was ever asked for or answered.
  pub fn get(&self, mailbox: &Mailbox) -> Option<&MailList> {
    self.0.get(mailbox)
  }

  /// Every list held.
  pub fn values(&self) -> impl Iterator<Item = &MailList> {
    self.0.values()
  }

  /// Marks `mailbox` as asked for, its newest page or with `more` the next older one, and answers whether
  /// `Command::LoadMail` should go: not while a page of it is on its way, nor for `more` before a first answer or once
  /// Gmail said there is nothing older.
  pub fn ask(&mut self, mailbox: &Mailbox, more: bool) -> bool {
    let list = self.0.entry(mailbox.clone()).or_default();
    if list.loading || (more && !(list.loaded && list.more)) {
      return false;
    }
    list.loading = true;
    list.failed = None;
    true
  }

  /// `Event::MailThreads`: the threads of `mailbox` replace what was held, whether it was asked for or not (the core
  /// also sends the lists a write changed), and it waits for nothing any more.
  pub fn answered(&mut self, mailbox: Mailbox, threads: Arc<Vec<MailThread>>, more: bool) {
    let list = self.0.entry(mailbox).or_default();
    list.threads = threads;
    list.more = more;
    list.loaded = true;
    list.loading = false;
    list.failed = None;
  }

  /// `Event::MailFailed`: every list on its way stops waiting and says why, so its Retry can ask again. Answers
  /// whether any was on its way: when none was, the failure is someone else's (a thread that did not open).
  pub fn failed(&mut self, message: &str) -> bool {
    let mut any = false;
    for list in self.0.values_mut().filter(|list| list.loading) {
      list.loading = false;
      list.failed = Some(message.to_string());
      any = true;
    }
    any
  }

  /// `Event::MailIncoming`: a new thread goes on top of the inbox held, in place of an older copy of it, before the
  /// core's next list of the inbox comes. Answers whether the inbox was there to take it.
  pub fn incoming(&mut self, thread: MailThread) -> bool {
    let Some(inbox) = self.0.get_mut(&Mailbox::Inbox).filter(|inbox| inbox.loaded) else {
      return false;
    };
    let threads = Arc::make_mut(&mut inbox.threads);
    threads.retain(|held| held.id != thread.id);
    threads.insert(0, thread);
    true
  }

  /// Whether `mailbox` must be asked for to show anything: it has no threads, none are on their way, and it did not
  /// fail. Nothing would ever answer it otherwise.
  pub fn needs_asking(&self, mailbox: &Mailbox) -> bool {
    self
      .get(mailbox)
      .is_none_or(|list| !list.loaded && !list.loading && list.failed.is_none())
  }
}

/// What a mailbox is, for traces: never its search words or label.
pub const fn mailbox_kind(mailbox: &Mailbox) -> &'static str {
  match mailbox {
    Mailbox::Inbox => "inbox",
    Mailbox::Starred => "starred",
    Mailbox::Sent => "sent",
    Mailbox::All => "all",
    Mailbox::Label(_) => "label",
    Mailbox::Search(_) => "search",
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn thread(id: &str) -> MailThread {
    MailThread {
      id: id.into(),
      subject: "Notes from Jordan Park".into(),
      snippet: String::new(),
      senders: Vec::new(),
      time: 0,
      messages: 1,
      unread: false,
      starred: false,
      attachments: false,
    }
  }

  fn threads(ids: &[&str]) -> Arc<Vec<MailThread>> {
    Arc::new(ids.iter().map(|id| thread(id)).collect())
  }

  /// The order seen live: All asked for while a write was going; the write's own lists, then its answer, then the
  /// thread that was open, then All.
  #[test]
  fn a_mailbox_asked_for_during_a_write_shows_its_answer() {
    let mut lists = MailLists::default();
    lists.answered(Mailbox::Inbox, threads(&["t1", "t2"]), false);
    assert!(lists.ask(&Mailbox::All, false));
    assert!(!lists.needs_asking(&Mailbox::All));
    lists.answered(Mailbox::Inbox, threads(&["t2"]), false);
    lists.answered(Mailbox::All, threads(&["t1", "t2", "t3"]), true);
    let all = lists.get(&Mailbox::All).unwrap();
    assert!(all.loaded && !all.loading && all.more);
    assert_eq!(all.threads.len(), 3);
  }

  /// The same answers the other way round: All's list before the write's.
  #[test]
  fn answers_in_any_order_land_on_their_own_list() {
    let mut lists = MailLists::default();
    assert!(lists.ask(&Mailbox::Starred, false));
    lists.answered(Mailbox::Starred, threads(&["s1"]), false);
    lists.answered(Mailbox::Inbox, threads(&["t1"]), false);
    assert!(lists.get(&Mailbox::Starred).unwrap().loaded);
    assert_eq!(lists.get(&Mailbox::Inbox).unwrap().threads.len(), 1);
  }

  /// A failure while All was on its way, then the inbox answering: All says why instead of waiting forever.
  #[test]
  fn a_failure_leaves_its_list_failed_not_waiting() {
    let mut lists = MailLists::default();
    lists.answered(Mailbox::Inbox, threads(&["t1"]), false);
    assert!(lists.ask(&Mailbox::All, false));
    assert!(lists.failed("Gmail did not answer."));
    lists.answered(Mailbox::Inbox, threads(&["t1"]), false);
    let all = lists.get(&Mailbox::All).unwrap();
    assert!(!all.loaded && !all.loading);
    assert_eq!(all.failed.as_deref(), Some("Gmail did not answer."));
    assert!(!lists.needs_asking(&Mailbox::All));
    // Retry asks again, and the failure goes.
    assert!(lists.ask(&Mailbox::All, false));
    assert!(lists.get(&Mailbox::All).unwrap().failed.is_none());
  }

  #[test]
  fn a_failure_with_nothing_asked_for_is_someone_elses() {
    let mut lists = MailLists::default();
    lists.answered(Mailbox::Inbox, threads(&["t1"]), false);
    assert!(!lists.failed("The thread did not open."));
    assert!(lists.get(&Mailbox::Inbox).unwrap().failed.is_none());
  }

  #[test]
  fn a_list_never_asked_for_needs_asking_and_one_on_its_way_does_not_go_twice() {
    let mut lists = MailLists::default();
    assert!(lists.needs_asking(&Mailbox::Sent));
    assert!(lists.ask(&Mailbox::Sent, false));
    assert!(!lists.ask(&Mailbox::Sent, false));
    assert!(!lists.ask(&Mailbox::Sent, true));
    lists.answered(Mailbox::Sent, threads(&["s1"]), true);
    assert!(lists.ask(&Mailbox::Sent, true));
    lists.answered(Mailbox::Sent, threads(&["s1", "s2"]), false);
    assert!(!lists.ask(&Mailbox::Sent, true));
  }

  #[test]
  fn new_mail_goes_on_top_of_a_loaded_inbox_only() {
    let mut lists = MailLists::default();
    assert!(!lists.incoming(thread("t9")));
    lists.answered(Mailbox::Inbox, threads(&["t1", "t9"]), false);
    assert!(lists.incoming(thread("t9")));
    let ids: Vec<&str> = lists
      .get(&Mailbox::Inbox)
      .unwrap()
      .threads
      .iter()
      .map(|t| t.id.as_str())
      .collect();
    assert_eq!(ids, ["t9", "t1"]);
  }

  #[test]
  fn traces_name_the_kind_of_mailbox_only() {
    assert_eq!(mailbox_kind(&Mailbox::Search("from:jpark@example.com".into())), "search");
    assert_eq!(mailbox_kind(&Mailbox::Label("Label_7".into())), "label");
    assert_eq!(mailbox_kind(&Mailbox::All), "all");
  }
}
