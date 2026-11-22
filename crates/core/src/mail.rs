//! Gmail with the Chat session: the sync JSON the Gmail web page itself calls (`/sync/u/0/i/bv` for a thread list,
//! `/sync/u/0/i/fd` for a thread's emails, `/sync/u/0/i/s` for a change), in Gmail's positional arrays.
//!
//! Opening a session loads the Gmail page once. On its way there through accounts.google.com, Google sets Gmail's
//! own cookies; the page holds the XSRF token, the id key the sync calls carry, and the address mail is sent from.
//! Reading changes nothing: a thread stays unread until `Client::mark` marks it read. The only changes are marking
//! read or unread, archiving or moving back to the inbox, starring, the user's own labels, saving a draft, and
//! sending a reply, a new email (a forward is one), or a saved draft.
//!
//! Every index into an answer or a change is a named path below, so a change of Gmail's shapes is fixed in one place.
//! The field map is in `AGENTS.md`.
#![allow(
  clippy::expect_used,
  clippy::significant_drop_tightening,
  reason = "the regexes and the header template are static and checked by the tests, and the session lock is held \
            while the session opens so two calls do not open it twice"
)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use reqwest::cookie::Jar;
use serde_json::{Value, json};

use crate::http::{ServiceSession, SessionCookie, USER_AGENT, asks_sign_in, read_cookies};
use crate::model::{
  MailAddress, MailAttachment, MailCompose, MailDraft, MailForward, MailLabel, MailMessage, MailReply, MailSignature,
  MailThread, Mailbox, Micros,
};

const ORIGIN: &str = "https://mail.google.com";
const PAGE: &str = "https://mail.google.com/mail/u/0/";
const SYNC: &str = "https://mail.google.com/sync/u/0/i";
/// Threads asked for per page. Gmail may answer a few more on later pages.
const PAGE_SIZE: u64 = 50;
/// Longest a read (`bv`, `fd`) waits for its whole answer before it fails, so a stuck request turns into
/// `MailFailed` and never holds a list in Loading.
const READ_TIMEOUT: Duration = Duration::from_secs(20);
/// Longest a change (`s`) waits for its answer.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// The `x-gmail-btai` header the web page sends, as captured, with the per-session fields left null: the
/// `BTAI_*` paths say where each goes.
const BTAI_TEMPLATE: &str = r#"[null,null,[null,null,null,null,null,null,null,null,null,1,null,null,1,null,0,1,1,0,1,null,null,1,1,1,1,1,1,1,0,1,1,1,1,1,0,1,"en",null,1,0,25,null,0,1,0,1,1,1,1,1,null,1,1,null,1,1,0,1,null,1,1,null,1,1,null,null,1,null,1,0,1,1,null,0,0,0,null,null,1,100,1,1,0,1,0,null,0,0,0,1,1,null,null,1,null,null,1,null,null,1,null,1,0,1,0,1,1,1,0,null,1,null,1,1,1,1,1,1,1,0,1,1,1,1,0,1,1,1,1,1,0,1,1,0,0,0,0,0,0,0],null,null,null,25,null,1,5,"",null,null,null,null,null,"","",null]"#;
const BTAI_USER_AGENT: &str = "/2/37";
const BTAI_ID_KEY: &str = "/4";
const BTAI_BUILD: &str = "/7";
/// The device's offset from UTC, in milliseconds.
const BTAI_OFFSET: &str = "/11";
const BTAI_TIME_ZONE: &str = "/12";
/// `GLOBALS[2]` of the page.
const BTAI_GLOBALS: &str = "/15";
/// Now, in milliseconds.
const BTAI_NOW: &str = "/18";

/// A `bv` request for the first page of the inbox, as captured; `BV_*` paths say what each call changes.
const BV_TEMPLATE: &str = r#"[[1,50,null,"in:inbox",[null,null,null,null,5],"itemlist-ViewType(1)-1",1,2000,null,0,null,null,null,1,null,null,null,null,0,1,null,null,null,null,null,null,null,0,0,"",null,null,[1794331236783,null,null,46]],null,[0,5,null,null,1,1,1]]"#;
const BV_SIZE: &str = "/0/1";
const BV_QUERY: &str = "/0/3";
/// Page index: page `n` starts at thread `n × size`.
const BV_PAGE: &str = "/0/9";

// The page: `_GM_setData({...})` objects and `GLOBALS=[...]`.
/// `_GM_setData` key of `["sdpc", "<xsrf token>", "<id key>", ...]`.
const PAGE_TOKENS: &str = "w43KIf";
const TOKENS_XSRF: usize = 1;
const TOKENS_ID_KEY: usize = 2;
const GLOBALS_NUMBER: usize = 2;
const GLOBALS_BUILD: usize = 3;
/// `GLOBALS[10]`: the account's address.
const GLOBALS_EMAIL: usize = 10;
/// `_GM_setData` key whose `[6][1][0]` is the send-as identity `[email, …, name at 4]`.
const PAGE_IDENTITY: &str = "sBEv4c";
const IDENTITY: &[usize] = &[6, 1, 0];
const IDENTITY_EMAIL: usize = 0;
const IDENTITY_NAME: usize = 4;
/// `sBEv4c` is a list of tagged entries; the one tagged `"p"` holds the account's settings as `[name, value]` pairs
/// (`SETTINGS_PAIRS`), values as strings.
const SETTINGS_TAG: &str = "p";
const SETTINGS_PAIRS: usize = 1;
/// "Insert signature before quoted text in replies and remove the "--" line that precedes it": `"1"` on, `"0"` off.
const SETTING_SIGNATURE_BEFORE_QUOTE: &str = "bx_stw";
/// The signature switch: `"1"` when the account signs its mail (absent before a signature was ever set).
const SETTING_SIGNATURE_ON: &str = "bx_se";
/// Gmail's plain-text rendering of the default signature (bold as `*word*`), kept only as the last fallback when the
/// rich settings have no signature.
const SETTING_SIGNATURE: &str = "sx_sg";
/// `_GM_setData` key whose value is a JSON string of the account's rich settings: at `RICH_SETTINGS_LIST` a list of
/// `[number, {extension id: value}]` entries.
const PAGE_RICH_SETTINGS: &str = "w4hJLd";
const RICH_SETTINGS_LIST: &str = "/0/0";
/// The named signatures and their picks (entry `"105"` live): `[[signatures], [picks]]`, at `NAMED_SIGNATURES` the
/// signatures `[id, null, name, [null, html]]`, at `SIGNATURE_PICKS` per address `[address, new id, reply id]`.
const SIGNATURES_EXTENSION: &str = "221494602";
const NAMED_SIGNATURES: &[usize] = &[0, 0];
const NAMED_SIGNATURE_HTML: &[usize] = &[3, 1];
const SIGNATURE_PICKS: &[usize] = &[1, 0];
/// "For new emails use" and "On reply/forward use", in the settings page's order (the live account picks the same
/// signature for both, so the order is not proven by a difference).
const PICK_NEW: usize = 1;
const PICK_REPLY: usize = 2;
/// Per address, one signature for both kinds (entry `"61"` live): at `ADDRESS_SIGNATURES` `[address, [null, html]]`.
const ADDRESS_SIGNATURES_EXTENSION: &str = "138780968";
const ADDRESS_SIGNATURES: &[usize] = &[0];
const ADDRESS_SIGNATURE_HTML: &[usize] = &[1, 1];

// A signature as Gmail's own client sends it (its `SignatureTextFormatterImpl`, checked against a Gmail-sent new email
// and reply): `SIGNATURE_BREAK`, then for the `-- ` line `SIGNATURE_PREFIX`, then the signature in a `gmail_signature`
// element, which carries `SIGNATURE_SMARTMAIL` in a new email but not in a reply. Before the quoted text of a reply
// (that setting on) it is the break and the element in a `<div>`, without the `-- ` line.
const SIGNATURE_BREAK: &str = "<div><br clear=\"all\"></div>";
const SIGNATURE_PREFIX: &str = "<div><br></div><span class=\"gmail_signature_prefix\">-- </span><br>";
const SIGNATURE_SMARTMAIL: &str = " data-smartmail=\"gmail_signature\"";

// `s`: a change. Envelope `[null, [[op…]], [1, sync version, null, null, [null, 0], null, 1], [earlier ms, op count,
// now ms, 1 when an op sends, 90], 2]`; an op is `[op index, [thread id, payload]]`.
/// Answer `[[[op index, code, null, sync version], …]]`; code 0 is accepted.
const WRITE_RESULTS: &[usize] = &[0, 0];
const RESULT_CODE: usize = 1;
const RESULT_VERSION: usize = 3;
/// The `bv` answer's sync version.
const LIST_VERSION: &[usize] = &[4];
/// Payload slot of a label change: `[[added], [removed] | null, [message ids]]`. Message ids are required: without
/// them Gmail answers 0 and changes nothing.
const OP_LABELS: usize = 6;
/// Payload slot that makes a new thread: `[[subject, null, now ms, thread id, [message as a draft]]]`. A new email's
/// thread (`thread-a:r<random i64>`, minted by the client) is made this way before its email is sent.
const OP_CREATE: usize = 2;
/// Payload slot that saves a draft in a thread Gmail already has: `[message as a draft]` (draft labels, no kind).
/// Gmail's reply box saves the reply this way before sending that same message.
const OP_SAVE_DRAFT: usize = 1;
/// Payload slot of a send: `[message, answered message id, 1, null, [now ms]]` for a reply, `[message, null, 1]`
/// for a new email, whose thread `OP_CREATE` made.
const OP_SEND: usize = 13;
const LABEL_OPENED: &str = "^o";
const LABEL_UNSEEN: &str = "^us";
const LABEL_ARCHIVED: &str = "^a";

// A message as a send carries it (`OP_SEND`'s first entry), by slot.
const SENT_ID: usize = 0;
/// `[1, email, name, null×6, email]`.
const SENT_FROM: usize = 1;
const SENT_TO: usize = 2;
const SENT_CC: usize = 3;
/// Who gets a copy the other recipients do not see, after to and cc in the same address shape (as an `fd` detail
/// lists to, cc, bcc); a live send confirmed it. Left out without anyone, as cc is.
const SENT_BCC: usize = 4;
const SENT_TIME: usize = 6;
const SENT_SUBJECT: usize = 7;
/// `[null, [[0, "<written html>"], [2, "<quoted html>"]], null, null, null, null, 1]`.
const SENT_BODY: usize = 8;
const SENT_LABELS: usize = 10;
/// In-Reply-To: the answered email's Message-ID.
const SENT_IN_REPLY_TO: usize = 15;
const SENT_TIME_AGAIN: usize = 17;
const SENT_FLAG: usize = 41;
const SENT_FLAGS: usize = 42;
/// `s:<client session>|#<message id>|0`, again at `[76][6]`.
const SENT_CLIENT_ID: usize = 51;
/// 2 for a reply (1 for a new email).
const SENT_KIND: usize = 74;
const SENT_CLIENT_ID_AGAIN: usize = 76;
const SENT_EXTRAS: usize = 77;
const SENT_SLOTS: usize = 78;
const SENT_KIND_REPLY: u8 = 2;
const SENT_KIND_NEW: u8 = 1;
/// What a sent email carries when it leaves: sent, pending until Gmail sends it, and the web client's own marks.
const SENT_LABEL_SET: [&str; 7] = ["^all", "^pfg", "^f_bt", "^f_btns", "^f_cl", "^io_im", "^io_imc3"];
/// What a saved draft carries (a new thread's first email, a reply before it is sent): a draft, and the web client's
/// own marks.
const DRAFT_LABEL_SET: [&str; 5] = ["^all", "^r", "^r_bt", "^io_im", "^io_imc3"];
/// Body part kinds of a send.
const PART_WRITTEN: u8 = 0;
const PART_QUOTED: u8 = 2;

// `bv` answer.
const LIST_THREADS: &[usize] = &[2];
/// Rows `[label, unread threads, total threads]`.
const LIST_LABEL_COUNTS: &[usize] = &[6, 0];
const LABEL_COUNT_UNREAD: usize = 1;
/// `1` when older threads exist past this page, `2` at the end.
const LIST_END: &[usize] = &[11, 0];
const LIST_MORE: u64 = 1;
/// Label rows `[[id, name, …, colors at 14, …, kind at 17, …], n]`.
const LIST_LABELS: &[usize] = &[1];
const LABEL_ID: &[usize] = &[0, 0];
const LABEL_NAME: &[usize] = &[0, 1];
/// `[text color, background color]`, each `0xRRGGBB` as a number; absent when the label has no color.
const LABEL_BACKGROUND: &[usize] = &[0, 14, 1];
const LABEL_KIND: &[usize] = &[0, 17];
/// `LABEL_KIND` of the user's own labels (Gmail's are 5, categories 1).
const LABEL_KIND_USER: u64 = 2;
/// Prefix of the user's own label ids; `in:<id>` lists a label's threads.
const USER_LABEL_PREFIX: &str = "^x_";

// A thread of the `bv` list: `[thread, history id]`.
const THREAD_SUBJECT: &[usize] = &[0, 0];
const THREAD_SNIPPET: &[usize] = &[0, 1];
const THREAD_TIME: &[usize] = &[0, 2];
const THREAD_ID: &[usize] = &[0, 3];
const THREAD_MESSAGES: &[usize] = &[0, 4];

// A message of a `bv` thread.
const LISTED_FROM: &[usize] = &[1];
const LISTED_LABELS: &[usize] = &[10];
const LISTED_ATTACHMENTS: &[usize] = &[11];

// `fd` answer: entries `[thread id, [summary, [message summaries]], [[message id, detail]], null, n]`.
const DETAILS_THREADS: &[usize] = &[1];
const ENTRY_ID: &[usize] = &[0];
const ENTRY_SUMMARIES: &[usize] = &[1, 1];
const ENTRY_DETAILS: &[usize] = &[2];
const SUMMARY_ID: &[usize] = &[0];
const SUMMARY_FROM: &[usize] = &[1];
const SUMMARY_TIME: &[usize] = &[2];
const SUMMARY_LABELS: &[usize] = &[3];
const DETAIL_ID: &[usize] = &[0];
const DETAIL_FIELDS: &[usize] = &[1];
const DETAIL_TO: &[usize] = &[0];
const DETAIL_CC: &[usize] = &[1];
/// Only the user's own sent emails carry it.
const DETAIL_BCC: &[usize] = &[2];
const DETAIL_REPLY_TO: &[usize] = &[3];
/// The email's own Message-ID header (`[8]` is References, `[9]` In-Reply-To).
const DETAIL_MESSAGE_ID: &[usize] = &[7];
/// The thread's subject in the `fd` entry's summary.
const ENTRY_SUBJECT: &[usize] = &[1, 0, 1];
/// Body parts `[kind, null, [null, "<html>"], hash]`, in order.
const DETAIL_PARTS: &[usize] = &[5, 1];
const PART_HTML: &[usize] = &[2, 1];
const DETAIL_ATTACHMENTS: &[usize] = &[13];

// An attachment: `[[regular, part id, 1, [image, url, name, mime, size, ids]], ...]`.
/// `1` for a file, `0` for an image inside the HTML.
const ATTACHMENT_REGULAR: &[usize] = &[0, 0];
const ATTACHMENT_URL: &[usize] = &[0, 3, 1];
const ATTACHMENT_NAME: &[usize] = &[0, 3, 2];
const ATTACHMENT_MIME: &[usize] = &[0, 3, 3];
const ATTACHMENT_SIZE: &[usize] = &[0, 3, 4];

// An address: `[1, email, name]`, the name left out when the header has none.
const ADDRESS_EMAIL: usize = 1;
const ADDRESS_NAME: usize = 2;

const LABEL_INBOX: &str = "^i";
const LABEL_UNREAD: &str = "^u";
const LABEL_STARRED: &str = "^t";
/// A draft Gmail holds, not sent (an `fd` summary's labels).
const LABEL_DRAFT: &str = "^r";

/// What a sync call answers when it fails: `["er", null, null, null, null, <http status>, ...]`.
const ERROR_TAG: &str = "er";
const ERROR_STATUS: &[usize] = &[5];

/// Google refused the session (400 or 401: a stale XSRF token or signed-out cookies): it is opened again once.
#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str("Gmail refused the session")
  }
}

impl std::error::Error for Refused {}

/// One page of a mailbox's threads.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ThreadPage {
  /// Threads, newest first. A later page can repeat a thread of the one before it.
  pub threads: Vec<MailThread>,
  /// Gmail has older threads than these.
  pub more: bool,
  /// Unread threads in the inbox, when Gmail said.
  pub inbox_unread: Option<u32>,
  /// The user's labels by name, when the answer carried them (every list answer seen does).
  pub labels: Option<Vec<MailLabel>>,
}

/// Gmail for the session in `cookies.json`. Cheap to make: the session opens on the first call.
pub struct Client {
  signed: ServiceSession,
  session: tokio::sync::Mutex<Option<Arc<Session>>>,
}

impl Client {
  /// A client for the session `signed` names.
  pub fn new(signed: ServiceSession) -> Self {
    Self {
      signed,
      session: tokio::sync::Mutex::new(None),
    }
  }

  async fn session(&self) -> Result<Arc<Session>> {
    let mut held = self.session.lock().await;
    if let Some(session) = held.as_ref() {
      return Ok(session.clone());
    }
    let started = std::time::Instant::now();
    let session = Arc::new(self.signed.opened(Session::open(&self.signed).await)?);
    tracing::debug!("Gmail session opened: {} ms", started.elapsed().as_millis());
    *held = Some(session.clone());
    Ok(session)
  }

  /// Runs `work` with the session, opening a fresh one (a new token) once when Google refuses the one held.
  async fn with_session<T, F, Fut>(&self, work: F) -> Result<T>
  where
    T: Send,
    F: Fn(Arc<Session>) -> Fut + Send + Sync,
    Fut: Future<Output = Result<T>> + Send,
  {
    match work(self.session().await?).await {
      Err(error) if error.downcast_ref::<Refused>().is_some() => {
        *self.session.lock().await = None;
        work(self.session().await?).await
      },
      other => other,
    }
  }

  /// Forgets the session, so the next call opens a new one.
  pub async fn close(&self) {
    *self.session.lock().await = None;
  }

  /// Page `page` (0 the newest) of a mailbox's threads.
  pub async fn threads(&self, mailbox: &Mailbox, page: u32) -> Result<ThreadPage> {
    let request = list_request(mailbox, page);
    let answer = self
      .with_session(|session| {
        let request = request.clone();
        async move {
          let answer = session.call("bv", &request).await?;
          session.note_version(&answer);
          Ok(answer)
        }
      })
      .await?;
    thread_page(&answer)
  }

  /// A thread's emails, oldest first. Leaves them unread.
  pub async fn messages(&self, thread_id: &str) -> Result<Vec<MailMessage>> {
    let answer = self
      .with_session(|session| async move { session.details(thread_id).await })
      .await?;
    Ok(thread_detail(&answer, thread_id)?.messages)
  }

  /// Downloads an attachment or an image of an email (`mail.google.com` URLs need Gmail's own cookies).
  pub async fn fetch(&self, url: &str) -> Result<Vec<u8>> {
    self.with_session(|session| async move { session.fetch(url).await }).await
  }

  /// The account's signatures as the session's page gave them; a new session (after a 400 or 401, or `close`) reads
  /// them again.
  pub(crate) async fn signatures(&self) -> Result<Signatures> {
    Ok(self.session().await?.signatures.clone())
  }

  /// The address mail goes from, as the session's page said.
  pub(crate) async fn me(&self) -> Result<Option<MailAddress>> {
    Ok(self.session().await?.me.clone())
  }

  /// Marks every email of a thread read, or its newest one unread, as Gmail's Mark as read and Mark as unread do.
  pub async fn mark(&self, thread: &str, read: bool) -> Result<()> {
    self
      .with_session(|session| async move {
        let ids = session.message_ids(thread).await?;
        let op = if read {
          label_op(&[LABEL_OPENED], &[LABEL_UNREAD, LABEL_UNSEEN], &ids)
        } else {
          label_op(&[LABEL_UNREAD], &[], ids.last().map_or_default(std::slice::from_ref))
        };
        session.write(thread, vec![op], false).await
      })
      .await
  }

  /// Takes a thread out of the inbox, or puts it back.
  pub async fn archive(&self, thread: &str, archive: bool) -> Result<()> {
    self
      .with_session(|session| async move {
        let ids = session.message_ids(thread).await?;
        let op = if archive {
          label_op(&[LABEL_ARCHIVED], &[LABEL_INBOX], &ids)
        } else {
          label_op(&[LABEL_INBOX], &[LABEL_ARCHIVED], &ids)
        };
        session.write(thread, vec![op], false).await
      })
      .await
  }

  /// Stars a thread, as Gmail's star does (`^t` on its newest email), or unstars it (`^t` off every email).
  pub async fn star(&self, thread: &str, starred: bool) -> Result<()> {
    self
      .with_session(|session| async move {
        let ids = session.message_ids(thread).await?;
        session.write(thread, vec![star_op(&ids, starred)], false).await
      })
      .await
  }

  /// Puts the user's labels `add` on every email of a thread and takes `remove` off. Only the user's own labels (ids
  /// `^x_…`) change: any other id is refused before anything is sent.
  pub async fn label(&self, thread: &str, add: &[String], remove: &[String]) -> Result<()> {
    check_user_labels(add, remove)?;
    self
      .with_session(|session| async move {
        let ids = session.message_ids(thread).await?;
        session.write(thread, vec![labels_op(add, remove, &ids)], false).await
      })
      .await
  }

  /// Sends `reply` into its thread from the signed-in address: the written text above Gmail's quote of the answered
  /// email, subject `Re: ` and the thread's, In-Reply-To the answered email. As Gmail's reply box: the reply saved
  /// as a draft in the thread, then that same message sent. Sent without the draft, Gmail's server cut the signature
  /// out into a part of its own that its reader shows gray.
  pub async fn reply(&self, reply: &MailReply) -> Result<()> {
    check_recipients(&reply.to, &reply.cc, &[])?;
    let id = &new_message_id();
    self
      .with_session(|session| async move {
        let draft = session.reply_email(reply, id).await?;
        session.save(&draft).await?;
        session.send(&draft).await
      })
      .await
  }

  /// Sends `compose` as a new email from the signed-in address, in the new thread `thread` (`new_thread_id`): the
  /// thread is made with the email as a draft, then that same email is sent. Who it goes to is checked first:
  /// nothing is sent without a recipient or with one that is not an address. Answers whether the user is among its
  /// recipients, which puts it in their inbox too.
  pub async fn compose(&self, compose: &MailCompose, thread: &str) -> Result<bool> {
    let recipients = &Recipients::checked(&compose.to, &compose.cc, &compose.bcc)?;
    let text = &NewText::of(compose);
    let id = &new_message_id();
    self
      .with_session(|session| async move { session.send_new(recipients, text, thread, id).await })
      .await
  }

  /// Forwards an email as Gmail's Forward does, as a new email in the new thread `thread` (`new_thread_id`), sent as
  /// `compose` sends one: subject `Fwd: ` and the thread's, what the user wrote, then Gmail's forwarded block (who
  /// wrote it, when, the subject, and who it went to, above its HTML). Its files are not carried. The email must be
  /// in its thread as Gmail has it now, and not a draft. Answers whether the user is among its recipients.
  pub async fn forward(&self, forward: &MailForward, thread: &str) -> Result<bool> {
    let recipients = &Recipients::checked(&forward.to, &forward.cc, &forward.bcc)?;
    let id = &new_message_id();
    self
      .with_session(|session| async move {
        let detail = thread_detail(&session.details(&forward.thread).await?, &forward.thread)?;
        let message = detail
          .messages
          .iter()
          .find(|message| message.id == forward.message)
          .context("The email to forward is not in that thread.")?;
        if message.draft {
          bail!("A draft cannot be forwarded.");
        }
        let subject = forward_subject(&detail.subject);
        let text = NewText {
          subject: &subject,
          text: &forward.text,
          signature: forward.signature,
          forwarded: Some(forwarded_html(message, &detail.subject)),
        };
        session.send_new(recipients, &text, thread, id).await
      })
      .await
  }

  /// Saves `draft` in Gmail as the new email `id` (`new_message_id`) in `thread` (a reply's own thread, a new email's
  /// new one from `new_thread_id`) and answers what its send needs (`send_draft`); nothing is sent. Who it goes to is
  /// checked first. A new email's draft makes its thread (`OP_CREATE`), as `compose` does; a reply's is saved into
  /// its thread (`OP_SAVE_DRAFT`), as `reply` does. A draft is saved once: Gmail took a second save of the same email
  /// but kept showing the first.
  pub(crate) async fn save_draft(&self, draft: &MailDraft, thread: &str, id: &str) -> Result<Draft> {
    match draft {
      MailDraft::New(compose) => {
        let recipients = &Recipients::checked(&compose.to, &compose.cc, &compose.bcc)?;
        let text = &NewText::of(compose);
        self
          .with_session(|session| async move {
            let draft = session.new_email(recipients, text, thread, id)?;
            session.save(&draft).await?;
            Ok(draft)
          })
          .await
      },
      MailDraft::Reply(reply) => {
        if reply.thread != thread {
          bail!("A reply's draft stays in the thread it answers.");
        }
        check_recipients(&reply.to, &reply.cc, &[])?;
        self
          .with_session(|session| async move {
            let draft = session.reply_email(reply, id).await?;
            session.save(&draft).await?;
            Ok(draft)
          })
          .await
      },
    }
  }

  /// Sends a draft `save_draft` saved, exactly as saved: the same email, with the sent labels and kind.
  pub(crate) async fn send_draft(&self, draft: &Draft) -> Result<()> {
    self.with_session(|session| async move { session.send(draft).await }).await
  }
}

/// A new thread's id, as the web client mints one for a new email.
pub(crate) fn new_thread_id() -> String {
  format!("thread-a:r{}", rand::random::<i64>())
}

/// A new email's id, as the web client mints one for each email it writes; its draft and the sent email share it.
pub(crate) fn new_message_id() -> String {
  format!("msg-a:r{}", rand::random::<i64>())
}

/// Addresses as a send carries them: name and address trimmed.
fn trimmed(addresses: &[MailAddress]) -> Vec<MailAddress> {
  addresses
    .iter()
    .map(|address| MailAddress {
      name: address.name.trim().to_string(),
      email: address.email.trim().to_string(),
    })
    .collect()
}

/// A new email's recipients as a send carries them: trimmed, and checked before anything is sent.
struct Recipients {
  to: Vec<MailAddress>,
  cc: Vec<MailAddress>,
  bcc: Vec<MailAddress>,
}

impl Recipients {
  /// `to`, `cc`, and `bcc` trimmed, with someone among them and each an address (`check_recipients`).
  fn checked(to: &[MailAddress], cc: &[MailAddress], bcc: &[MailAddress]) -> Result<Self> {
    let recipients = Self {
      to: trimmed(to),
      cc: trimmed(cc),
      bcc: trimmed(bcc),
    };
    check_recipients(&recipients.to, &recipients.cc, &recipients.bcc)?;
    Ok(recipients)
  }
}

/// What a new email says: its subject, what the user wrote, whether the signature for new emails goes under it, and a
/// forward's block (`forwarded_html`) after that.
struct NewText<'a> {
  subject: &'a str,
  text: &'a str,
  signature: bool,
  forwarded: Option<String>,
}

impl<'a> NewText<'a> {
  /// A new email's, as the user wrote it.
  fn of(compose: &'a MailCompose) -> Self {
    Self {
      subject: compose.subject.trim(),
      text: &compose.text,
      signature: compose.signature,
      forwarded: None,
    }
  }
}

struct Session {
  http: reqwest::Client,
  xsrf: String,
  /// The `x-gmail-btai` header with this session's fields; `BTAI_NOW` is set per call.
  btai: Value,
  /// The `c` parameter: the page's sync calls count up from 0.
  calls: AtomicU64,
  /// The address mail is sent from, when the page said.
  me: Option<MailAddress>,
  /// The account's signatures, as the page's settings gave them.
  signatures: Signatures,
  /// This session's own id, which a sent email carries (`SENT_CLIENT_ID`).
  client: String,
  /// The next change's op index. Gmail takes any start, 1 included.
  ops: AtomicU64,
  /// The newest sync version an answer gave. Gmail takes a stale one, 0 included.
  version: AtomicU64,
  /// When the last change went out, which the next one names.
  last_write: AtomicU64,
}

impl Session {
  async fn open(signed: &ServiceSession) -> Result<Self> {
    let cookies = read_cookies(signed.cookie_path())?;
    let http = reqwest::Client::builder()
      .user_agent(USER_AGENT)
      .cookie_provider(Arc::new(jar(&cookies)))
      .connect_timeout(Duration::from_secs(15))
      .timeout(Duration::from_secs(45))
      // A connection Google or the network dropped while idle is found by a ping, not by a request that hangs on it.
      .http2_keep_alive_interval(Duration::from_secs(20))
      .http2_keep_alive_timeout(Duration::from_secs(10))
      .http2_keep_alive_while_idle(true)
      .pool_idle_timeout(Duration::from_secs(60))
      .build()?;
    // Every mail call waits for the session while it opens: a page that does not come fails in `READ_TIMEOUT`.
    let response = http
      .get(PAGE)
      .timeout(READ_TIMEOUT)
      .send()
      .await
      .map_err(|error| too_slow(error, READ_TIMEOUT))?;
    if asks_sign_in(response.url()) {
      return Err(signed.refused());
    }
    let host = response.url().host_str().unwrap_or_default().to_string();
    let page = response.text().await?;
    if host != "mail.google.com" {
      bail!("Gmail did not open");
    }
    let config = PageConfig::parse(&page)?;
    let offset_ms = i64::from(chrono::Local::now().offset().local_minus_utc()) * 1000;
    let btai = config.btai(USER_AGENT, offset_ms, &crate::engine::device_time_zone());
    Ok(Self {
      http,
      xsrf: config.xsrf,
      btai,
      calls: AtomicU64::new(0),
      me: config.me,
      signatures: config.signatures,
      client: format!("s:{:016x}", rand::random::<u64>()),
      ops: AtomicU64::new(1),
      version: AtomicU64::new(0),
      last_write: AtomicU64::new(0),
    })
  }

  /// The `fd` answer for one thread.
  async fn details(&self, thread: &str) -> Result<Value> {
    self.call("fd", &json!([[[thread, 1]], 2])).await
  }

  /// Sends `ops` on `thread` as one change and checks Gmail took each of them.
  async fn write(&self, thread: &str, ops: Vec<Value>, sends: bool) -> Result<()> {
    let count = u64::try_from(ops.len()).unwrap_or(u64::MAX);
    let first = self.ops.fetch_add(count, Ordering::Relaxed);
    let numbered: Vec<Value> = (first..).zip(ops).map(|(index, op)| json!([index, [thread, op]])).collect();
    let now = now_ms();
    let earlier = match self.last_write.swap(now, Ordering::Relaxed) {
      0 => now,
      earlier => earlier,
    };
    let version = self.version.load(Ordering::Relaxed);
    let body = json!([
      null,
      [numbered],
      [1, version, null, null, [null, 0], null, 1],
      [earlier, count, now, u8::from(sends), 90],
      2
    ]);
    let answer = self.call("s", &body).await?;
    let results = items(&answer, WRITE_RESULTS);
    if results.is_empty() {
      bail!("Gmail did not confirm the change");
    }
    for result in results {
      if let Some(version) = result.get(RESULT_VERSION).and_then(Value::as_u64) {
        self.version.fetch_max(version, Ordering::Relaxed);
      }
      if result.get(RESULT_CODE).and_then(Value::as_u64) != Some(0) {
        bail!("Gmail did not take the change");
      }
    }
    Ok(())
  }

  /// The ids of a thread's emails, oldest first, as a label change needs them.
  async fn message_ids(&self, thread: &str) -> Result<Vec<String>> {
    message_ids_of(&self.details(thread).await?, thread)
  }

  /// `reply` as the email `id` from the signed-in address, read against its thread as Gmail has it now.
  async fn reply_email(&self, reply: &MailReply, id: &str) -> Result<Draft> {
    let me = self.me.as_ref().context("Gmail did not say which address sends mail")?;
    let detail = thread_detail(&self.details(&reply.thread).await?, &reply.thread)?;
    let (answered, header) = detail
      .messages
      .iter()
      .zip(&detail.message_ids)
      .find(|(message, _)| message.id == reply.message)
      .context("the email to answer is no longer in the thread")?;
    let signature = self.signatures.reply.as_ref().filter(|_| reply.signature);
    let (written, quoted) = reply_body(&reply.text, quoted_html(answered), signature, self.signatures.before_quote);
    let email = SentEmail {
      id,
      client: &self.client,
      me,
      to: &reply.to,
      cc: &reply.cc,
      bcc: &[],
      subject: &reply_subject(&detail.subject),
      written,
      answering: Some(Answering { in_reply_to: header.as_deref(), quoted }),
      now: now_ms(),
    };
    Ok(email_draft(&email, &reply.thread, Some(&answered.id), false))
  }

  /// A new email as the email `id` from the signed-in address, to `recipients`, in the new thread `thread`: `text`
  /// written as Gmail's compose writes it, the signature for new emails under it when asked, then a forward's block.
  fn new_email(&self, recipients: &Recipients, text: &NewText<'_>, thread: &str, id: &str) -> Result<Draft> {
    let me = self.me.as_ref().context("Gmail did not say which address sends mail")?;
    let signature = self.signatures.new.as_ref().filter(|_| text.signature);
    let mut written = written_html(
      text.text,
      signature
        .map(|signature| signature_block(&signature.html, SignaturePlace::new_email(text.text)))
        .as_deref(),
    );
    if let Some(forwarded) = &text.forwarded {
      written.push_str(forwarded);
    }
    let email = SentEmail {
      id,
      client: &self.client,
      me,
      to: &recipients.to,
      cc: &recipients.cc,
      bcc: &recipients.bcc,
      subject: text.subject,
      written,
      answering: None,
      now: now_ms(),
    };
    Ok(email_draft(&email, thread, None, signature.is_some()))
  }

  /// A new email, as the web client sends one: its thread is made with the email as a draft, then that same email is
  /// sent. Answers whether the user is among its recipients.
  async fn send_new(&self, recipients: &Recipients, text: &NewText<'_>, thread: &str, id: &str) -> Result<bool> {
    let draft = self.new_email(recipients, text, thread, id)?;
    self.save(&draft).await?;
    self.send(&draft).await?;
    Ok(draft.to_me)
  }

  /// Saves `draft` in Gmail: a new email's makes its new thread with it, a reply's goes into its thread.
  async fn save(&self, draft: &Draft) -> Result<()> {
    self.write(&draft.thread, vec![draft_op(draft)], false).await
  }

  /// Sends a saved draft, which turns it into the sent email. A send into a thread Gmail does not have yet is
  /// refused.
  async fn send(&self, draft: &Draft) -> Result<()> {
    self.write(&draft.thread, vec![send_op(draft, now_ms())], true).await
  }

  /// Keeps the sync version a list answer gave.
  fn note_version(&self, answer: &Value) {
    if let Some(version) = at(answer, LIST_VERSION).as_u64() {
      self.version.fetch_max(version, Ordering::Relaxed);
    }
  }

  /// POSTs `body` to the sync endpoint `kind` (`bv`, `fd`, or `s`) and answers its JSON. Its time goes to the log
  /// (the kind and the milliseconds; never the content).
  async fn call(&self, kind: &str, body: &Value) -> Result<Value> {
    let started = std::time::Instant::now();
    let result = self.call_once(kind, body).await;
    tracing::debug!(
      "Gmail {kind}: {} ms{}",
      started.elapsed().as_millis(),
      if result.is_ok() { "" } else { ", failed" }
    );
    result
  }

  async fn call_once(&self, kind: &str, body: &Value) -> Result<Value> {
    let mut btai = self.btai.clone();
    if let Some(now) = btai.pointer_mut(BTAI_NOW) {
      *now = Value::from(now_ms());
    }
    let count = self.calls.fetch_add(1, Ordering::Relaxed);
    let limit = if kind == "s" {
      WRITE_TIMEOUT
    } else {
      READ_TIMEOUT
    };
    let response = self
      .http
      .post(format!("{SYNC}/{kind}"))
      .timeout(limit)
      .query(&[("hl", "en"), ("c", &count.to_string()), ("rt", "r"), ("pt", "ji")])
      .header("content-type", "application/json")
      .header("x-framework-xsrf-token", &self.xsrf)
      .header("x-gmail-btai", btai.to_string())
      .header("x-google-btd", "1")
      .header("origin", ORIGIN)
      .header("referer", PAGE)
      .body(body.to_string())
      .send()
      .await
      .map_err(|error| too_slow(error, limit))?;
    let status = response.status();
    let text = response.text().await.map_err(|error| too_slow(error, limit))?;
    if status == reqwest::StatusCode::BAD_REQUEST || status == reqwest::StatusCode::UNAUTHORIZED {
      return Err(Refused.into());
    }
    let answer = parse_answer(&text);
    if let Some(code) = answer.as_ref().ok().and_then(error_status) {
      if code == 400 || code == 401 {
        return Err(Refused.into());
      }
      bail!("Gmail answered error {code}");
    }
    if !status.is_success() {
      bail!("Gmail answered {status}");
    }
    answer
  }

  async fn fetch(&self, url: &str) -> Result<Vec<u8>> {
    let response = self.http.get(url).send().await.context("Gmail did not answer")?;
    if response.url().host_str() == Some("accounts.google.com") {
      return Err(Refused.into());
    }
    let status = response.status();
    if !status.is_success() {
      bail!("Gmail answered {status}");
    }
    Ok(response.bytes().await?.to_vec())
  }
}

/// A request that failed: one that ran out of time says how long it waited, safe to show.
fn too_slow(error: reqwest::Error, limit: Duration) -> anyhow::Error {
  if error.is_timeout() {
    anyhow!("Gmail did not answer in {} s", limit.as_secs())
  } else {
    anyhow::Error::new(error).context("Gmail did not answer")
  }
}

/// What a session takes from the Gmail page.
#[derive(Debug, PartialEq)]
struct PageConfig {
  /// The whole `x-framework-xsrf-token` value (token, colon, and expiry).
  xsrf: String,
  id_key: String,
  /// `GLOBALS[2]`, a number the page echoes in its header.
  globals_number: Value,
  build: String,
  /// The address mail is sent from, with its name when the page gave one.
  me: Option<MailAddress>,
  /// The account's signatures, as far as the page's settings say.
  signatures: Signatures,
}

/// The signatures Gmail adds for the signed-in address, and where a reply's goes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Signatures {
  /// For new emails ("For new emails use").
  pub(crate) new: Option<MailSignature>,
  /// For replies ("On reply/forward use").
  pub(crate) reply: Option<MailSignature>,
  /// A reply's signature goes above the quoted text, without the `-- ` line (`SETTING_SIGNATURE_BEFORE_QUOTE`).
  pub(crate) before_quote: bool,
}

/// The account's signatures from the page's settings.
///
/// Read live (`rich` is `PAGE_RICH_SETTINGS`, `settings` is `sBEv4c`): the formatted signatures and the picks are in
/// the rich settings' `SIGNATURES_EXTENSION` (named signatures, each `[id, null, name, [null, html]]`, and per address
/// `[address, id for new emails, id for replies]`, an empty id meaning no signature), else in
/// `ADDRESS_SIGNATURES_EXTENSION` (per address `[address, [null, html]]`, both kinds). `sx_sg` in `sBEv4c` is only
/// Gmail's plain-text rendering of the default signature (`*bold*`), kept as the last fallback. Everything waits for
/// the signature switch (`SETTING_SIGNATURE_ON`). `me` picks the address's entry; without one, the first.
fn page_signatures(settings: Option<&Value>, rich: Option<&Value>, me: Option<&str>) -> Signatures {
  let setting = |name: &str| {
    settings?
      .as_array()?
      .iter()
      .find(|entry| entry.get(0).and_then(Value::as_str) == Some(SETTINGS_TAG))?
      .get(SETTINGS_PAIRS)?
      .as_array()?
      .iter()
      .find(|pair| pair.get(0).and_then(Value::as_str) == Some(name))?
      .get(1)?
      .as_str()
      .map(str::to_string)
  };
  let before_quote = setting(SETTING_SIGNATURE_BEFORE_QUOTE).as_deref() == Some("1");
  if setting(SETTING_SIGNATURE_ON).as_deref() != Some("1") {
    return Signatures { before_quote, ..Signatures::default() };
  }
  let extension = |id: &str| {
    rich?
      .pointer(RICH_SETTINGS_LIST)?
      .as_array()?
      .iter()
      .find_map(|entry| entry.get(1)?.get(id))
  };
  // The entry of the signed-in address in a list of `[address, …]`, else the first.
  let mine = |entries: &[Value]| -> Option<Value> {
    entries
      .iter()
      .find(|entry| {
        let address = entry.get(0).and_then(Value::as_str).unwrap_or_default();
        me.is_some_and(|me| address.eq_ignore_ascii_case(me))
      })
      .or_else(|| entries.first())
      .cloned()
  };
  let from_html = |html: &str| (!html.trim().is_empty()).then(|| html_signature(html.to_string()));
  // Named signatures and their picks first, then the address's own one, then the plain form.
  let named = extension(SIGNATURES_EXTENSION).map(|named| {
    let signatures = items(named, NAMED_SIGNATURES);
    let html_of = |id: Option<&str>| {
      let id = id.filter(|id| !id.is_empty())?;
      let signature = signatures
        .iter()
        .find(|signature| signature.get(0).and_then(Value::as_str) == Some(id))?;
      from_html(at(signature, NAMED_SIGNATURE_HTML).as_str()?)
    };
    let picks = mine(items(named, SIGNATURE_PICKS)).unwrap_or_default();
    (
      html_of(picks.get(PICK_NEW).and_then(Value::as_str)),
      html_of(picks.get(PICK_REPLY).and_then(Value::as_str)),
    )
  });
  let by_address = || {
    extension(ADDRESS_SIGNATURES_EXTENSION).map(|by_address| {
      let entry = mine(items(by_address, ADDRESS_SIGNATURES)).unwrap_or_default();
      let signature = at(&entry, ADDRESS_SIGNATURE_HTML).as_str().and_then(from_html);
      (signature.clone(), signature)
    })
  };
  let plain = || {
    let signature = setting(SETTING_SIGNATURE)
      .filter(|value| !value.trim().is_empty())
      .map(|value| setting_signature(&value));
    (signature.clone(), signature)
  };
  let (new, reply) = named.or_else(by_address).unwrap_or_else(plain);
  Signatures { new, reply, before_quote }
}

/// A signature from its HTML, its text without markup (`<b>Word</b>` reads `Word`).
fn html_signature(html: String) -> MailSignature {
  MailSignature { text: html_to_text(&html), html }
}

/// A signature setting's value as HTML and text: plain text gets its characters escaped and its lines broken; a
/// value with HTML elements is kept as it is.
fn setting_signature(value: &str) -> MailSignature {
  static MARKUP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)</?(a|b|br|div|em|font|i|img|li|ol|p|span|strong|table|td|tr|u|ul)(\s[^<>]*)?/?>")
      .expect("valid pattern")
  });
  let html = if MARKUP.is_match(value) {
    value.to_string()
  } else {
    escape_html(&value.replace("\r\n", "\n")).replace('\n', "<br>")
  };
  html_signature(html)
}

impl PageConfig {
  fn parse(page: &str) -> Result<Self> {
    let mut data = serde_json::Map::new();
    for (at, call) in page.match_indices("_GM_setData(") {
      if let Some(Value::Object(object)) = page.get(at + call.len()..).and_then(first_json) {
        data.extend(object);
      }
    }
    let tokens = data
      .get(PAGE_TOKENS)
      .context("the Gmail page has no session token; Gmail may have changed")?;
    let string = |value: &Value, index: usize| value.get(index).and_then(Value::as_str).map(str::to_string);
    let globals = page
      .find("GLOBALS=")
      .and_then(|at| page.get(at + "GLOBALS=".len()..))
      .and_then(first_json)
      .context("the Gmail page has no GLOBALS; Gmail may have changed")?;
    let identity = data.get(PAGE_IDENTITY).map(|identity| at(identity, IDENTITY));
    let me = identity
      .and_then(|identity| {
        Some(MailAddress {
          email: string(identity, IDENTITY_EMAIL)?,
          name: string(identity, IDENTITY_NAME).unwrap_or_default(),
        })
      })
      .or_else(|| {
        Some(MailAddress {
          email: string(&globals, GLOBALS_EMAIL)?,
          name: String::new(),
        })
      });
    let signatures = page_signatures(
      data.get(PAGE_IDENTITY),
      data
        .get(PAGE_RICH_SETTINGS)
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str(text).ok())
        .as_ref(),
      me.as_ref().map(|me| me.email.as_str()),
    );
    Ok(Self {
      xsrf: string(tokens, TOKENS_XSRF).context("the Gmail page has no XSRF token")?,
      id_key: string(tokens, TOKENS_ID_KEY).context("the Gmail page has no id key")?,
      globals_number: globals.get(GLOBALS_NUMBER).cloned().unwrap_or(Value::Null),
      build: string(&globals, GLOBALS_BUILD).context("the Gmail page has no build label")?,
      me,
      signatures,
    })
  }

  /// The `x-gmail-btai` header of this page, its time left for each call to set.
  fn btai(&self, user_agent: &str, offset_ms: i64, time_zone: &str) -> Value {
    static TEMPLATE: LazyLock<Value> =
      LazyLock::new(|| serde_json::from_str(BTAI_TEMPLATE).expect("valid header template"));
    let mut btai = TEMPLATE.clone();
    let fields = [
      (BTAI_USER_AGENT, Value::from(user_agent)),
      (BTAI_ID_KEY, Value::from(self.id_key.as_str())),
      (BTAI_BUILD, Value::from(self.build.as_str())),
      (BTAI_OFFSET, Value::from(offset_ms)),
      (BTAI_TIME_ZONE, Value::from(time_zone)),
      (BTAI_GLOBALS, self.globals_number.clone()),
    ];
    for (path, value) in fields {
      if let Some(slot) = btai.pointer_mut(path) {
        *slot = value;
      }
    }
    btai
  }
}

/// The first JSON value at the start of `text`, ignoring what follows it.
fn first_json(text: &str) -> Option<Value> {
  serde_json::Deserializer::from_str(text)
    .into_iter::<Value>()
    .next()
    .and_then(Result::ok)
}

/// A sync answer's JSON, without the `)]}'` guard Gmail sometimes puts first.
fn parse_answer(text: &str) -> Result<Value> {
  let text = text.trim_start();
  let text = text.strip_prefix(")]}'").unwrap_or(text);
  serde_json::from_str(text).context("Gmail answered something that is not JSON")
}

/// The HTTP status of an `["er", ...]` answer.
fn error_status(answer: &Value) -> Option<u64> {
  (answer.get(0).and_then(Value::as_str) == Some(ERROR_TAG)).then(|| at(answer, ERROR_STATUS).as_u64().unwrap_or(0))
}

fn now_ms() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

/// Every cookie of `.google.com`, plus those of accounts.google.com, so the redirects through accounts.google.com
/// can set Gmail's own.
fn jar(cookies: &[SessionCookie]) -> Jar {
  let jar = Jar::default();
  for cookie in cookies {
    let host = cookie.domain.trim_start_matches('.');
    let shared = cookie.domain == ".google.com" || cookie.domain == "google.com";
    if !shared && host != "accounts.google.com" {
      continue;
    }
    let Ok(url) = format!("https://{host}/").parse::<reqwest::Url>() else {
      continue;
    };
    // `__Host-` cookies and those of one host carry no Domain attribute.
    let line = if cookie.name.starts_with("__Host-") || !cookie.domain.starts_with('.') {
      format!("{}={}; Path={}; Secure", cookie.name, cookie.value, cookie.path)
    } else {
      format!("{}={}; Domain={host}; Path={}; Secure", cookie.name, cookie.value, cookie.path)
    };
    jar.add_cookie_str(&line, &url);
  }
  jar
}

/// Gmail's search for a mailbox. A label goes by its id (`in:^x_…`), which holds for any name: spaces, slashes, and
/// accents need no escaping.
fn query(mailbox: &Mailbox) -> String {
  match mailbox {
    Mailbox::Inbox => "in:inbox".into(),
    Mailbox::Starred => "is:starred".into(),
    Mailbox::Sent => "in:sent".into(),
    Mailbox::All => "in:all".into(),
    Mailbox::Label(id) => format!("in:{id}"),
    Mailbox::Search(text) => text.clone(),
  }
}

/// The `bv` request for page `page` of `mailbox`.
fn list_request(mailbox: &Mailbox, page: u32) -> Value {
  static TEMPLATE: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(BV_TEMPLATE).expect("valid list template"));
  let mut request = TEMPLATE.clone();
  let fields = [
    (BV_SIZE, Value::from(PAGE_SIZE)),
    (BV_QUERY, Value::from(query(mailbox))),
    (BV_PAGE, Value::from(page)),
  ];
  for (path, value) in fields {
    if let Some(slot) = request.pointer_mut(path) {
      *slot = value;
    }
  }
  request
}

/// The value at `path` of positional arrays, `Null` where any step is missing.
fn at<'a>(value: &'a Value, path: &[usize]) -> &'a Value {
  static NULL: Value = Value::Null;
  path.iter().try_fold(value, |value, &index| value.get(index)).unwrap_or(&NULL)
}

fn items<'a>(value: &'a Value, path: &[usize]) -> &'a [Value] {
  at(value, path).as_array().map_or(&[], Vec::as_slice)
}

fn text(value: &Value, path: &[usize]) -> String {
  at(value, path).as_str().unwrap_or_default().to_string()
}

fn has_label(message: &Value, path: &[usize], label: &str) -> bool {
  items(message, path).iter().any(|held| held.as_str() == Some(label))
}

/// Milliseconds at `path` as `Micros`.
fn micros(value: &Value, path: &[usize]) -> Micros {
  at(value, path).as_i64().and_then(|ms| ms.checked_mul(1000)).unwrap_or_default()
}

fn address(value: &Value) -> Option<MailAddress> {
  let email = value.get(ADDRESS_EMAIL)?.as_str()?.to_string();
  let name = clean_name(value.get(ADDRESS_NAME).and_then(Value::as_str).unwrap_or_default());
  Some(MailAddress { name, email })
}

/// A display name without the quotes Gmail sometimes leaves around it (`"Jordan Park"`, a group's
/// `'Jordan Park at jpark@example.com' via Team`, or a lone leading apostrophe): trimmed, with one pair of single or
/// double quotes taken out and the rest kept.
fn clean_name(raw: &str) -> String {
  let name = raw.trim();
  let Some(quote) = name.chars().next().filter(|first| matches!(first, '"' | '\'')) else {
    return name.to_string();
  };
  let rest = name.strip_prefix(quote).unwrap_or(name);
  let cleaned = rest.rfind(quote).map_or_else(
    || rest.to_string(),
    |close| {
      let (inside, after) = rest.split_at(close);
      format!("{inside}{}", after.strip_prefix(quote).unwrap_or(after))
    },
  );
  cleaned.trim().to_string()
}

fn addresses(value: &Value, path: &[usize]) -> Vec<MailAddress> {
  items(value, path).iter().filter_map(address).collect()
}

/// The files of a message, leaving out the images its HTML shows.
fn attachments(value: &Value, path: &[usize]) -> Vec<MailAttachment> {
  items(value, path)
    .iter()
    .filter(|entry| at(entry, ATTACHMENT_REGULAR).as_u64() == Some(1))
    .filter_map(|entry| {
      Some(MailAttachment {
        name: text(entry, ATTACHMENT_NAME),
        mime: text(entry, ATTACHMENT_MIME),
        size: at(entry, ATTACHMENT_SIZE).as_u64().unwrap_or_default(),
        url: at(entry, ATTACHMENT_URL).as_str()?.to_string(),
      })
    })
    .collect()
}

/// A `bv` answer as threads.
fn thread_page(answer: &Value) -> Result<ThreadPage> {
  let list = at(answer, LIST_THREADS);
  if !list.is_null() && !list.is_array() {
    bail!("Gmail answered a thread list in an unknown shape");
  }
  let threads = items(answer, LIST_THREADS)
    .iter()
    .map(thread_from)
    .collect::<Option<Vec<_>>>()
    .context("Gmail answered a thread in an unknown shape")?;
  let counts = items(answer, LIST_LABEL_COUNTS);
  let unread_of = |label: &str| {
    counts
      .iter()
      .find(|row| row.get(0).and_then(Value::as_str) == Some(label))
      .and_then(|row| row.get(LABEL_COUNT_UNREAD)?.as_u64())
      .map(|count| u32::try_from(count).unwrap_or(u32::MAX))
  };
  let rows = at(answer, LIST_LABELS);
  let labels = rows.as_array().map(|rows| {
    let mut labels: Vec<MailLabel> = rows
      .iter()
      .filter(|row| at(row, LABEL_KIND).as_u64() == Some(LABEL_KIND_USER))
      .filter_map(|row| {
        let id = at(row, LABEL_ID).as_str().filter(|id| id.starts_with(USER_LABEL_PREFIX))?;
        Some(MailLabel {
          id: id.to_string(),
          name: text(row, LABEL_NAME),
          color: at(row, LABEL_BACKGROUND)
            .as_u64()
            .and_then(|color| u32::try_from(color & 0x00ff_ffff).ok()),
          unread: unread_of(id).unwrap_or_default(),
        })
      })
      .collect();
    labels.sort_by_cached_key(|label| label.name.to_lowercase());
    labels
  });
  Ok(ThreadPage {
    threads,
    more: at(answer, LIST_END).as_u64() == Some(LIST_MORE),
    inbox_unread: unread_of(LABEL_INBOX),
    labels,
  })
}

fn thread_from(entry: &Value) -> Option<MailThread> {
  let id = at(entry, THREAD_ID).as_str()?.to_string();
  let messages = at(entry, THREAD_MESSAGES).as_array()?;
  let mut senders: Vec<MailAddress> = Vec::new();
  for sender in messages.iter().filter_map(|message| address(at(message, LISTED_FROM))) {
    if !senders.iter().any(|held| held.email.eq_ignore_ascii_case(&sender.email)) {
      senders.push(sender);
    }
  }
  Some(MailThread {
    id,
    subject: subject(&text(entry, THREAD_SUBJECT)),
    snippet: one_line(&text(entry, THREAD_SNIPPET)),
    senders,
    time: micros(entry, THREAD_TIME),
    messages: u32::try_from(messages.len()).unwrap_or(u32::MAX),
    unread: messages.iter().any(|message| has_label(message, LISTED_LABELS, LABEL_UNREAD)),
    starred: messages.iter().any(|message| has_label(message, LISTED_LABELS, LABEL_STARRED)),
    attachments: messages
      .iter()
      .any(|message| !attachments(message, LISTED_ATTACHMENTS).is_empty()),
  })
}

fn subject(text: &str) -> String {
  let text = one_line(text);
  if text.is_empty() {
    "(no subject)".into()
  } else {
    text
  }
}

fn one_line(text: &str) -> String {
  text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A thread as `fd` answers it, with what a change needs besides the emails.
struct ThreadDetail {
  /// The thread's subject as written (empty when it has none).
  subject: String,
  /// Oldest first.
  messages: Vec<MailMessage>,
  /// Each email's Message-ID header, in the order of `messages`.
  message_ids: Vec<Option<String>>,
}

/// The emails of `thread_id` in an `fd` answer, oldest first.
fn thread_detail(answer: &Value, thread_id: &str) -> Result<ThreadDetail> {
  let entry = items(answer, DETAILS_THREADS)
    .iter()
    .find(|entry| at(entry, ENTRY_ID).as_str() == Some(thread_id))
    .context("Gmail did not answer this thread")?;
  let details: HashMap<&str, &Value> = items(entry, ENTRY_DETAILS)
    .iter()
    .filter_map(|detail| Some((at(detail, DETAIL_ID).as_str()?, at(detail, DETAIL_FIELDS))))
    .collect();
  let (messages, message_ids) = items(entry, ENTRY_SUMMARIES)
    .iter()
    .map(|summary| {
      let id = at(summary, SUMMARY_ID).as_str()?;
      let detail = details.get(id)?;
      let html: String = items(detail, DETAIL_PARTS)
        .iter()
        .filter_map(|part| at(part, PART_HTML).as_str())
        .collect();
      let message = MailMessage {
        id: id.to_string(),
        from: address(at(summary, SUMMARY_FROM))?,
        to: addresses(detail, DETAIL_TO),
        cc: addresses(detail, DETAIL_CC),
        bcc: addresses(detail, DETAIL_BCC),
        reply_to: addresses(detail, DETAIL_REPLY_TO),
        time: micros(summary, SUMMARY_TIME),
        text: html_to_text(&html),
        html,
        unread: has_label(summary, SUMMARY_LABELS, LABEL_UNREAD),
        draft: has_label(summary, SUMMARY_LABELS, LABEL_DRAFT),
        attachments: attachments(detail, DETAIL_ATTACHMENTS),
      };
      Some((message, at(detail, DETAIL_MESSAGE_ID).as_str().map(str::to_string)))
    })
    .collect::<Option<(Vec<_>, Vec<_>)>>()
    .context("Gmail answered an email in an unknown shape")?;
  Ok(ThreadDetail {
    subject: text(entry, ENTRY_SUBJECT),
    messages,
    message_ids,
  })
}

/// An array of `len` nulls with `fields` set at their slot, as Gmail pads its positional arrays.
fn positional(len: usize, fields: &[(usize, Value)]) -> Value {
  let mut array = vec![Value::Null; len];
  for (index, value) in fields {
    if let Some(slot) = array.get_mut(*index) {
      *slot = value.clone();
    }
  }
  Value::Array(array)
}

/// A label change of the emails `ids`. An empty `remove` goes as null, as the web client sends it.
fn label_op(add: &[&str], remove: &[&str], ids: &[String]) -> Value {
  let remove = if remove.is_empty() {
    Value::Null
  } else {
    json!(remove)
  };
  positional(OP_LABELS + 1, &[(OP_LABELS, json!([add, remove, ids]))])
}

/// Starring as Gmail's star does: `^t` on the newest of the emails `ids` (oldest first); unstarring takes it off
/// every one.
fn star_op(ids: &[String], starred: bool) -> Value {
  if starred {
    label_op(&[LABEL_STARRED], &[], ids.last().map_or_default(std::slice::from_ref))
  } else {
    label_op(&[], &[LABEL_STARRED], ids)
  }
}

/// The user's labels `add` put on and `remove` taken off every one of the emails `ids`. `check_user_labels` first.
fn labels_op(add: &[String], remove: &[String], ids: &[String]) -> Value {
  let add: Vec<&str> = add.iter().map(String::as_str).collect();
  let remove: Vec<&str> = remove.iter().map(String::as_str).collect();
  label_op(&add, &remove, ids)
}

/// Checks a label change before anything is sent: something to change, and only the user's own labels (ids `^x_…`),
/// never Gmail's (inbox, starred, read, and the rest). The error is short enough to show.
pub(crate) fn check_user_labels(add: &[String], remove: &[String]) -> Result<()> {
  if add.is_empty() && remove.is_empty() {
    bail!("Name a label to put on or take off.");
  }
  if let Some(wrong) = add
    .iter()
    .chain(remove)
    .find(|id| id.strip_prefix(USER_LABEL_PREFIX).is_none_or(str::is_empty))
  {
    bail!("“{wrong}” is not one of the user's labels: their ids start with {USER_LABEL_PREFIX}.");
  }
  if let Some(both) = add.iter().find(|id| remove.contains(*id)) {
    bail!("“{both}” is both put on and taken off.");
  }
  Ok(())
}

/// What a send needs to fill a message.
struct SentEmail<'a> {
  id: &'a str,
  client: &'a str,
  me: &'a MailAddress,
  to: &'a [MailAddress],
  cc: &'a [MailAddress],
  bcc: &'a [MailAddress],
  subject: &'a str,
  written: String,
  /// The email answered, for a reply; `None` for a new email.
  answering: Option<Answering<'a>>,
  now: u64,
}

/// What a reply adds to its message.
struct Answering<'a> {
  /// The answered email's Message-ID.
  in_reply_to: Option<&'a str>,
  /// Gmail's quote of it, under the written text.
  quoted: String,
}

fn address_value(address: &MailAddress) -> Value {
  if address.name.is_empty() {
    json!([1, address.email])
  } else {
    json!([1, address.email, address.name])
  }
}

/// The user is among `to`, `cc`, and `bcc`.
fn addressed_to(me: &str, to: &[MailAddress], cc: &[MailAddress], bcc: &[MailAddress]) -> bool {
  to.iter()
    .chain(cc)
    .chain(bcc)
    .any(|address| address.email.eq_ignore_ascii_case(me))
}

/// The message of a send, a reply or a new email, as the send op carries it. Mail the user is a recipient of also
/// goes to their inbox, as Gmail keeps only the sent copy of mail to oneself; it is never marked unread (the web
/// client marks a new email to oneself unread, which would count the user's own email as new mail).
fn sent_message(email: &SentEmail) -> Value {
  let me = email.me;
  let mut labels: Vec<&str> = SENT_LABEL_SET.to_vec();
  if addressed_to(&me.email, email.to, email.cc, email.bcc) {
    labels.push(LABEL_INBOX);
  }
  let client_id = format!("{}|#{}|0", email.client, email.id);
  let list = |addresses: &[MailAddress]| Value::Array(addresses.iter().map(address_value).collect());
  let (parts, kind, flags) = email.answering.as_ref().map_or_else(
    || (json!([[PART_WRITTEN, email.written]]), SENT_KIND_NEW, json!([0, 0, 1, null, 0])),
    |answering| {
      (
        json!([[PART_WRITTEN, email.written], [PART_QUOTED, answering.quoted]]),
        SENT_KIND_REPLY,
        json!([0, 0, 0, null, 0]),
      )
    },
  );
  let mut fields = vec![
    (SENT_ID, json!(email.id)),
    (
      SENT_FROM,
      json!([1, me.email, me.name, null, null, null, null, null, null, me.email]),
    ),
    (SENT_TO, list(email.to)),
    (SENT_TIME, json!(email.now)),
    (SENT_SUBJECT, json!(email.subject)),
    (SENT_BODY, json!([null, parts, null, null, null, null, 1])),
    (SENT_LABELS, json!(labels)),
    (SENT_TIME_AGAIN, json!(email.now)),
    (SENT_FLAG, json!(0)),
    (SENT_FLAGS, flags),
    (SENT_CLIENT_ID, json!(client_id)),
    (SENT_KIND, json!(kind)),
    (SENT_CLIENT_ID_AGAIN, json!([null, null, null, null, null, null, client_id])),
    (SENT_EXTRAS, json!([])),
  ];
  if !email.cc.is_empty() {
    fields.push((SENT_CC, list(email.cc)));
  }
  if !email.bcc.is_empty() {
    fields.push((SENT_BCC, list(email.bcc)));
  }
  if let Some(header) = email.answering.as_ref().and_then(|answering| answering.in_reply_to) {
    fields.push((SENT_IN_REPLY_TO, json!(header)));
  }
  positional(SENT_SLOTS, &fields)
}

/// A new email's message as the draft its thread is made with: the draft labels instead of the sent ones, and no
/// send kind.
fn draft_of(message: &Value) -> Value {
  let mut draft = message.clone();
  if let Some(slots) = draft.as_array_mut() {
    if let Some(labels) = slots.get_mut(SENT_LABELS) {
      *labels = json!(DRAFT_LABEL_SET);
    }
    if let Some(kind) = slots.get_mut(SENT_KIND) {
      *kind = Value::Null;
    }
  }
  draft
}

/// An email saved as a draft in Gmail, with what its send needs: the message as the send carries it, its thread, and
/// the email a reply answers. The draft and the sent email are the same message id.
#[derive(Clone, Debug)]
pub(crate) struct Draft {
  /// As `OP_SEND` carries it: the sent labels and kind (`draft_of` gives the saved draft).
  message: Value,
  /// Its thread: a reply's own, a new email's new one.
  pub(crate) thread: String,
  /// The email a reply answers; `None` for a new email.
  answered: Option<String>,
  /// Who it is addressed to.
  pub(crate) to: Vec<MailAddress>,
  /// Who gets a copy.
  pub(crate) cc: Vec<MailAddress>,
  /// Who gets a copy the other recipients do not see.
  pub(crate) bcc: Vec<MailAddress>,
  /// The user is among its recipients, which puts it in their inbox too once sent.
  pub(crate) to_me: bool,
}

impl Draft {
  /// It answers an email of its thread.
  pub(crate) const fn is_reply(&self) -> bool {
    self.answered.is_some()
  }
}

/// `email` as a draft in `thread`, to save and later send. A reply answers the email `answered`. A `signed` new email
/// carries the flags Gmail's client sent its signed one with (its unsigned one `[0, 0, 1, null, 0]`).
fn email_draft(email: &SentEmail, thread: &str, answered: Option<&str>, signed: bool) -> Draft {
  let mut message = sent_message(email);
  if signed
    && answered.is_none()
    && let Some(flags) = message.get_mut(SENT_FLAGS)
  {
    *flags = json!([0, 0, 0, null, 0]);
  }
  Draft {
    message,
    thread: thread.to_string(),
    answered: answered.map(str::to_string),
    to: email.to.to_vec(),
    cc: email.cc.to_vec(),
    bcc: email.bcc.to_vec(),
    to_me: addressed_to(&email.me.email, email.to, email.cc, email.bcc),
  }
}

/// The op that saves `draft` in Gmail: a new email's makes its new thread with it (`OP_CREATE`), a reply's saves it
/// into its thread (`OP_SAVE_DRAFT`).
fn draft_op(draft: &Draft) -> Value {
  let saved = draft_of(&draft.message);
  if draft.is_reply() {
    positional(OP_SAVE_DRAFT + 1, &[(OP_SAVE_DRAFT, json!([saved]))])
  } else {
    let subject = draft.message.get(SENT_SUBJECT).cloned().unwrap_or_default();
    let time = draft.message.get(SENT_TIME).cloned().unwrap_or_default();
    positional(
      OP_CREATE + 1,
      &[(OP_CREATE, json!([[subject, null, time, draft.thread, [saved]]]))],
    )
  }
}

/// The ids of the emails of `thread` in an `fd` answer, oldest first. A thread without emails (an id Gmail does not
/// have) is refused: a label change without ids is answered 0 and changes nothing.
fn message_ids_of(answer: &Value, thread: &str) -> Result<Vec<String>> {
  let ids: Vec<String> = thread_detail(answer, thread)?
    .messages
    .into_iter()
    .map(|message| message.id)
    .collect();
  if ids.is_empty() {
    bail!("Gmail has no such thread");
  }
  Ok(ids)
}

/// The op that sends `draft` as saved: a reply with the email it answers and the time `now` (ms), a new email alone.
fn send_op(draft: &Draft, now: u64) -> Value {
  let payload = draft.answered.as_ref().map_or_else(
    || json!([draft.message, null, 1]),
    |answered| json!([draft.message, answered, 1, null, [now]]),
  );
  positional(OP_SEND + 1, &[(OP_SEND, payload)])
}

/// Checks who a send goes to before anything is sent: someone at all (in `to`, `cc`, or `bcc`), and each a plausible
/// `local@domain`. The error is short enough to show.
pub(crate) fn check_recipients(to: &[MailAddress], cc: &[MailAddress], bcc: &[MailAddress]) -> Result<()> {
  if to.is_empty() && cc.is_empty() && bcc.is_empty() {
    bail!("Add someone to send the email to.");
  }
  if let Some(wrong) = to.iter().chain(cc).chain(bcc).find(|address| !plausible_email(&address.email)) {
    bail!("“{}” is not an email address.", wrong.email.trim());
  }
  Ok(())
}

/// `local@domain`: one `@`, something before it, a domain with a dot inside, and no spaces or brackets.
fn plausible_email(email: &str) -> bool {
  let Some((local, domain)) = email.split_once('@') else {
    return false;
  };
  let clean = |part: &str| {
    !part.is_empty()
      && !part
        .chars()
        .any(|character| character.is_whitespace() || "@<>,;\"()[]".contains(character))
  };
  clean(local)
    && clean(domain)
    && domain
      .split_once('.')
      .is_some_and(|(name, rest)| !name.is_empty() && !rest.is_empty() && !rest.ends_with('.'))
    && !domain.contains("..")
}

/// A reply's subject: the thread's, with `Re: ` before it unless it has one.
fn reply_subject(subject: &str) -> String {
  let subject = subject.trim();
  let replied = subject.get(..3).is_some_and(|start| start.eq_ignore_ascii_case("re:"));
  if replied {
    subject.to_string()
  } else {
    format!("Re: {subject}")
  }
}

/// A forward's subject: the thread's, with `Fwd: ` before it unless it has one.
fn forward_subject(subject: &str) -> String {
  let subject = subject.trim();
  let forwarded = subject.get(..4).is_some_and(|start| start.eq_ignore_ascii_case("fwd:"));
  if forwarded {
    subject.to_string()
  } else {
    format!("Fwd: {subject}")
  }
}

/// `text` with the characters HTML gives meaning to written as references.
fn escape_html(text: &str) -> String {
  let mut out = String::with_capacity(text.len());
  for character in text.chars() {
    match character {
      '&' => out.push_str("&amp;"),
      '<' => out.push_str("&lt;"),
      '>' => out.push_str("&gt;"),
      '"' => out.push_str("&quot;"),
      '\'' => out.push_str("&#39;"),
      other => out.push(other),
    }
  }
  out
}

/// What the user wrote as HTML, as Gmail's compose writes it: each line its own `<div>`, an empty line
/// `<div><br></div>`, all in Gmail's `<div dir="ltr">`, with `signature` (a `signature_block`) after them inside the
/// same element, where Gmail's editor keeps it. Blank lines at the start and the end are dropped.
pub(crate) fn written_html(text: &str, signature: Option<&str>) -> String {
  let text = text.replace("\r\n", "\n");
  let text = text.trim();
  let lines: String = text
    .split('\n')
    .filter(|_| !text.is_empty())
    .map(|line| {
      if line.trim().is_empty() {
        "<div><br></div>".to_string()
      } else {
        format!("<div>{}</div>", escape_html(line))
      }
    })
    .collect();
  format!("<div dir=\"ltr\">{lines}{}</div>", signature.unwrap_or_default())
}

/// Where a signature goes, which decides its markup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SignaturePlace {
  /// Under the text of a new email, inside its written part. Gmail's compose window opens with an empty first line
  /// (`SIGNATURE_BREAK`) above the signature, which the user's text takes the place of (`with_text`).
  NewEmail { with_text: bool },
  /// After the quoted email at the end of a reply, inside the quoted part, as Gmail's reply box leaves it.
  AfterQuote,
  /// Under the text of a reply, above the quote, without the `-- ` line (the "before quoted text" setting).
  BeforeQuote,
}

impl SignaturePlace {
  /// A new email's place, by whether the user wrote anything.
  pub(crate) fn new_email(text: &str) -> Self {
    Self::NewEmail { with_text: !text.trim().is_empty() }
  }
}

/// A signature's HTML as Gmail's client sends it: in a `gmail_signature` element (`dir="rtl"` when the signature is
/// written right to left; a signature that already is such an element gets a new opening tag), after the break and
/// the `-- ` line, or after the break in a `<div>` of its own before a reply's quote. Gmail-sent samples: a new email
/// with text comes back from Gmail's server as `[0 text][3 signature][0 …]` (its reader draws part 3 gray, its own
/// too), a reply keeps the signature in its quoted part.
pub(crate) fn signature_block(html: &str, place: SignaturePlace) -> String {
  static WRAPPED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^<div [^>]*class="gmail_signature"[^>]*>"#).expect("valid pattern"));
  let dir = if html.contains("dir=\"rtl\"") {
    "rtl"
  } else {
    "ltr"
  };
  let smartmail = if matches!(place, SignaturePlace::NewEmail { .. }) {
    SIGNATURE_SMARTMAIL
  } else {
    ""
  };
  let open = format!("<div dir=\"{dir}\" class=\"gmail_signature\"{smartmail}>");
  let wrapped = WRAPPED.find(html).map_or_else(
    || format!("{open}{html}</div>"),
    |found| format!("{open}{}", html.get(found.end()..).unwrap_or_default()),
  );
  match place {
    SignaturePlace::BeforeQuote => format!("{SIGNATURE_BREAK}<div>{wrapped}</div>"),
    SignaturePlace::NewEmail { with_text: true } => format!("{SIGNATURE_PREFIX}{wrapped}"),
    SignaturePlace::NewEmail { with_text: false } | SignaturePlace::AfterQuote => {
      format!("{SIGNATURE_BREAK}{SIGNATURE_PREFIX}{wrapped}")
    },
  }
}

/// A reply's two body parts, the written text and Gmail's quote, with `signature` where the settings put it: under
/// the written text, above the quote, when `before_quote`; else after the quote, as Gmail's reply box leaves it.
fn reply_body(text: &str, quoted: String, signature: Option<&MailSignature>, before_quote: bool) -> (String, String) {
  match signature {
    Some(signature) if before_quote => (
      written_html(text, Some(&signature_block(&signature.html, SignaturePlace::BeforeQuote))),
      quoted,
    ),
    Some(signature) => (
      written_html(text, None),
      quoted + &signature_block(&signature.html, SignaturePlace::AfterQuote),
    ),
    None => (written_html(text, None), quoted),
  }
}

/// When an email came, as Gmail writes it above a quote or in a forward (`Thu, Nov 19, 2026 at 3:28 PM`), in local
/// time.
fn gmail_date(time: Micros) -> String {
  chrono::DateTime::from_timestamp_micros(time)
    .unwrap_or_default()
    .with_timezone(&chrono::Local)
    .format("%a, %b %-d, %Y at %-I:%M %p")
    .to_string()
}

/// Gmail's quote of the answered email: `On <date> <sender> wrote:` and a blockquote of its HTML.
fn quoted_html(answered: &MailMessage) -> String {
  let when = gmail_date(answered.time);
  let from = &answered.from;
  let sender = if from.name.is_empty() {
    format!("&lt;{}&gt;", escape_html(&from.email))
  } else {
    format!("{} &lt;{}&gt;", escape_html(&from.name), escape_html(&from.email))
  };
  format!(
    "<br><div class=\"gmail_quote gmail_quote_container\"><div dir=\"ltr\" class=\"gmail_attr\">On {when} {sender} \
     wrote:<br></div><blockquote class=\"gmail_quote\" style=\"margin:0px 0px 0px 0.8ex;border-left:1px solid \
     rgb(204,204,204);padding-left:1ex\">{}</blockquote></div>",
    answered.html
  )
}

/// Gmail's forwarded block, as the web client's compose writes it under a forward's text: the `Forwarded message`
/// line, who wrote the email, when, `subject`, who it went to and (when anyone) who got a copy, then its HTML.
fn forwarded_html(message: &MailMessage, subject: &str) -> String {
  let from = &message.from;
  let email = escape_html(&from.email);
  let address = format!("<span dir=\"auto\">&lt;<a href=\"mailto:{email}\">{email}</a>&gt;</span>");
  let sender = if from.name.is_empty() {
    address
  } else {
    format!(
      "<strong class=\"gmail_sendername\" dir=\"auto\">{}</strong> {address}",
      escape_html(&from.name)
    )
  };
  let cc = if message.cc.is_empty() {
    String::new()
  } else {
    format!("Cc: {}<br>", forwarded_addresses(&message.cc))
  };
  format!(
    "<div class=\"gmail_quote gmail_quote_container\"><div dir=\"ltr\" class=\"gmail_attr\">---------- Forwarded \
     message ---------<br>From: {sender}<br>Date: {}<br>Subject: {}<br>To: {}<br>{cc}</div><br><br>{}</div>",
    gmail_date(message.time),
    escape_html(subject),
    forwarded_addresses(&message.to),
    message.html
  )
}

/// Addresses in a forwarded block's To and Cc lines: `Name <address>` with the address a `mailto:` link, comma
/// separated.
fn forwarded_addresses(addresses: &[MailAddress]) -> String {
  addresses
    .iter()
    .map(|address| {
      let email = escape_html(&address.email);
      let link = format!("&lt;<a href=\"mailto:{email}\">{email}</a>&gt;");
      if address.name.is_empty() {
        link
      } else {
        format!("{} {link}", escape_html(&address.name))
      }
    })
    .collect::<Vec<_>>()
    .join(", ")
}

/// Who a reply to `message` goes to, as Gmail's Reply (`all` false) and Reply all do.
///
/// Reply goes to its Reply-To, else its sender, else (when the user sent it) its recipients; Reply all adds its other
/// recipients to `to` and its copies to `cc`. The user's own address (`me`) is left out, unless it is the only one
/// there is (a thread with oneself), and nobody is named twice. Addresses compare without regard to case.
pub fn reply_recipients(message: &MailMessage, me: &str, all: bool) -> (Vec<MailAddress>, Vec<MailAddress>) {
  let mine = |address: &MailAddress| address.email.eq_ignore_ascii_case(me);
  let primary: Vec<MailAddress> = if !message.reply_to.is_empty() {
    message.reply_to.clone()
  } else if mine(&message.from) {
    message.to.clone()
  } else {
    vec![message.from.clone()]
  };
  let mut to: Vec<MailAddress> = Vec::new();
  let mut cc: Vec<MailAddress> = Vec::new();
  let named = |list: &[MailAddress], address: &MailAddress| {
    list.iter().any(|held| held.email.eq_ignore_ascii_case(&address.email))
  };
  let extra: &[MailAddress] = if all { &message.to } else { &[] };
  for address in primary.iter().chain(extra) {
    if !mine(address) && !named(&to, address) {
      to.push(address.clone());
    }
  }
  if all {
    for address in &message.cc {
      if !mine(address) && !named(&to, address) && !named(&cc, address) {
        cc.push(address.clone());
      }
    }
  }
  if to.is_empty() && cc.is_empty() {
    // Only the user is there: a thread with oneself answers to oneself.
    to = primary.into_iter().take(1).collect();
  }
  (to, cc)
}

/// The emails of `thread_id` in an `fd` answer, oldest first.
#[cfg(test)]
fn messages_from(answer: &Value, thread_id: &str) -> Result<Vec<MailMessage>> {
  Ok(thread_detail(answer, thread_id)?.messages)
}

/// Marks a block boundary (`div`, `li`, `tr`): what follows starts a line.
const BLOCK: char = '\u{1}';
/// Marks a paragraph boundary (`p`, lists, quotes, headings): a blank line around it.
const PARAGRAPH: char = '\u{2}';

/// An email's HTML as plain text: what a person reads, block elements on their own lines, at most one blank line
/// in a row.
pub(crate) fn html_to_text(html: &str) -> String {
  static HIDDEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
      r"(?is)<head\b.*?</head\s*>|<style\b.*?</style\s*>|<script\b.*?</script\s*>|<title\b.*?</title\s*>|<!--.*?-->",
    )
    .expect("valid pattern")
  });
  static SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t\r\n]+").expect("valid pattern"));
  static LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\b[^>]*>").expect("valid pattern"));
  static BLOCKS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)</?(?:div|li|tr|hr|pre)\b[^>]*>").expect("valid pattern"));
  static PARAGRAPHS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)</?(?:p|ul|ol|table|blockquote|h[1-6])\b[^>]*>").expect("valid pattern"));
  static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").expect("valid pattern"));
  let text = HIDDEN.replace_all(html, "");
  let text = SPACE.replace_all(&text, " ");
  let text = LINE.replace_all(&text, "\n");
  let text = BLOCKS.replace_all(&text, BLOCK.to_string());
  let text = PARAGRAPHS.replace_all(&text, PARAGRAPH.to_string());
  let text = TAG.replace_all(&text, "");
  let text = decode_entities(&text);
  let mut broken = String::with_capacity(text.len());
  for found in text.chars() {
    match found {
      BLOCK | PARAGRAPH => {
        broken.truncate(broken.trim_end_matches(' ').len());
        if broken.is_empty() {
          continue;
        }
        let wanted = if found == BLOCK { "\n" } else { "\n\n" };
        while !broken.ends_with(wanted) {
          broken.push('\n');
        }
      },
      found => broken.push(found),
    }
  }
  let mut out = String::with_capacity(broken.len());
  let mut blank = true;
  for line in broken.lines().map(one_line) {
    if line.is_empty() {
      if !blank {
        out.push('\n');
        blank = true;
      }
      continue;
    }
    out.push_str(&line);
    out.push('\n');
    blank = false;
  }
  out.trim_end().to_string()
}

/// An email's text without the earlier emails it quotes (Gmail's `gmail_quote` blocks, and `<blockquote type="cite">`
/// as other mail clients write them), the rest read by `html_to_text`. `None` when it quotes nothing, or when the
/// quote is all it has: the caller keeps the whole text then.
pub(crate) fn html_to_text_unquoted(html: &str) -> Option<String> {
  static TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<(/?)(div|blockquote)\b([^>]*)>").expect("valid pattern"));
  static QUOTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\bclass\s*=\s*["'][^"']*\bgmail_quote|\btype\s*=\s*["']?cite\b"#).expect("valid pattern")
  });
  let mut kept = String::with_capacity(html.len());
  let mut from = 0;
  // The quote being skipped: its tag's name and how many tags of that name are open in it.
  let mut open: Option<(String, usize)> = None;
  let mut quoted = false;
  for tag in TAG.captures_iter(html) {
    let (Some(whole), Some(name)) = (tag.get(0), tag.get(2)) else {
      continue;
    };
    let closing = tag.get(1).is_some_and(|slash| !slash.as_str().is_empty());
    let name = name.as_str().to_ascii_lowercase();
    if let Some((quote, depth)) = open.as_mut() {
      if *quote != name {
        continue;
      }
      if closing {
        *depth = depth.saturating_sub(1);
      } else {
        *depth += 1;
      }
      if *depth == 0 {
        open = None;
        from = whole.end();
      }
    } else if !closing && QUOTE.is_match(tag.get(3).map_or("", |attributes| attributes.as_str())) {
      kept.push_str(html.get(from..whole.start()).unwrap_or_default());
      open = Some((name, 1));
      quoted = true;
    }
  }
  if !quoted {
    return None;
  }
  if open.is_none() {
    kept.push_str(html.get(from..).unwrap_or_default());
  }
  let text = html_to_text(&kept);
  (!text.is_empty()).then_some(text)
}

/// `text` with HTML character references replaced; an unknown one stays as written.
fn decode_entities(text: &str) -> String {
  let mut parts = text.split('&');
  let mut out = parts.next().unwrap_or_default().to_string();
  for part in parts {
    let (decoded, rest) = part
      .split_once(';')
      .and_then(|(name, rest)| Some((entity(name)?, rest)))
      .unwrap_or_else(|| ("&".to_string(), part));
    out.push_str(&decoded);
    out.push_str(rest);
  }
  out
}

fn entity(name: &str) -> Option<String> {
  if let Some(number) = name.strip_prefix('#') {
    let code = match number.strip_prefix(['x', 'X']) {
      Some(hex) => u32::from_str_radix(hex, 16).ok()?,
      None => number.parse().ok()?,
    };
    return char::from_u32(code).map(|found| match found {
      '\u{a0}' => " ".into(),
      found if found.is_control() && found != '\n' && found != '\t' => String::new(),
      found => found.to_string(),
    });
  }
  let decoded = match name {
    "amp" => "&",
    "lt" => "<",
    "gt" => ">",
    "quot" => "\"",
    "apos" => "'",
    "nbsp" | "ensp" | "emsp" | "thinsp" => " ",
    "zwnj" | "zwj" | "shy" => "",
    "ndash" => "–",
    "mdash" => "—",
    "hellip" => "…",
    "lsquo" => "‘",
    "rsquo" => "’",
    "ldquo" => "“",
    "rdquo" => "”",
    "laquo" => "«",
    "raquo" => "»",
    "bull" => "•",
    "middot" => "·",
    "copy" => "©",
    "reg" => "®",
    "trade" => "™",
    "euro" => "€",
    _ => return None,
  };
  Some(decoded.to_string())
}

/// The inbox's first page read again while older pages are held: the fresh threads, then the held ones older than
/// all of them. A `complete` page is the whole mailbox and replaces what is held.
pub(crate) fn merge_newest(held: &[MailThread], fresh: Vec<MailThread>, complete: bool) -> Vec<MailThread> {
  let Some(oldest) = fresh.iter().map(|thread| thread.time).min().filter(|_| !complete) else {
    return fresh;
  };
  let older: Vec<MailThread> = held
    .iter()
    .filter(|thread| thread.time < oldest && !fresh.iter().any(|newer| newer.id == thread.id))
    .cloned()
    .collect();
  let mut merged = fresh;
  merged.extend(older);
  merged
}

/// Adds an older page to the threads held, leaving out the ones held already (later pages overlap).
pub(crate) fn append_older(held: &mut Vec<MailThread>, older: Vec<MailThread>) {
  let ids: std::collections::HashSet<String> = held.iter().map(|thread| thread.id.clone()).collect();
  held.extend(older.into_iter().filter(|thread| !ids.contains(&thread.id)));
}

/// How long a written change still corrects list answers asked for before it was written.
const CHANGE_KEPT: Duration = Duration::from_secs(120);

/// One thread's change the user made: what it is now, and when Gmail had it.
#[derive(Debug, Default)]
struct Change {
  unread: Option<bool>,
  in_inbox: Option<bool>,
  starred: Option<bool>,
  /// The tick and time it was written; `None` while it is on its way.
  written: Option<(u64, std::time::Instant)>,
}

/// The user's changes to threads (read, unread, archived, starred), laid over list answers Gmail may have made before
/// it had them: an answer asked for before a change was written still shows the thread as it was, and its counts
/// too.
#[derive(Debug, Default)]
pub(crate) struct Changes {
  /// Counts list requests and written changes, in the order they happen.
  tick: u64,
  held: HashMap<String, Change>,
}

impl Changes {
  /// A list request starts: its tick, which `apply` takes with its answer.
  pub(crate) const fn request(&mut self) -> u64 {
    self.tick = self.tick.saturating_add(1);
    self.tick
  }

  /// The user changed `thread`: read or unread, in or out of the inbox.
  pub(crate) fn note(&mut self, thread: &str, unread: Option<bool>, in_inbox: Option<bool>) {
    let change = self.held.entry(thread.to_string()).or_default();
    change.unread = unread.or(change.unread);
    change.in_inbox = in_inbox.or(change.in_inbox);
    change.written = None;
  }

  /// The user starred or unstarred `thread`.
  pub(crate) fn note_starred(&mut self, thread: &str, starred: bool) {
    let change = self.held.entry(thread.to_string()).or_default();
    change.starred = Some(starred);
    change.written = None;
  }

  /// Gmail has the change of `thread`.
  pub(crate) fn written(&mut self, thread: &str) {
    self.tick = self.tick.saturating_add(1);
    if let Some(change) = self.held.get_mut(thread) {
      change.written = Some((self.tick, std::time::Instant::now()));
    }
  }

  /// Gmail did not take the change of `thread`: answers show it as Gmail has it.
  pub(crate) fn forget(&mut self, thread: &str) {
    self.held.remove(thread);
  }

  /// Lays the changes over `page`, the answer to the request of tick `requested`: a change on its way, or written
  /// after the request went out, shows in its threads (an archived thread leaves an inbox list), and the inbox count
  /// moves to match. A count the answer cannot vouch for (any other list's, or labels') is left out.
  pub(crate) fn apply(&mut self, page: &mut ThreadPage, inbox: bool, requested: u64) {
    self
      .held
      .retain(|_, change| change.written.is_none_or(|(_, at)| at.elapsed() < CHANGE_KEPT));
    let stale = |change: &Change| change.written.is_none_or(|(tick, _)| tick > requested);
    if !self.held.values().any(stale) {
      return;
    }
    page.labels = None;
    if !inbox {
      page.inbox_unread = None;
    }
    let mut more: i64 = 0;
    page.threads.retain_mut(|thread| {
      let Some(change) = self.held.get(&thread.id).filter(|change| stale(change)) else {
        return true;
      };
      let was_unread = thread.unread;
      if inbox && change.in_inbox == Some(false) {
        more -= i64::from(was_unread);
        return false;
      }
      if let Some(unread) = change.unread {
        thread.unread = unread;
        more += i64::from(unread) - i64::from(was_unread);
      }
      if let Some(starred) = change.starred {
        thread.starred = starred;
      }
      true
    });
    if inbox {
      page.inbox_unread = page
        .inbox_unread
        .map(|count| u32::try_from((i64::from(count) + more).max(0)).unwrap_or(count));
    }
  }
}

/// Threads of `told` kept before the ones no longer in the inbox are forgotten.
const TOLD_HELD: usize = 1000;

/// Which inbox threads are new mail: unread, newer than anything the first load saw, and newer than what was last
/// told of the same thread.
#[derive(Debug, Default)]
pub(crate) struct Arrivals {
  /// Newest thread time of the first load.
  seen_until: Option<Micros>,
  /// Per thread, the time last told of.
  told: HashMap<String, Micros>,
}

impl Arrivals {
  /// The user wrote into `thread`: it does not count as new mail up to `until`.
  pub(crate) fn note_own(&mut self, thread: &str, until: Micros) {
    let told = self.told.entry(thread.to_string()).or_insert(until);
    *told = (*told).max(until);
  }

  /// The threads of `inbox` (its newest page) that arrived since the last call. The first call only takes note of
  /// what is there; `now` stands for the newest time when the inbox is empty.
  pub(crate) fn arrived(&mut self, inbox: &[MailThread], now: Micros) -> Vec<MailThread> {
    let Some(seen_until) = self.seen_until else {
      self.seen_until = Some(inbox.iter().map(|thread| thread.time).max().unwrap_or(now));
      return Vec::new();
    };
    let arrived: Vec<MailThread> = inbox
      .iter()
      .filter(|thread| {
        thread.unread && thread.time > seen_until && self.told.get(&thread.id).is_none_or(|&told| thread.time > told)
      })
      .cloned()
      .collect();
    for thread in &arrived {
      self.told.insert(thread.id.clone(), thread.time);
    }
    if self.told.len() > TOLD_HELD {
      self.told.retain(|id, _| inbox.iter().any(|thread| &thread.id == id));
    }
    arrived
  }
}

/// A URL only Gmail's own cookies open: an attachment, or an image inside an email.
pub(crate) fn is_mail_url(url: &str) -> bool {
  reqwest::Url::parse(url).is_ok_and(|url| url.host_str() == Some("mail.google.com"))
}

/// The bytes start like a picture, so they belong in the image cache; other attachments stay out of it.
pub(crate) fn looks_like_image(bytes: &[u8]) -> bool {
  let at = |range: std::ops::Range<usize>| bytes.get(range).unwrap_or_default();
  bytes.starts_with(b"\x89PNG")
    || bytes.starts_with(&[0xff, 0xd8, 0xff])
    || bytes.starts_with(b"GIF8")
    || bytes.starts_with(b"BM")
    || bytes.starts_with(b"II*\0")
    || bytes.starts_with(b"MM\0*")
    || (at(0..4) == b"RIFF" && at(8..12) == b"WEBP")
    || (at(4..8) == b"ftyp" && [&b"avif"[..], b"avis", b"heic", b"heix", b"mif1"].contains(&at(8..12)))
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  /// An array of `len` nulls with `fields` set at their index, as Gmail pads its positional arrays.
  fn slots(len: usize, fields: &[(usize, Value)]) -> Value {
    let mut array = vec![Value::Null; len];
    for (index, value) in fields {
      if let Some(slot) = array.get_mut(*index) {
        *slot = value.clone();
      }
    }
    Value::Array(array)
  }

  fn listed_message(id: &str, from: Value, labels: &[&str], attachments: Value) -> Value {
    slots(
      90,
      &[
        (0, json!(id)),
        (1, from),
        (6, json!(1_794_896_454_636_i64)),
        (9, json!("Snippet")),
        (10, json!(labels)),
        (11, attachments),
      ],
    )
  }

  fn attachment(regular: u8, name: &str, mime: &str, size: u64) -> Value {
    json!([
      [
        regular,
        "0.1",
        1,
        [
          u8::from(mime.starts_with("image/")),
          "https://mail.google.com/mail/u/0/?ui=2&ik=abc&attid=0.1&th=thread-f:1&view=att&disp=safe",
          name,
          mime,
          size,
          ["a", "b", null, "c"]
        ]
      ],
      null,
      "",
      null,
      ["x", size, 1, 2, 3, "y"]
    ])
  }

  fn listed_thread(id: &str, subject: &str, snippet: &str, time: i64, messages: Value) -> Value {
    json!([
      slots(
        25,
        &[
          (0, json!(subject)),
          (1, json!(snippet)),
          (2, json!(time)),
          (3, json!(id)),
          (4, messages),
          (24, json!([[0], "en", "en"])),
        ]
      ),
      5_131_319
    ])
  }

  fn list_answer(threads: Value, end: u64) -> Value {
    slots(
      19,
      &[
        (0, json!(0)),
        (2, threads),
        (3, json!(1)),
        (4, json!(5_131_360)),
        (6, json!([[["^all", 2054, 7122], ["^i", 2, 3], ["^t", 0, 1]]])),
        (11, json!([end, 42])),
        (18, json!([5_131_360])),
      ],
    )
  }

  #[test]
  fn thread_list_reads_fields_flags_and_counts() {
    let jordan = json!([1, "jpark@example.com", "Jordan Park"]);
    let riley = json!([1, "rbrooks@example.com"]);
    let answer = list_answer(
      json!([
        listed_thread(
          "thread-f:1",
          "Lunch on Friday",
          "Want to  grab\nlunch?",
          1_794_896_454_636,
          json!([
            listed_message("msg-f:1", jordan, &["^all", "^i"], Value::Null),
            listed_message(
              "msg-f:2",
              riley.clone(),
              &["^all", "^u", "^t"],
              json!([attachment(1, "menu.pdf", "application/pdf", 73_283)])
            ),
            listed_message(
              "msg-f:3",
              json!([1, "JPARK@example.com", "Jordan"]),
              &["^all", "^i"],
              Value::Null
            ),
          ])
        ),
        listed_thread(
          "thread-f:2",
          "",
          "Photo",
          1_794_000_000_000,
          json!([listed_message(
            "msg-f:4",
            riley,
            &["^all", "^i", "^fhas"],
            json!([attachment(0, "image.png", "image/png", 10)])
          )])
        ),
      ]),
      1,
    );
    let page = thread_page(&answer).expect("parses");
    assert!(page.more);
    assert_eq!(page.inbox_unread, Some(2));
    let [first, second] = page.threads.as_slice() else {
      panic!("two threads");
    };
    assert_eq!(first.id, "thread-f:1");
    assert_eq!(first.subject, "Lunch on Friday");
    assert_eq!(first.snippet, "Want to grab lunch?");
    assert_eq!(first.time, 1_794_896_454_636_000);
    assert_eq!(first.messages, 3);
    assert_eq!(
      first.senders,
      vec![
        MailAddress {
          name: "Jordan Park".into(),
          email: "jpark@example.com".into()
        },
        MailAddress {
          name: String::new(),
          email: "rbrooks@example.com".into()
        },
      ]
    );
    assert!(first.unread && first.starred && first.attachments);
    assert_eq!(second.subject, "(no subject)");
    assert!(!second.unread && !second.starred);
    assert!(!second.attachments, "an image inside the HTML is not a file");
  }

  #[test]
  fn thread_list_end_and_empty() {
    let page = thread_page(&list_answer(Value::Null, 2)).expect("parses");
    assert_eq!(
      page,
      ThreadPage {
        threads: Vec::new(),
        more: false,
        inbox_unread: Some(2),
        labels: None,
      }
    );
  }

  #[test]
  fn thread_list_rejects_an_unknown_thread_shape() {
    let answer = list_answer(json!([[slots(5, &[(0, json!("Subject"))]), 1]]), 2);
    assert!(thread_page(&answer).is_err());
  }

  /// A label row as `bv` answers it: `[[id, name, …, colors at 14, …, kind at 17, …], n]`.
  fn label_row(id: &str, name: &str, colors: Value, kind: u64) -> Value {
    json!([
      slots(
        28,
        &[
          (0, json!(id)),
          (1, json!(name)),
          (6, json!(1)),
          (14, colors),
          (17, json!(kind)),
          (27, json!(kind))
        ]
      ),
      827_189
    ])
  }

  #[test]
  fn labels_are_the_users_own_by_name_with_color_and_unread() {
    let mut answer = list_answer(Value::Null, 2);
    answer[1] = json!([
      label_row("^i", "^i", Value::Null, 5),
      label_row("^smartlabel_social", "^smartlabel_social", json!([16_777_215, 1_484_645]), 1),
      label_row("^x_8419", "Receipts", Value::Null, 2),
      label_row("^x_2041486528587302235", "Clients/Acme", json!([16_777_215, 4_818_663]), 2),
      label_row("^x_30", "clients", json!([16_770_759, 13_384_225]), 2),
    ]);
    answer[6] = json!([[["^i", 2, 3], ["^x_2041486528587302235", 4, 9], ["^x_30", 0, 1]]]);
    let labels = thread_page(&answer).expect("parses").labels.expect("labels");
    assert_eq!(
      labels,
      vec![
        MailLabel {
          id: "^x_30".into(),
          name: "clients".into(),
          color: Some(0x00cc_3a21),
          unread: 0,
        },
        MailLabel {
          id: "^x_2041486528587302235".into(),
          name: "Clients/Acme".into(),
          color: Some(0x0049_86e7),
          unread: 4,
        },
        MailLabel {
          id: "^x_8419".into(),
          name: "Receipts".into(),
          color: None,
          unread: 0,
        },
      ]
    );
    assert_eq!(query(&Mailbox::Label("^x_8419".into())), "in:^x_8419");
  }

  #[test]
  fn sender_names_lose_wrapping_quotes() {
    assert_eq!(clean_name("\"Jordan Park\""), "Jordan Park");
    assert_eq!(
      clean_name("'Jordan Park at jpark@example.com' via Team"),
      "Jordan Park at jpark@example.com via Team"
    );
    assert_eq!(clean_name("'Riley Brooks"), "Riley Brooks");
    assert_eq!(clean_name("  Riley Brooks  "), "Riley Brooks");
    assert_eq!(clean_name("Riley O'Brooks"), "Riley O'Brooks");
    assert_eq!(clean_name("'Riley O'Brooks'"), "Riley O'Brooks");
    assert_eq!(clean_name(""), "");
    let sender = address(&json!([1, "jpark@example.com", " \"Jordan Park\" "])).expect("address");
    assert_eq!(sender.name, "Jordan Park");
  }

  fn details_answer() -> Value {
    let summary = |id: &str, from: Value, time: i64, labels: &[&str]| {
      slots(
        60,
        &[
          (0, json!(id)),
          (1, from),
          (2, json!(time)),
          (3, json!(labels)),
          (6, json!(time + 60)),
          (10, json!("Snippet")),
        ],
      )
    };
    let detail = |to: Value, cc: Value, parts: Value, files: Value| {
      slots(
        55,
        &[
          (0, to),
          (1, cc),
          (3, json!([[1, "team-replies@example.com", "Team"]])),
          (4, json!("Lunch on Friday")),
          (7, json!("<lunch-1@mail.example.com>")),
          (5, json!([null, parts, 0, null, null, null, 1, 0])),
          (6, json!("Snippet")),
          (13, files),
          (16, json!(1)),
        ],
      )
    };
    json!([
      0,
      [
        ["thread-f:9", [["thread-f:9", "Other", "Other", 1], []], [], null, 1],
        [
          "thread-f:1",
          [
            slots(
              21,
              &[
                (0, json!("thread-f:1")),
                (1, json!("Lunch on Friday")),
                (2, json!("Sure")),
                (3, json!(2))
              ]
            ),
            [
              summary(
                "msg-f:1",
                json!([1, "jpark@example.com", "Jordan Park"]),
                1_000,
                &["^all", "^i"]
              ),
              summary(
                "msg-f:2",
                json!([1, "rbrooks@example.com", "Riley Brooks"]),
                2_000,
                &["^all", "^i", "^u"]
              ),
            ]
          ],
          [
            [
              "msg-f:1",
              detail(
                json!([[1, "rbrooks@example.com", "Riley Brooks"]]),
                json!([[1, "team@example.com"]]),
                json!([
                  [
                    0,
                    null,
                    [null, "<div dir=\"ltr\">Want to grab lunch?&nbsp;Noon&#39;s fine.</div>"],
                    1
                  ],
                  [3, null, [null, "<div>-- <br>Jordan</div>"], 2]
                ]),
                json!([
                  attachment(1, "menu.pdf", "application/pdf", 73_283),
                  attachment(0, "logo.png", "image/png", 5)
                ])
              )
            ],
            [
              "msg-f:2",
              detail(
                json!([[1, "jpark@example.com", "Jordan Park"]]),
                Value::Null,
                Value::Null,
                Value::Null
              )
            ]
          ],
          null,
          5_130_107
        ]
      ],
      null,
      null,
      [5_131_202]
    ])
  }

  #[test]
  fn thread_emails_read_addresses_body_and_files() {
    let messages = messages_from(&details_answer(), "thread-f:1").expect("parses");
    let [first, second] = messages.as_slice() else {
      panic!("two emails");
    };
    assert_eq!(first.id, "msg-f:1");
    assert_eq!(first.from.name, "Jordan Park");
    assert_eq!(
      first.to,
      vec![MailAddress {
        name: "Riley Brooks".into(),
        email: "rbrooks@example.com".into()
      }]
    );
    assert_eq!(
      first.cc,
      vec![MailAddress {
        name: String::new(),
        email: "team@example.com".into()
      }]
    );
    assert_eq!(first.time, 1_000_000);
    assert_eq!(
      first.reply_to,
      vec![MailAddress {
        name: "Team".into(),
        email: "team-replies@example.com".into()
      }]
    );
    let detail = thread_detail(&details_answer(), "thread-f:1").expect("parses");
    assert_eq!(detail.subject, "Lunch on Friday");
    assert_eq!(detail.message_ids, vec![Some("<lunch-1@mail.example.com>".to_string()); 2]);
    assert!(!first.unread);
    assert!(first.html.starts_with("<div dir=\"ltr\">Want") && first.html.ends_with("Jordan</div>"));
    assert_eq!(first.text, "Want to grab lunch? Noon's fine.\n--\nJordan");
    assert_eq!(
      first.attachments,
      vec![MailAttachment {
        name: "menu.pdf".into(),
        mime: "application/pdf".into(),
        size: 73_283,
        url: "https://mail.google.com/mail/u/0/?ui=2&ik=abc&attid=0.1&th=thread-f:1&view=att&disp=safe".into(),
      }]
    );
    assert_eq!(second.from.email, "rbrooks@example.com");
    assert!(second.unread);
    assert!(second.html.is_empty() && second.text.is_empty() && second.cc.is_empty());
  }

  #[test]
  fn thread_emails_need_the_asked_thread() {
    assert!(messages_from(&details_answer(), "thread-f:404").is_err());
  }

  #[test]
  fn a_change_needs_a_thread_with_emails() {
    let ids = message_ids_of(&details_answer(), "thread-f:1").expect("two emails");
    assert_eq!(ids, ["msg-f:1", "msg-f:2"]);
    // Gmail answers a thread it does not have without emails; a label change without ids would answer 0.
    let none = message_ids_of(&details_answer(), "thread-f:9").map_err(|error| error.to_string());
    assert_eq!(none, Err("Gmail has no such thread".into()));
    assert!(message_ids_of(&details_answer(), "thread-f:404").is_err());
  }

  #[test]
  fn an_email_with_the_draft_label_is_a_draft() {
    let mut answer = details_answer();
    let labels = answer.pointer_mut("/1/1/1/1/1/3").expect("the second email's labels");
    *labels = json!(["^all", "^r", "^r_bt"]);
    let messages = messages_from(&answer, "thread-f:1").expect("parses");
    let [first, second] = messages.as_slice() else {
      panic!("two emails");
    };
    assert!(!first.draft);
    assert!(second.draft && !second.unread);
  }

  #[test]
  fn the_users_own_email_carries_its_bcc() {
    let mut answer = details_answer();
    let bcc = answer.pointer_mut("/1/1/2/0/1/2").expect("the first email's bcc");
    *bcc = json!([[1, "rbrooks@example.com", "Riley Brooks"]]);
    let messages = messages_from(&answer, "thread-f:1").expect("parses");
    let [first, second] = messages.as_slice() else {
      panic!("two emails");
    };
    assert_eq!(first.bcc, vec![person("Riley Brooks", "rbrooks@example.com")]);
    assert_eq!(second.bcc, Vec::new());
  }

  #[test]
  fn html_becomes_readable_text() {
    let html = "<html><head><style>p { color: red }</style><title>T</title></head><body>\
                <p>Hi Riley,</p>\n<p>The   plan:<br>one &amp; two &lt;three&gt; &#x2014; &euro;5 &bogus; a&b</p>\
                <!-- hidden --><script>alert(1)</script><div><br></div><div><br></div><ul><li>first</li><li>second</li></ul>\
                <blockquote class=\"gmail_quote\">quoted</blockquote></body></html>";
    assert_eq!(
      html_to_text(html),
      "Hi Riley,\n\nThe plan:\none & two <three> — €5 &bogus; a&b\n\nfirst\nsecond\n\nquoted"
    );
    assert_eq!(html_to_text("Plain text mail"), "Plain text mail");
  }

  #[test]
  fn quoted_emails_are_left_out() {
    let gmail = "<div dir=\"ltr\">Sounds good.</div><br><div class=\"gmail_quote gmail_quote_container\">\
                 <div dir=\"ltr\" class=\"gmail_attr\">On Tue, Jordan Park &lt;jpark@example.com&gt; wrote:<br></div>\
                 <blockquote class=\"gmail_quote\" style=\"margin:0\"><div dir=\"ltr\"><div>Noon?</div></div>\
                 </blockquote></div>";
    assert_eq!(html_to_text_unquoted(gmail).as_deref(), Some("Sounds good."));
    let cited = "<div>Yes.</div><BLOCKQUOTE type=cite><div>Earlier</div><blockquote type=\"cite\">Older</blockquote>\
                 <div>still quoted</div></BLOCKQUOTE><div>After</div>";
    assert_eq!(html_to_text_unquoted(cited).as_deref(), Some("Yes.\nAfter"));
    let unclosed = "<div>Top</div><div class=\"gmail_quote\"><div>cut off";
    assert_eq!(html_to_text_unquoted(unclosed).as_deref(), Some("Top"));
    // Nothing quoted, or nothing but the quote: the caller keeps the whole text.
    assert_eq!(html_to_text_unquoted("<div>Plain</div><blockquote>said</blockquote>"), None);
    assert_eq!(
      html_to_text_unquoted("<blockquote type=\"cite\"><div>Only this</div></blockquote>"),
      None
    );
  }

  #[test]
  fn page_config_reads_tokens_and_globals() {
    let page = r#"<script>_GM_setData({"a6jdv":[1,"x"]});</script><script nonce="n">_GM_setData({"w43KIf":["sdpc","AKtoken:1795000000000","0a1b2c3d4e",null],"Cl6csf":{"k":"});"}});</script>
      <script>_GM_setData({"sBEv4c":[0,0,0,0,0,0,[0,[["jpark@example.com",null,null,null,"Jordan Park"]]]]});</script>
      <script>var GLOBALS=[null,null,1021169763,"gmail.pinto-server_20261110.06_p0","jsver",null,null,null,null,null,"other@example.com"];</script>"#;
    let config = PageConfig::parse(page).expect("parses");
    assert_eq!(
      config,
      PageConfig {
        xsrf: "AKtoken:1795000000000".into(),
        id_key: "0a1b2c3d4e".into(),
        globals_number: json!(1_021_169_763),
        build: "gmail.pinto-server_20261110.06_p0".into(),
        me: Some(MailAddress {
          name: "Jordan Park".into(),
          email: "jpark@example.com".into()
        }),
        signatures: Signatures::default(),
      }
    );
    let without_identity = page.replace("sBEv4c", "other");
    assert_eq!(
      PageConfig::parse(&without_identity).expect("parses").me,
      Some(MailAddress {
        name: String::new(),
        email: "other@example.com".into()
      })
    );
    let btai = config.btai("Agent/1", -18_000_000, "America/New_York");
    assert_eq!(btai.pointer(BTAI_USER_AGENT), Some(&json!("Agent/1")));
    assert_eq!(btai.pointer(BTAI_ID_KEY), Some(&json!("0a1b2c3d4e")));
    assert_eq!(btai.pointer(BTAI_BUILD), Some(&json!("gmail.pinto-server_20261110.06_p0")));
    assert_eq!(btai.pointer(BTAI_OFFSET), Some(&json!(-18_000_000)));
    assert_eq!(btai.pointer(BTAI_TIME_ZONE), Some(&json!("America/New_York")));
    assert_eq!(btai.pointer(BTAI_GLOBALS), Some(&json!(1_021_169_763)));
    assert_eq!(btai.pointer(BTAI_NOW), Some(&Value::Null));
    assert_eq!(btai.pointer("/2/36"), Some(&json!("en")));
  }

  #[test]
  fn page_config_fails_without_a_token() {
    assert!(PageConfig::parse("<html>signed out</html>").is_err());
  }

  #[test]
  fn list_request_sets_query_size_and_page() {
    let request = list_request(&Mailbox::Search("from:jpark@example.com".into()), 2);
    assert_eq!(request.pointer(BV_QUERY), Some(&json!("from:jpark@example.com")));
    assert_eq!(request.pointer(BV_SIZE), Some(&json!(PAGE_SIZE)));
    assert_eq!(request.pointer(BV_PAGE), Some(&json!(2)));
    assert_eq!(request.pointer("/0/5"), Some(&json!("itemlist-ViewType(1)-1")));
    assert_eq!(query(&Mailbox::Inbox), "in:inbox");
    assert_eq!(query(&Mailbox::Starred), "is:starred");
    assert_eq!(query(&Mailbox::Sent), "in:sent");
    assert_eq!(query(&Mailbox::All), "in:all");
  }

  #[test]
  fn answers_lose_their_guard_and_errors_carry_status() {
    let answer = parse_answer(")]}'\n[\"er\",null,null,null,null,400,null,null,null,3]").expect("parses");
    assert_eq!(error_status(&answer), Some(400));
    assert_eq!(error_status(&json!([0, []])), None);
  }

  #[test]
  fn jar_keeps_google_and_accounts_cookies_only() {
    use reqwest::cookie::CookieStore;

    let cookie = |name: &str, domain: &str| SessionCookie {
      name: name.into(),
      value: "v".into(),
      domain: domain.into(),
      path: "/".into(),
    };
    let jar = jar(&[
      cookie("SID", ".google.com"),
      cookie("LSID", "accounts.google.com"),
      cookie("COMPASS", "chat.google.com"),
    ]);
    let header = |url: &str| {
      jar
        .cookies(&url.parse().expect("url"))
        .map_or_default(|value| value.to_str().unwrap_or_default().to_string())
    };
    assert_eq!(header("https://mail.google.com/"), "SID=v");
    let accounts = header("https://accounts.google.com/");
    let mut sent: Vec<&str> = accounts.split("; ").collect();
    sent.sort_unstable();
    assert_eq!(sent, ["LSID=v", "SID=v"]);
    assert_eq!(header("https://chat.google.com/"), "SID=v");
  }

  fn thread(id: &str, time: Micros, unread: bool) -> MailThread {
    MailThread {
      id: id.into(),
      subject: "Lunch on Friday".into(),
      snippet: String::new(),
      senders: vec![MailAddress {
        name: "Jordan Park".into(),
        email: "jpark@example.com".into(),
      }],
      time,
      messages: 1,
      unread,
      starred: false,
      attachments: false,
    }
  }

  fn ids(threads: &[MailThread]) -> Vec<&str> {
    threads.iter().map(|thread| thread.id.as_str()).collect()
  }

  #[test]
  fn newest_page_keeps_older_pages_held() {
    let held = [thread("c", 30, false), thread("b", 20, false), thread("a", 10, false)];
    let fresh = vec![thread("d", 40, true), thread("c", 31, false)];
    assert_eq!(ids(&merge_newest(&held, fresh.clone(), false)), ["d", "c", "b", "a"]);
    assert_eq!(ids(&merge_newest(&held, fresh, true)), ["d", "c"]);
    assert_eq!(merge_newest(&held, Vec::new(), false), Vec::new());
  }

  #[test]
  fn older_pages_skip_threads_held() {
    let mut held = vec![thread("c", 30, false), thread("b", 20, false)];
    append_older(&mut held, vec![thread("b", 20, false), thread("a", 10, false)]);
    assert_eq!(ids(&held), ["c", "b", "a"]);
  }

  #[test]
  fn arrivals_are_unread_threads_after_the_first_load() {
    let mut arrivals = Arrivals::default();
    assert_eq!(arrivals.arrived(&[thread("old", 100, true)], 500), Vec::new());
    let inbox = [thread("new", 200, true), thread("read", 300, false), thread("old", 100, true)];
    assert_eq!(ids(&arrivals.arrived(&inbox, 500)), ["new"]);
    assert!(arrivals.arrived(&inbox, 600).is_empty(), "told once");
    let replied = [thread("new", 250, true), thread("old", 100, true)];
    assert_eq!(ids(&arrivals.arrived(&replied, 700)), ["new"], "a newer message tells again");

    let mut empty = Arrivals::default();
    assert_eq!(empty.arrived(&[], 1_000), Vec::new());
    assert_eq!(empty.arrived(&[thread("older", 900, true)], 1_100), Vec::new());
    assert_eq!(ids(&empty.arrived(&[thread("first", 1_050, true)], 1_200)), ["first"]);
  }

  #[test]
  fn mail_urls_and_images() {
    assert!(is_mail_url("https://mail.google.com/mail/u/0/?ui=2&attid=0.1&view=att"));
    assert!(!is_mail_url("https://ci3.googleusercontent.com/proxy/abc"));
    assert!(!is_mail_url("not a url"));
    assert!(looks_like_image(b"\x89PNG\r\n\x1a\n...."));
    assert!(looks_like_image(&[0xff, 0xd8, 0xff, 0xe0]));
    assert!(looks_like_image(b"RIFF\0\0\0\0WEBPVP8 "));
    assert!(looks_like_image(b"\0\0\0\x1cftypheic"));
    assert!(!looks_like_image(b"%PDF-1.7"));
    assert!(!looks_like_image(b"\x1f\x8b\x08"));
    assert!(!looks_like_image(b""));
  }

  fn person(name: &str, email: &str) -> MailAddress {
    MailAddress { name: name.into(), email: email.into() }
  }

  fn email(from: MailAddress, to: Vec<MailAddress>, cc: Vec<MailAddress>, reply_to: Vec<MailAddress>) -> MailMessage {
    MailMessage {
      id: "msg-f:1".into(),
      from,
      to,
      cc,
      bcc: Vec::new(),
      reply_to,
      time: 1_794_896_454_636_000,
      html: "<div dir=\"ltr\">Noon works.</div>".into(),
      text: "Noon works.".into(),
      unread: false,
      draft: false,
      attachments: Vec::new(),
    }
  }

  const ME: &str = "jpark@example.com";

  #[test]
  fn reply_goes_to_the_sender_or_its_reply_to() {
    let riley = person("Riley Brooks", "rbrooks@example.com");
    let sam = person("Sam Lee", "slee@example.com");
    let ana = person("", "ana@example.com");
    let message = email(
      riley.clone(),
      vec![person("Jordan Park", "JPark@example.com"), sam.clone()],
      vec![ana.clone()],
      Vec::new(),
    );
    assert_eq!(reply_recipients(&message, ME, false), (vec![riley.clone()], Vec::new()));
    assert_eq!(
      reply_recipients(&message, ME, true),
      (vec![riley.clone(), sam.clone()], vec![ana])
    );

    let list = person("Team", "team-replies@example.com");
    let listed = email(riley, vec![sam.clone()], Vec::new(), vec![list.clone()]);
    assert_eq!(reply_recipients(&listed, ME, false), (vec![list.clone()], Vec::new()));
    assert_eq!(reply_recipients(&listed, ME, true), (vec![list, sam], Vec::new()));
  }

  #[test]
  fn reply_to_ones_own_email_goes_to_its_recipients_once() {
    let me = person("Jordan Park", ME);
    let riley = person("Riley Brooks", "rbrooks@example.com");
    let mine = email(
      me.clone(),
      vec![riley.clone(), person("Riley", "RBROOKS@example.com")],
      vec![person("", "rbrooks@example.com"), me.clone()],
      Vec::new(),
    );
    assert_eq!(reply_recipients(&mine, ME, false), (vec![riley.clone()], Vec::new()));
    assert_eq!(reply_recipients(&mine, ME, true), (vec![riley], Vec::new()));
    let to_self = email(me.clone(), vec![me.clone()], Vec::new(), Vec::new());
    assert_eq!(
      reply_recipients(&to_self, ME, true),
      (vec![me], Vec::new()),
      "a thread with oneself"
    );
  }

  #[test]
  fn reply_text_subject_and_quote() {
    assert_eq!(reply_subject("Lunch on Friday"), "Re: Lunch on Friday");
    assert_eq!(reply_subject("RE: Lunch"), "RE: Lunch");
    assert_eq!(reply_subject(""), "Re: ");
    assert_eq!(
      written_html("\nSure, <noon> & coffee.\r\nSee you there.\n\n  \nJordan\n\n", None),
      "<div dir=\"ltr\"><div>Sure, &lt;noon&gt; &amp; coffee.</div><div>See you there.</div><div><br></div><div><br></div>\
       <div>Jordan</div></div>"
    );
    assert_eq!(written_html("  \n ", None), "<div dir=\"ltr\"></div>");
    let riley = person("Riley Brooks", "rbrooks@example.com");
    let quoted = quoted_html(&email(riley, Vec::new(), Vec::new(), Vec::new()));
    assert!(
      quoted
        .starts_with("<br><div class=\"gmail_quote gmail_quote_container\"><div dir=\"ltr\" class=\"gmail_attr\">On ")
    );
    assert!(
      quoted.contains(" Riley Brooks &lt;rbrooks@example.com&gt; wrote:<br></div><blockquote class=\"gmail_quote\"")
    );
    assert!(quoted.ends_with("<div dir=\"ltr\">Noon works.</div></blockquote></div>"));
  }

  #[test]
  fn changes_carry_labels_and_message_ids() {
    let ids = vec!["msg-f:1".to_string(), "msg-f:2".to_string()];
    let read = label_op(&[LABEL_OPENED], &[LABEL_UNREAD, LABEL_UNSEEN], &ids);
    assert_eq!(
      read,
      json!([
        null,
        null,
        null,
        null,
        null,
        null,
        [["^o"], ["^u", "^us"], ["msg-f:1", "msg-f:2"]]
      ])
    );
    let unread = label_op(&[LABEL_UNREAD], &[], &ids[1..]);
    assert_eq!(unread.pointer("/6"), Some(&json!([["^u"], null, ["msg-f:2"]])));
  }

  #[test]
  fn a_reply_message_fills_gmails_slots() {
    let me = person("Jordan Park", ME);
    let reply = MailReply {
      thread: "thread-f:1".into(),
      message: "msg-f:1".into(),
      to: vec![person("Riley Brooks", "rbrooks@example.com")],
      cc: vec![person("", "jpark@example.com")],
      text: "Noon works.".into(),
      signature: false,
    };
    let message = sent_message(&SentEmail {
      id: "msg-a:r42",
      client: "s:00000000000000ab",
      me: &me,
      to: &reply.to,
      cc: &reply.cc,
      bcc: &[],
      subject: "Re: Lunch on Friday",
      written: "<p>Noon works.</p>".into(),
      answering: Some(Answering {
        in_reply_to: Some("<lunch-1@mail.example.com>"),
        quoted: "<br><div>quote</div>".into(),
      }),
      now: 1_794_896_454_636,
    });
    let slot = |index: usize| message.get(index).cloned().unwrap_or_default();
    assert_eq!(message.as_array().map(Vec::len), Some(SENT_SLOTS));
    assert_eq!(slot(SENT_ID), json!("msg-a:r42"));
    assert_eq!(
      slot(SENT_FROM),
      json!([1, ME, "Jordan Park", null, null, null, null, null, null, ME])
    );
    assert_eq!(slot(SENT_TO), json!([[1, "rbrooks@example.com", "Riley Brooks"]]));
    assert_eq!(slot(SENT_CC), json!([[1, "jpark@example.com"]]));
    assert_eq!(slot(SENT_SUBJECT), json!("Re: Lunch on Friday"));
    assert_eq!(slot(SENT_BODY).pointer("/1/1"), Some(&json!([2, "<br><div>quote</div>"])));
    assert_eq!(slot(SENT_IN_REPLY_TO), json!("<lunch-1@mail.example.com>"));
    assert_eq!(slot(SENT_CLIENT_ID), json!("s:00000000000000ab|#msg-a:r42|0"));
    assert_eq!(slot(SENT_KIND), json!(2));
    let labels = slot(SENT_LABELS);
    assert!(
      labels
        .as_array()
        .is_some_and(|labels| labels.contains(&json!("^i")) && !labels.contains(&json!("^u")))
    );
  }

  /// A new email as the web client sent one: kind 1, the written part alone, no In-Reply-To.
  #[test]
  fn a_new_email_fills_gmails_slots() {
    let me = person("Jordan Park", ME);
    let to = vec![person("Riley Brooks", "rbrooks@example.com")];
    let message = sent_message(&SentEmail {
      id: "msg-a:r7",
      client: "s:00000000000000ab",
      me: &me,
      to: &to,
      cc: &[],
      bcc: &[],
      subject: "Lunch on Friday",
      written: written_html("Noon?\n\nAt the usual place.", None),
      answering: None,
      now: 1_794_896_454_636,
    });
    let slot = |index: usize| message.get(index).cloned().unwrap_or_default();
    assert_eq!(message.as_array().map(Vec::len), Some(SENT_SLOTS));
    assert_eq!(slot(SENT_TO), json!([[1, "rbrooks@example.com", "Riley Brooks"]]));
    assert_eq!(slot(SENT_CC), Value::Null);
    assert_eq!(slot(SENT_SUBJECT), json!("Lunch on Friday"));
    assert_eq!(
      slot(SENT_BODY),
      json!([
        null,
        [[
          0,
          "<div dir=\"ltr\"><div>Noon?</div><div><br></div><div>At the usual place.</div></div>"
        ]],
        null,
        null,
        null,
        null,
        1
      ])
    );
    assert_eq!(slot(SENT_IN_REPLY_TO), Value::Null);
    assert_eq!(slot(SENT_KIND), json!(1));
    assert_eq!(slot(SENT_FLAGS), json!([0, 0, 1, null, 0]));
    assert!(!slot(SENT_LABELS).as_array().is_some_and(|labels| labels.contains(&json!("^i"))));
    // To oneself: in the inbox as well, read.
    let mine = vec![person("", ME)];
    let message = sent_message(&SentEmail {
      id: "msg-a:r8",
      client: "s:00000000000000ab",
      me: &me,
      to: &[],
      cc: &mine,
      bcc: &[],
      subject: "",
      written: written_html("Note to self.", None),
      answering: None,
      now: 1_794_896_454_636,
    });
    let labels = message.get(SENT_LABELS).cloned().unwrap_or_default();
    assert!(
      labels
        .as_array()
        .is_some_and(|labels| labels.contains(&json!("^i")) && !labels.contains(&json!("^u")))
    );
    assert!(new_thread_id().starts_with("thread-a:r"));
    // The thread is made with the same email as a draft: draft labels, no send kind, the rest as sent.
    let draft = draft_of(&message);
    assert_eq!(
      draft.get(SENT_LABELS),
      Some(&json!(["^all", "^r", "^r_bt", "^io_im", "^io_imc3"]))
    );
    assert_eq!(draft.get(SENT_KIND), Some(&Value::Null));
    assert_eq!(draft.get(SENT_ID), message.get(SENT_ID));
    assert_eq!(draft.get(SENT_BODY), message.get(SENT_BODY));
  }

  #[test]
  fn a_send_needs_someone_with_an_address() {
    let riley = person("Riley Brooks", "rbrooks@example.com");
    assert!(check_recipients(std::slice::from_ref(&riley), &[], &[]).is_ok());
    assert!(check_recipients(&[], std::slice::from_ref(&riley), &[]).is_ok());
    assert!(
      check_recipients(&[], &[], std::slice::from_ref(&riley)).is_ok(),
      "bcc alone is someone"
    );
    let none = check_recipients(&[], &[], &[]).map_err(|error| error.to_string());
    assert_eq!(none, Err("Add someone to send the email to.".into()));
    assert!(
      check_recipients(&[], &[], &[person("", "rbrooks")]).is_err(),
      "bcc is checked too"
    );
    for wrong in [
      "rbrooks",
      "rbrooks@",
      "@example.com",
      "rbrooks@example",
      "r brooks@example.com",
      "rbrooks@@example.com",
      "rbrooks@example..com",
      "rbrooks@example.com.",
      "<rbrooks@example.com>",
    ] {
      let result = check_recipients(&[riley.clone(), person("", wrong)], &[], &[]);
      assert!(result.is_err(), "{wrong} passed");
    }
    for right in ["r.brooks+lunch@mail.example.co.uk", "jpark@example.com"] {
      assert!(plausible_email(right), "{right} failed");
    }
  }

  fn inbox_page(threads: Vec<MailThread>, unread: u32) -> ThreadPage {
    ThreadPage {
      threads,
      more: false,
      inbox_unread: Some(unread),
      labels: Some(Vec::new()),
    }
  }

  #[test]
  fn an_answer_asked_for_before_a_change_shows_the_change() {
    let mut changes = Changes::default();
    let early = changes.request();
    changes.note("a", Some(false), None);
    changes.note("b", None, Some(false));
    let mut page = inbox_page(vec![thread("a", 30, true), thread("b", 20, true), thread("c", 10, true)], 3);
    changes.apply(&mut page, true, early);
    assert_eq!(ids(&page.threads), ["a", "c"]);
    assert!(!page.threads[0].unread);
    assert_eq!(page.inbox_unread, Some(1), "a read, b archived");
    assert_eq!(page.labels, None, "label counts from before the change are left out");

    // Written, then asked for: Gmail's answer has the change and is taken as it is.
    changes.written("a");
    changes.written("b");
    let late = changes.request();
    let mut fresh = inbox_page(vec![thread("a", 30, false), thread("c", 10, true)], 1);
    changes.apply(&mut fresh, true, late);
    assert_eq!(fresh.inbox_unread, Some(1));
    assert_eq!(fresh.labels, Some(Vec::new()));

    // An answer asked for before the write, arriving after it, is still corrected.
    let mut stale = inbox_page(vec![thread("a", 30, true), thread("b", 20, true)], 2);
    changes.apply(&mut stale, true, early);
    assert_eq!(ids(&stale.threads), ["a"]);
    assert_eq!(stale.inbox_unread, Some(0));
  }

  #[test]
  fn a_change_gmail_did_not_take_stops_correcting() {
    let mut changes = Changes::default();
    let early = changes.request();
    changes.note("a", Some(true), None);
    let mut other = inbox_page(vec![thread("a", 30, false)], 4);
    changes.apply(&mut other, false, early);
    assert!(other.threads[0].unread);
    assert_eq!(other.inbox_unread, None, "another list's inbox count may predate the change");
    changes.forget("a");
    let mut page = inbox_page(vec![thread("a", 30, false)], 4);
    changes.apply(&mut page, true, early);
    assert!(!page.threads[0].unread);
    assert_eq!(page.inbox_unread, Some(4));
  }

  #[test]
  fn a_star_on_its_way_shows_in_answers_asked_before_it() {
    let mut changes = Changes::default();
    let early = changes.request();
    changes.note_starred("a", true);
    let mut page = inbox_page(vec![thread("a", 30, true), thread("b", 20, false)], 1);
    changes.apply(&mut page, true, early);
    assert!(page.threads[0].starred && !page.threads[1].starred);
    assert_eq!(page.inbox_unread, Some(1), "a star moves no count");
    changes.written("a");
    let late = changes.request();
    let mut fresh = inbox_page(vec![thread("a", 30, true)], 1);
    changes.apply(&mut fresh, true, late);
    assert!(!fresh.threads[0].starred, "an answer asked for after the write is Gmail's own");
  }

  #[test]
  fn stars_and_labels_change_labels_on_the_emails() {
    let ids = vec!["msg-f:1".to_string(), "msg-f:2".to_string()];
    assert_eq!(star_op(&ids, true).pointer("/6"), Some(&json!([["^t"], null, ["msg-f:2"]])));
    assert_eq!(
      star_op(&ids, false).pointer("/6"),
      Some(&json!([[], ["^t"], ["msg-f:1", "msg-f:2"]]))
    );
    let add = vec!["^x_lunch".to_string()];
    let remove = vec!["^x_old".to_string()];
    assert_eq!(
      labels_op(&add, &remove, &ids).pointer("/6"),
      Some(&json!([["^x_lunch"], ["^x_old"], ["msg-f:1", "msg-f:2"]]))
    );
    assert_eq!(
      labels_op(&add, &[], &ids).pointer("/6"),
      Some(&json!([["^x_lunch"], null, ["msg-f:1", "msg-f:2"]]))
    );
    assert_eq!(
      labels_op(&[], &remove, &ids).pointer("/6"),
      Some(&json!([[], ["^x_old"], ["msg-f:1", "msg-f:2"]]))
    );
    assert!(check_user_labels(&add, &remove).is_ok());
    assert!(check_user_labels(&[], &[]).is_err(), "nothing to change");
    assert!(check_user_labels(&add, &add).is_err(), "on and off at once");
    for wrong in ["^t", "^i", "^u", "^x_", "Lunch", ""] {
      assert!(check_user_labels(&[wrong.to_string()], &[]).is_err(), "{wrong} passed");
      assert!(check_user_labels(&[], &[wrong.to_string()]).is_err(), "{wrong} passed");
    }
  }

  /// A draft is saved, then sent as the same email: one message id, the draft labels and no kind while saved, the
  /// sent labels and the send kind when sent.
  #[test]
  fn a_draft_is_sent_as_saved() {
    let me = person("Jordan Park", ME);
    let to = vec![person("Riley Brooks", "rbrooks@example.com")];
    let new = SentEmail {
      id: "msg-a:r7",
      client: "s:00000000000000ab",
      me: &me,
      to: &to,
      cc: &[],
      bcc: &[],
      subject: "Lunch on Friday",
      written: written_html("Noon?", None),
      answering: None,
      now: 1_794_896_454_636,
    };
    let draft = email_draft(&new, "thread-a:r5", None, false);
    assert!(!draft.is_reply() && !draft.to_me);
    assert_eq!((draft.thread.as_str(), draft.to.clone()), ("thread-a:r5", to.clone()));
    // A new email's draft makes its thread with it.
    let create = draft_op(&draft);
    assert_eq!(create.as_array().map(Vec::len), Some(OP_CREATE + 1));
    assert_eq!(create.pointer("/2/0/0"), Some(&json!("Lunch on Friday")));
    assert_eq!(create.pointer("/2/0/2"), Some(&json!(1_794_896_454_636_u64)));
    assert_eq!(create.pointer("/2/0/3"), Some(&json!("thread-a:r5")));
    let saved = create.pointer("/2/0/4/0").cloned().unwrap_or_default();
    let op = send_op(&draft, 1_794_896_460_000);
    assert_eq!(op.as_array().map(Vec::len), Some(OP_SEND + 1));
    assert_eq!(op.pointer("/13/1"), Some(&Value::Null));
    assert_eq!(op.pointer("/13/2"), Some(&json!(1)));
    let sent = op.pointer("/13/0").cloned().unwrap_or_default();
    assert_eq!(saved.get(SENT_ID), Some(&json!("msg-a:r7")));
    assert_eq!(sent.get(SENT_ID), saved.get(SENT_ID));
    assert_eq!(sent.get(SENT_BODY), saved.get(SENT_BODY));
    assert_eq!(saved.get(SENT_LABELS), Some(&json!(DRAFT_LABEL_SET)));
    assert_eq!(sent.get(SENT_LABELS), Some(&json!(SENT_LABEL_SET)));
    assert_eq!(saved.get(SENT_KIND), Some(&Value::Null));
    assert_eq!(sent.get(SENT_KIND), Some(&json!(SENT_KIND_NEW)));
    assert_eq!(sent.get(SENT_FLAGS), Some(&json!([0, 0, 1, null, 0])));
    let signed = email_draft(&new, "thread-a:r5", None, true);
    assert_eq!(signed.message.get(SENT_FLAGS), Some(&json!([0, 0, 0, null, 0])));

    // A reply's draft is saved into its thread and sent answering its email, the user among its recipients.
    let mine = vec![person("", ME)];
    let reply = SentEmail {
      id: "msg-a:r8",
      client: "s:00000000000000ab",
      me: &me,
      to: &to,
      cc: &mine,
      bcc: &[],
      subject: "Re: Lunch on Friday",
      written: written_html("Noon works.", None),
      answering: Some(Answering {
        in_reply_to: Some("<lunch-1@mail.example.com>"),
        quoted: "<br><div>quote</div>".into(),
      }),
      now: 1_794_896_454_636,
    };
    let draft = email_draft(&reply, "thread-f:1", Some("msg-f:1"), false);
    assert!(draft.is_reply() && draft.to_me);
    let saved = draft_op(&draft);
    assert_eq!(saved.as_array().map(Vec::len), Some(OP_SAVE_DRAFT + 1));
    let saved = saved.pointer("/1/0").cloned().unwrap_or_default();
    let op = send_op(&draft, 1_794_896_460_000);
    assert_eq!(op.pointer("/13/1"), Some(&json!("msg-f:1")));
    assert_eq!(op.pointer("/13/4"), Some(&json!([1_794_896_460_000_u64])));
    let sent = op.pointer("/13/0").cloned().unwrap_or_default();
    assert_eq!(sent.get(SENT_ID), Some(&json!("msg-a:r8")));
    assert_eq!(saved.get(SENT_ID), sent.get(SENT_ID));
    assert_eq!(saved.get(SENT_IN_REPLY_TO), sent.get(SENT_IN_REPLY_TO));
    assert_eq!(saved.get(SENT_LABELS), Some(&json!(DRAFT_LABEL_SET)));
    assert_eq!(saved.get(SENT_KIND), Some(&Value::Null));
    assert_eq!(sent.get(SENT_KIND), Some(&json!(SENT_KIND_REPLY)));
    assert!(new_message_id().starts_with("msg-a:r"));
  }

  #[test]
  fn bcc_goes_in_its_own_slot_of_the_draft_and_the_send() {
    let me = person("Jordan Park", ME);
    let to = vec![person("Riley Brooks", "rbrooks@example.com")];
    let hidden = vec![person("", "team@example.com"), person("Jordan Park", ME)];
    let email = SentEmail {
      id: "msg-a:r9",
      client: "s:00000000000000ab",
      me: &me,
      to: &to,
      cc: &[],
      bcc: &hidden,
      subject: "Lunch on Friday",
      written: written_html("Noon?", None),
      answering: None,
      now: 1_794_896_454_636,
    };
    let draft = email_draft(&email, "thread-a:r5", None, false);
    assert_eq!(draft.bcc, hidden);
    assert!(draft.to_me, "bcc to oneself puts it in the inbox too");
    let bcc = json!([[1, "team@example.com"], [1, ME, "Jordan Park"]]);
    let saved = draft_op(&draft).pointer("/2/0/4/0").cloned().unwrap_or_default();
    let sent = send_op(&draft, 1_794_896_460_000).pointer("/13/0").cloned().unwrap_or_default();
    for message in [&saved, &sent] {
      assert_eq!(message.get(SENT_BCC), Some(&bcc));
      assert_eq!(message.get(SENT_CC), Some(&Value::Null));
      assert!(message.get(SENT_LABELS).and_then(Value::as_array).is_some());
    }
    assert!(
      sent
        .get(SENT_LABELS)
        .and_then(Value::as_array)
        .is_some_and(|labels| labels.contains(&json!("^i")))
    );
    let unseen = SentEmail { bcc: &[], ..email };
    assert_eq!(sent_message(&unseen).get(SENT_BCC), Some(&Value::Null), "no bcc, no slot");
  }

  #[test]
  fn a_forward_quotes_the_email_under_gmails_header() {
    assert_eq!(forward_subject("Lunch on Friday"), "Fwd: Lunch on Friday");
    assert_eq!(forward_subject(" FWD: Lunch "), "FWD: Lunch");
    assert_eq!(forward_subject("fwd:Lunch"), "fwd:Lunch");
    assert_eq!(forward_subject("Re: Lunch"), "Fwd: Re: Lunch");
    let message = email(
      person("Jordan Park", "jpark@example.com"),
      vec![person("Riley <R&B>", "rbrooks@example.com"), person("", "team@example.com")],
      Vec::new(),
      Vec::new(),
    );
    let date = gmail_date(message.time);
    let block = forwarded_html(&message, "Lunch & \"more\"");
    assert_eq!(
      block,
      format!(
        "<div class=\"gmail_quote gmail_quote_container\"><div dir=\"ltr\" class=\"gmail_attr\">---------- \
         Forwarded message ---------<br>From: <strong class=\"gmail_sendername\" dir=\"auto\">Jordan Park</strong> \
         <span dir=\"auto\">&lt;<a href=\"mailto:jpark@example.com\">jpark@example.com</a>&gt;</span><br>Date: \
         {date}<br>Subject: Lunch &amp; &quot;more&quot;<br>To: Riley &lt;R&amp;B&gt; &lt;<a \
         href=\"mailto:rbrooks@example.com\">rbrooks@example.com</a>&gt;, &lt;<a \
         href=\"mailto:team@example.com\">team@example.com</a>&gt;<br></div><br><br><div dir=\"ltr\">Noon \
         works.</div></div>"
      )
    );
    assert!(!block.contains("Cc: "), "no Cc line without copies");
    let copied = email(
      person("", "jpark@example.com"),
      Vec::new(),
      vec![person("Team", "team@example.com")],
      Vec::new(),
    );
    let block = forwarded_html(&copied, "Lunch");
    assert!(block.contains("From: <span dir=\"auto\">&lt;<a href=\"mailto:jpark@example.com\">"));
    assert!(block.contains("<br>Cc: Team &lt;<a href=\"mailto:team@example.com\">team@example.com</a>&gt;<br></div>"));
    let english = Regex::new(r"^[A-Z][a-z]{2}, [A-Z][a-z]{2} \d{1,2}, \d{4} at \d{1,2}:\d{2} [AP]M$").expect("valid");
    assert!(english.is_match(&date), "{date}");
  }

  fn signature(html: &str) -> MailSignature {
    MailSignature {
      html: html.into(),
      text: html_to_text(html),
    }
  }

  /// The markup Gmail's own client sent for a signed new email and a signed reply (structure read from a Gmail-sent
  /// pair, made-up text here).
  #[test]
  fn a_signature_goes_where_gmail_puts_it() {
    let jordan = "<div dir=\"ltr\">Jordan Park<div><b>Example Co.</b></div></div>";
    let prefix = "<div><br></div><span class=\"gmail_signature_prefix\">-- </span><br>";
    let lead = format!("<div><br clear=\"all\"></div>{prefix}");
    let element =
      format!("<div dir=\"ltr\" class=\"gmail_signature\" data-smartmail=\"gmail_signature\">{jordan}</div>");
    let after_quote = format!("{lead}<div dir=\"ltr\" class=\"gmail_signature\">{jordan}</div>");
    // A new email: the user's text takes the place of the empty first line Gmail's compose window opens with.
    assert_eq!(
      signature_block(jordan, SignaturePlace::new_email("")),
      format!("{lead}{element}")
    );
    assert_eq!(
      signature_block(jordan, SignaturePlace::new_email("Noon?")),
      format!("{prefix}{element}")
    );
    assert_eq!(signature_block(jordan, SignaturePlace::AfterQuote), after_quote);
    assert_eq!(
      signature_block(jordan, SignaturePlace::BeforeQuote),
      format!("<div><br clear=\"all\"></div><div><div dir=\"ltr\" class=\"gmail_signature\">{jordan}</div></div>")
    );
    // A signature that is already Gmail's element gets Gmail's opening tag; one written right to left keeps its way.
    assert_eq!(
      signature_block(
        "<div class=\"gmail_signature\" id=\"x\">Jordan Park</div>",
        SignaturePlace::AfterQuote
      ),
      format!("{lead}<div dir=\"ltr\" class=\"gmail_signature\">Jordan Park</div>")
    );
    assert!(
      signature_block("<div dir=\"rtl\">יורדן</div>", SignaturePlace::new_email("x"))
        .contains("<div dir=\"rtl\" class=\"gmail_signature\"")
    );

    // A new email: under the text, inside the written part's element, as Gmail's own.
    assert_eq!(
      written_html("Noon?", Some(&signature_block(jordan, SignaturePlace::new_email("Noon?")))),
      format!("<div dir=\"ltr\"><div>Noon?</div>{prefix}{element}</div>")
    );
    assert_eq!(
      written_html("", Some(&signature_block(jordan, SignaturePlace::new_email("")))),
      format!("<div dir=\"ltr\">{lead}{element}</div>")
    );
    // A reply: at the end of the quoted part, or above the quote without the `-- ` line when the setting says so.
    let sig = signature(jordan);
    let quote = "<br><div class=\"gmail_quote\">quoted</div>".to_string();
    let (written, quoted) = reply_body("Sure.", quote.clone(), Some(&sig), false);
    assert_eq!(written, "<div dir=\"ltr\"><div>Sure.</div></div>");
    assert_eq!(quoted, format!("{quote}{after_quote}"));
    let (written, quoted) = reply_body("Sure.", quote.clone(), Some(&sig), true);
    assert_eq!(
      written,
      format!(
        "<div dir=\"ltr\"><div>Sure.</div><div><br clear=\"all\"></div><div><div dir=\"ltr\" \
         class=\"gmail_signature\">{jordan}</div></div></div>"
      )
    );
    assert_eq!(quoted, quote);
    assert_eq!(
      reply_body("Sure.", quote.clone(), None, true),
      ("<div dir=\"ltr\"><div>Sure.</div></div>".to_string(), quote)
    );
  }

  /// The settings as the page has them live: the signature switch, the one signature (plain text), and where a
  /// reply's goes.
  #[test]
  fn the_page_settings_give_the_signature_and_where_it_goes() {
    let settings = json!([
      ["qu", ""],
      [
        "p",
        [
          ["bx_thb", "1"],
          ["bx_se", "1"],
          ["bx_stw", "1"],
          ["sx_sg", "Jordan Park\nExample Co. <ops> & sales"]
        ]
      ]
    ]);
    let read = page_signatures(Some(&settings), None, None);
    let signature = MailSignature {
      html: "Jordan Park<br>Example Co. &lt;ops&gt; &amp; sales".into(),
      text: "Jordan Park\nExample Co. <ops> & sales".into(),
    };
    assert_eq!(
      read,
      Signatures {
        new: Some(signature.clone()),
        reply: Some(signature),
        before_quote: true
      }
    );
    // Markup is kept as it is.
    let rich = json!([["p", [["bx_se", "1"], ["sx_sg", "<div><b>Jordan Park</b></div>"]]]]);
    assert_eq!(
      page_signatures(Some(&rich), None, None).new.map(|signature| signature.html),
      Some("<div><b>Jordan Park</b></div>".into())
    );
    // Switched off, empty, or never set: none.
    let off = json!([["p", [["bx_se", "0"], ["bx_stw", "0"], ["sx_sg", "Jordan Park"]]]]);
    assert_eq!(page_signatures(Some(&off), None, None), Signatures::default());
    let empty = json!([["p", [["bx_se", "1"], ["sx_sg", "  "]]]]);
    assert_eq!(page_signatures(Some(&empty), None, None), Signatures::default());
    assert_eq!(page_signatures(None, None, None), Signatures::default());
  }

  /// The formatted signatures in the rich settings, as read live: named signatures with HTML, and per address which
  /// one new emails and replies use. Their text reads without markup; the plain `sx_sg` is not used beside them.
  #[test]
  fn the_rich_settings_give_the_formatted_signatures_and_picks() {
    let settings = json!([["p", [["bx_se", "1"], ["bx_stw", "0"], ["sx_sg", "Jordan Park\n*Example Co.*"]]]]);
    let named = |picks: Value| {
      let mut list = json!([[[["104", {"1": "x"}], ["105", {}]]]]);
      list[0][0][1][1][SIGNATURES_EXTENSION] = json!([
        [[
          [
            "sig-long",
            null,
            "Long",
            [null, "<div dir=\"ltr\">Jordan Park<div><b>Example Co.</b></div></div>"]
          ],
          ["sig-short", null, "Short", [null, "<div dir=\"ltr\">Jordan</div>"]]
        ]],
        [picks]
      ]);
      list
    };
    let rich = named(json!([
      ["other@example.com", "sig-short", ""],
      ["JPark@example.com", "sig-long", "sig-short"]
    ]));
    let read = page_signatures(Some(&settings), Some(&rich), Some("jpark@example.com"));
    let new = read.new.unwrap();
    assert_eq!(new.html, "<div dir=\"ltr\">Jordan Park<div><b>Example Co.</b></div></div>");
    assert_eq!(new.text, "Jordan Park\nExample Co.");
    assert_eq!(read.reply.unwrap().text, "Jordan");
    // An empty pick is "No signature"; it never falls back to the plain form.
    let rich = named(json!([["jpark@example.com", "", "sig-short"]]));
    let read = page_signatures(Some(&settings), Some(&rich), Some("jpark@example.com"));
    assert!(read.new.is_none() && read.reply.is_some());
    // Without named signatures, the address's own one serves both kinds.
    let mut by_address = json!([[[["61", {}]]]]);
    by_address[0][0][0][1][ADDRESS_SIGNATURES_EXTENSION] = json!([[
      ["jpark@example.com", [null, "<div><b>Jordan</b></div>"]],
      ["other@example.com", [null, ""]]
    ]]);
    let read = page_signatures(Some(&settings), Some(&by_address), Some("jpark@example.com"));
    assert_eq!(read.new, read.reply);
    assert_eq!(read.new.unwrap().text, "Jordan");
    // With the switch off, none at all.
    let off = json!([["p", [["bx_se", "0"]]]]);
    assert_eq!(
      page_signatures(Some(&off), Some(&rich), Some("jpark@example.com")),
      Signatures::default()
    );
  }
}
