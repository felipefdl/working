//! Google Calendar with the Chat session: the Calendar v3 JSON API at `clients6.google.com`, signed the way
//! calendar.google.com signs its own calls: the session's SAPISID hashes and the browser key of that page.
//!
//! Opening a session loads the calendar page once. On its way there through accounts.google.com, Google sets the
//! calendar's own cookies; the page holds a few browser keys, and the first one the API accepts is kept.
#![allow(
  clippy::unreadable_literal,
  clippy::expect_used,
  clippy::indexing_slicing,
  clippy::significant_drop_tightening,
  reason = "colors are 0xRRGGBB, the regexes are static, `Value[key]` answers Null for a missing key, and the session \
            lock is held while the session opens so two calls do not open it twice"
)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone, Weekday};
use regex::Regex;
use reqwest::Method;
use reqwest::cookie::Jar;
use serde_json::{Map, Value, json};

use crate::http::{SessionCookie, USER_AGENT, read_cookies};
use crate::model::{
  Calendar, CalendarAccess, CalendarEvent, EditScope, EventDraft, EventKind, Guest, Micros, Reminder, Repeat, Rsvp,
  Visibility,
};

const ORIGIN: &str = "https://calendar.google.com";
const PAGE: &str = "https://calendar.google.com/calendar/u/0/r";
const API: &str = "https://clients6.google.com/calendar/v3";
/// Events per page, the most the API gives.
const PAGE_SIZE: &str = "2500";

/// The event colors of Google Calendar, by color id 1 to 11: name and `0xRRGGBB` as the web app paints them.
pub const EVENT_COLORS: [(u8, &str, u32); 11] = [
  (1, "Lavender", 0x7986cb),
  (2, "Sage", 0x33b679),
  (3, "Grape", 0x8e24aa),
  (4, "Flamingo", 0xe67c73),
  (5, "Banana", 0xf6bf26),
  (6, "Tangerine", 0xf4511e),
  (7, "Peacock", 0x039be5),
  (8, "Graphite", 0x616161),
  (9, "Blueberry", 0x3f51b5),
  (10, "Basil", 0x0b8043),
  (11, "Tomato", 0xd50000),
];

/// The calendar colors of the web app, by color id 1 to 24. The API still answers the pastel palette of the old web
/// app for these ids.
const CALENDAR_COLORS: [u32; 24] = [
  0x795548, 0xe67c73, 0xd50000, 0xf4511e, 0xef6c00, 0xf09300, 0x009688, 0x0b8043, 0x7cb342, 0xc0ca33, 0xe4c441,
  0xf6bf26, 0x33b679, 0x039be5, 0x4285f4, 0x3f51b5, 0x7986cb, 0xb39ddb, 0x616161, 0xa79b8e, 0xad1457, 0xd81b60,
  0x8e24aa, 0x9e69af,
];

/// Cobalt, for a calendar that names no color.
const DEFAULT_COLOR: u32 = 0x4285f4;

/// Google refused the session (401): it is opened again once.
#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str("Google Calendar refused the session")
  }
}

impl std::error::Error for Refused {}

/// Google Calendar for the session in `cookies.json`. Cheap to make: the session opens on the first call.
pub struct Client {
  cookie_path: PathBuf,
  session: tokio::sync::Mutex<Option<Arc<Session>>>,
  /// The calendars last listed, which `events` reads.
  calendars: Mutex<Vec<Calendar>>,
}

impl Client {
  pub fn new(cookie_path: PathBuf) -> Self {
    Self {
      cookie_path,
      session: tokio::sync::Mutex::new(None),
      calendars: Mutex::new(Vec::new()),
    }
  }

  async fn session(&self) -> Result<Arc<Session>> {
    let mut held = self.session.lock().await;
    if let Some(session) = held.as_ref() {
      return Ok(session.clone());
    }
    let session = Arc::new(Session::open(&self.cookie_path).await?);
    *held = Some(session.clone());
    Ok(session)
  }

  /// Runs `work` with the session, opening a fresh one once when Google refuses the one held.
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

  /// The account's calendars: the primary one first, then the user's own, then the shared ones, each by name.
  pub async fn calendars(&self) -> Result<Vec<Calendar>> {
    let mut list = self.with_session(|session| async move { session.calendars().await }).await?;
    list.sort_by(|a, b| {
      let rank = |calendar: &Calendar| (!calendar.primary, !calendar.access.can_write());
      rank(a)
        .cmp(&rank(b))
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    list.clone_into(&mut self.calendars.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    Ok(list)
  }

  /// Every calendar's events that overlap `start..end`, by start. A calendar that cannot be read is left out, unless
  /// none can.
  pub async fn events(&self, start: Micros, end: Micros) -> Result<Vec<CalendarEvent>> {
    let mut calendars = self.calendars.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    if calendars.is_empty() {
      calendars = self.calendars().await?;
    }
    let calendars = Arc::new(calendars);
    let mut events = self
      .with_session(|session| {
        let calendars = calendars.clone();
        async move { session.events_of_all(&calendars, start, end).await }
      })
      .await?;
    sort_events(&mut events);
    Ok(events)
  }

  /// Creates the event of `draft`, or changes the one it edits.
  pub async fn save(&self, draft: &EventDraft) -> Result<()> {
    self.with_session(|session| async move { session.save(draft).await }).await
  }

  /// Deletes an event, or every occurrence of the series of the occurrence `event_id` with `EditScope::All`.
  pub async fn delete(&self, calendar_id: &str, event_id: &str, scope: EditScope, notify: bool) -> Result<()> {
    self
      .with_session(|session| async move {
        let target = session.target(calendar_id, event_id, scope).await?;
        let path = event_path(calendar_id, &target);
        session
          .call(Method::DELETE, &path, &[("sendUpdates", send_updates(notify))], None)
          .await?;
        Ok(())
      })
      .await
  }

  /// Answers an invitation for one occurrence, or for the whole series with `EditScope::All`.
  pub async fn respond(&self, calendar_id: &str, event_id: &str, scope: EditScope, rsvp: Rsvp) -> Result<()> {
    self
      .with_session(|session| async move {
        let target = session.target(calendar_id, event_id, scope).await?;
        let path = event_path(calendar_id, &target);
        let event = session.call(Method::GET, &path, &[], None).await?;
        let mut attendees = event["attendees"].as_array().cloned().unwrap_or_default();
        let me = attendees
          .iter_mut()
          .find(|attendee| attendee["self"].as_bool() == Some(true))
          .ok_or_else(|| anyhow!("you are not a guest of this event"))?;
        me["responseStatus"] = json!(rsvp_name(rsvp));
        let body = json!({ "attendees": attendees });
        session
          .call(Method::PATCH, &path, &[("sendUpdates", "all".into())], Some(&body))
          .await?;
        Ok(())
      })
      .await
  }
}

struct Session {
  http: reqwest::Client,
  key: String,
  /// The session's SAPISID cookies, by the label of their hash in the authorization header.
  apisids: Vec<(&'static str, String)>,
  /// The pastel calendar colors the API answers, by color id, to tell a palette color from a custom one.
  pastels: HashMap<String, u32>,
}

impl Session {
  async fn open(cookie_path: &std::path::Path) -> Result<Self> {
    let cookies = read_cookies(cookie_path)?;
    let apisids = apisids(&cookies);
    if apisids.is_empty() {
      bail!("the Google session has no SAPISID cookie; sign in again");
    }
    let http = reqwest::Client::builder()
      .user_agent(USER_AGENT)
      .cookie_provider(Arc::new(jar(&cookies)))
      .connect_timeout(Duration::from_secs(15))
      .timeout(Duration::from_secs(45))
      .build()?;
    let response = http.get(PAGE).send().await.context("Google Calendar did not answer")?;
    let host = response.url().host_str().unwrap_or_default().to_string();
    let page = response.text().await?;
    if host != "calendar.google.com" {
      bail!("Google Calendar asks to sign in again");
    }
    let mut session = Self {
      http,
      key: String::new(),
      apisids,
      pastels: HashMap::new(),
    };
    for key in browser_keys(&page) {
      session.key = key;
      if let Ok(colors) = session.call(Method::GET, "colors", &[], None).await {
        session.pastels = pastels(&colors);
        return Ok(session);
      }
    }
    bail!("Google Calendar did not accept the session")
  }

  /// `SAPISIDHASH <time>_<sha1>` for each SAPISID cookie, as the web app signs a call from its origin.
  fn authorization(&self) -> String {
    let now = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .map_or(0, |elapsed| elapsed.as_secs());
    let hashes: Vec<String> = self
      .apisids
      .iter()
      .map(|(label, value)| {
        let digest = sha1_smol::Sha1::from(format!("{now} {value} {ORIGIN}")).digest().to_string();
        format!("{label} {now}_{digest}")
      })
      .collect();
    hashes.join(" ")
  }

  async fn call(&self, method: Method, path: &str, query: &[(&str, String)], body: Option<&Value>) -> Result<Value> {
    let mut request = self
      .http
      .request(method, format!("{API}/{path}"))
      .query(&[("key", self.key.as_str())])
      .query(query)
      .header("authorization", self.authorization())
      .header("origin", ORIGIN)
      .header("referer", "https://calendar.google.com/")
      .header("x-goog-authuser", "0");
    if let Some(body) = body {
      request = request.header("content-type", "application/json").body(body.to_string());
    }
    let response = request.send().await?;
    let status = response.status();
    let text = response.text().await?;
    if status == reqwest::StatusCode::UNAUTHORIZED {
      return Err(Refused.into());
    }
    if !status.is_success() {
      let reason = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|answer| answer["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| status.to_string());
      bail!("Google Calendar: {reason}");
    }
    if text.trim().is_empty() {
      return Ok(Value::Null);
    }
    serde_json::from_str(&text).context("Google Calendar answered something that is not JSON")
  }

  async fn calendars(&self) -> Result<Vec<Calendar>> {
    let mut list = Vec::new();
    let mut page: Option<String> = None;
    loop {
      let mut query = vec![("maxResults", "250".to_string())];
      if let Some(token) = page.take() {
        query.push(("pageToken", token));
      }
      let answer = self.call(Method::GET, "users/me/calendarList", &query, None).await?;
      let items = answer["items"].as_array().map(Vec::as_slice).unwrap_or_default();
      list.extend(items.iter().filter_map(|entry| calendar_from(entry, &self.pastels)));
      match answer["nextPageToken"].as_str() {
        Some(token) => page = Some(token.to_string()),
        None => return Ok(list),
      }
    }
  }

  async fn events_of_all(
    self: Arc<Self>,
    calendars: &Arc<Vec<Calendar>>,
    start: Micros,
    end: Micros,
  ) -> Result<Vec<CalendarEvent>> {
    let mut tasks = tokio::task::JoinSet::new();
    for calendar in calendars.iter().filter(|calendar| calendar.access != CalendarAccess::FreeBusy) {
      let (session, calendar) = (self.clone(), calendar.clone());
      tasks.spawn(async move { session.events(&calendar, start, end).await });
    }
    let mut events = Vec::new();
    let mut failure = None;
    let mut read = 0_usize;
    while let Some(joined) = tasks.join_next().await {
      match joined {
        Ok(Ok(list)) => {
          read += 1;
          events.extend(list);
        },
        Ok(Err(error)) => {
          tracing::warn!("a calendar could not be read: {error:#}");
          failure = Some(error);
        },
        Err(error) => failure = Some(error.into()),
      }
    }
    match failure {
      Some(error) if read == 0 => Err(error),
      _ => Ok(events),
    }
  }

  async fn events(&self, calendar: &Calendar, start: Micros, end: Micros) -> Result<Vec<CalendarEvent>> {
    let path = format!("calendars/{}/events", urlencoding::encode(&calendar.id));
    let mut events = Vec::new();
    let mut page: Option<String> = None;
    loop {
      let mut query = vec![
        ("timeMin", rfc3339(start)?),
        ("timeMax", rfc3339(end)?),
        ("singleEvents", "true".to_string()),
        ("orderBy", "startTime".to_string()),
        ("maxResults", PAGE_SIZE.to_string()),
      ];
      if let Some(token) = page.take() {
        query.push(("pageToken", token));
      }
      let answer = match self.call(Method::GET, &path, &query, None).await {
        Ok(answer) => answer,
        Err(error) => return Err(error.context(format!("reading {}", calendar.name))),
      };
      let items = answer["items"].as_array().map(Vec::as_slice).unwrap_or_default();
      events.extend(items.iter().filter_map(|item| event_from(item, calendar)));
      match answer["nextPageToken"].as_str() {
        Some(token) => page = Some(token.to_string()),
        None => return Ok(events),
      }
    }
  }

  /// The event a change goes to: the occurrence itself, or its series with `EditScope::All`.
  async fn target(&self, calendar_id: &str, event_id: &str, scope: EditScope) -> Result<String> {
    if scope == EditScope::This {
      return Ok(event_id.to_string());
    }
    let event = self.call(Method::GET, &event_path(calendar_id, event_id), &[], None).await?;
    Ok(event["recurringEventId"].as_str().unwrap_or(event_id).to_string())
  }

  async fn save(&self, draft: &EventDraft) -> Result<()> {
    let notify = ("sendUpdates", send_updates(draft.notify));
    let conference = ("conferenceDataVersion", "1".to_string());
    let Some((calendar_id, event_id)) = &draft.editing else {
      let path = format!("calendars/{}/events", urlencoding::encode(&draft.calendar_id));
      let body = event_body(draft, None)?;
      self.call(Method::POST, &path, &[notify, conference], Some(&body)).await?;
      return Ok(());
    };
    let instance = self.call(Method::GET, &event_path(calendar_id, event_id), &[], None).await?;
    let series = instance["recurringEventId"].as_str().map(str::to_string);
    let (target, existing, draft) = match (&series, draft.scope) {
      (Some(series), EditScope::All) => {
        let master = self.call(Method::GET, &event_path(calendar_id, series), &[], None).await?;
        let shifted = shift_to_series(draft, &instance, &master)?;
        (series.clone(), master, shifted)
      },
      _ => (event_id.clone(), instance, draft.clone()),
    };
    let body = event_body(&draft, Some(&existing))?;
    let path = event_path(calendar_id, &target);
    self
      .call(Method::PATCH, &path, &[notify.clone(), conference], Some(&body))
      .await?;
    if draft.calendar_id != *calendar_id {
      if series.is_some() && draft.scope == EditScope::This {
        bail!("a single occurrence cannot move to another calendar; change all events instead");
      }
      let path = format!("{path}/move");
      let destination = ("destination", draft.calendar_id.clone());
      self.call(Method::POST, &path, &[destination, notify], None).await?;
    }
    Ok(())
  }
}

/// Orders events by start; on the same start, all-day ones first, then the longer ones.
pub(crate) fn sort_events(events: &mut [CalendarEvent]) {
  events.sort_by(|a, b| {
    a.start
      .cmp(&b.start)
      .then_with(|| b.all_day.cmp(&a.all_day))
      .then_with(|| b.end.cmp(&a.end))
      .then_with(|| a.title.cmp(&b.title))
  });
}

/// The `0xRRGGBB` of Google Calendar's event color `id`, 1 to 11.
pub(crate) fn event_color(id: u8) -> Option<u32> {
  EVENT_COLORS.iter().find(|(held, _, _)| *held == id).map(|(_, _, color)| *color)
}

fn event_path(calendar_id: &str, event_id: &str) -> String {
  format!(
    "calendars/{}/events/{}",
    urlencoding::encode(calendar_id),
    urlencoding::encode(event_id)
  )
}

fn send_updates(notify: bool) -> String {
  if notify { "all" } else { "none" }.to_string()
}

/// The SAPISID cookies of `.google.com`, by the label of their hash.
fn apisids(cookies: &[SessionCookie]) -> Vec<(&'static str, String)> {
  let find = |name: &str| {
    cookies
      .iter()
      .find(|cookie| cookie.name == name && cookie.domain.trim_start_matches('.') == "google.com")
      .map(|cookie| cookie.value.clone())
  };
  [
    ("SAPISIDHASH", "SAPISID"),
    ("SAPISID1PHASH", "__Secure-1PAPISID"),
    ("SAPISID3PHASH", "__Secure-3PAPISID"),
  ]
  .into_iter()
  .filter_map(|(label, name)| find(name).map(|value| (label, value)))
  .collect()
}

/// Every Google cookie of the session, each for the host it was issued for, so the redirects through
/// accounts.google.com can set the calendar's own.
fn jar(cookies: &[SessionCookie]) -> Jar {
  let jar = Jar::default();
  for cookie in cookies {
    let host = cookie.domain.trim_start_matches('.');
    if host != "google.com" && !host.ends_with(".google.com") {
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

/// The browser keys in the calendar page, in page order and without repeats.
fn browser_keys(page: &str) -> Vec<String> {
  static KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"AIzaSy[0-9A-Za-z_\-]{33}").expect("valid pattern"));
  let mut keys: Vec<String> = Vec::new();
  for found in KEY.find_iter(page) {
    if !keys.iter().any(|key| key == found.as_str()) {
      keys.push(found.as_str().to_string());
    }
  }
  keys
}

fn pastels(colors: &Value) -> HashMap<String, u32> {
  colors["calendar"]
    .as_object()
    .map(|palette| {
      palette
        .iter()
        .filter_map(|(id, color)| Some((id.clone(), hex(color["background"].as_str()?)?)))
        .collect()
    })
    .unwrap_or_default()
}

fn hex(text: &str) -> Option<u32> {
  u32::from_str_radix(text.trim_start_matches('#'), 16).ok()
}

/// The 1-based palette entry `id` of `palette`.
fn palette_color<T: Copy>(palette: &[T], id: &str) -> Option<T> {
  let index = id.parse::<usize>().ok()?.checked_sub(1)?;
  palette.get(index).copied()
}

fn calendar_from(entry: &Value, pastels: &HashMap<String, u32>) -> Option<Calendar> {
  if entry["deleted"].as_bool() == Some(true) || entry["hidden"].as_bool() == Some(true) {
    return None;
  }
  let id = entry["id"].as_str()?.to_string();
  let name = entry["summaryOverride"]
    .as_str()
    .or_else(|| entry["summary"].as_str())
    .unwrap_or(&id)
    .to_string();
  let color_id = entry["colorId"].as_str().unwrap_or_default();
  let modern = palette_color(&CALENDAR_COLORS, color_id);
  let background = entry["backgroundColor"].as_str().and_then(hex);
  // A palette color shows as the web app paints it; a color the user mixed shows as it is.
  let color = match background {
    Some(background) if pastels.get(color_id) == Some(&background) => modern.unwrap_or(background),
    Some(background) => background,
    None => modern.unwrap_or(DEFAULT_COLOR),
  };
  let access = match entry["accessRole"].as_str() {
    Some("owner") => CalendarAccess::Owner,
    Some("writer") => CalendarAccess::Writer,
    Some("freeBusyReader") => CalendarAccess::FreeBusy,
    _ => CalendarAccess::Reader,
  };
  Some(Calendar {
    id,
    name,
    color,
    access,
    primary: entry["primary"].as_bool() == Some(true),
    selected: entry["selected"].as_bool() == Some(true),
  })
}

/// Local midnight of `date`, or the first moment of it when a clock change skips midnight.
pub fn local_midnight(date: NaiveDate) -> Option<Micros> {
  let midnight = date.and_hms_opt(0, 0, 0)?;
  let one = date.and_hms_opt(1, 0, 0)?;
  Local
    .from_local_datetime(&midnight)
    .earliest()
    .or_else(|| Local.from_local_datetime(&one).earliest())
    .map(|at| at.timestamp_micros())
}

/// The local date an all-day boundary at `at` stands for. A few hours of slack keep a boundary moved by a clock change
/// on its day.
pub fn boundary_date(at: Micros) -> Option<NaiveDate> {
  let at = at.checked_add(3 * 3_600_000_000)?;
  Some(Local.timestamp_micros(at).single()?.date_naive())
}

fn rfc3339(at: Micros) -> Result<String> {
  let at = Local
    .timestamp_micros(at)
    .single()
    .ok_or_else(|| anyhow!("{at} is not a time"))?;
  Ok(at.to_rfc3339())
}

/// An event's start or end: the instant, and whether it is a date of an all-day event.
fn when(value: &Value) -> Option<(Micros, bool)> {
  if let Some(text) = value["dateTime"].as_str() {
    return Some((DateTime::parse_from_rfc3339(text).ok()?.timestamp_micros(), false));
  }
  let date = NaiveDate::parse_from_str(value["date"].as_str()?, "%Y-%m-%d").ok()?;
  Some((local_midnight(date)?, true))
}

fn rsvp_of(text: Option<&str>) -> Rsvp {
  match text {
    Some("accepted") => Rsvp::Accepted,
    Some("tentative") => Rsvp::Tentative,
    Some("declined") => Rsvp::Declined,
    _ => Rsvp::NeedsAction,
  }
}

const fn rsvp_name(rsvp: Rsvp) -> &'static str {
  match rsvp {
    Rsvp::NeedsAction => "needsAction",
    Rsvp::Accepted => "accepted",
    Rsvp::Tentative => "tentative",
    Rsvp::Declined => "declined",
  }
}

fn guest_from(attendee: &Value) -> Option<Guest> {
  if attendee["resource"].as_bool() == Some(true) {
    return None;
  }
  Some(Guest {
    email: attendee["email"].as_str()?.to_string(),
    name: attendee["displayName"].as_str().unwrap_or_default().to_string(),
    rsvp: rsvp_of(attendee["responseStatus"].as_str()),
    organizer: attendee["organizer"].as_bool() == Some(true),
    me: attendee["self"].as_bool() == Some(true),
    optional: attendee["optional"].as_bool() == Some(true),
  })
}

fn meet_url(item: &Value) -> Option<String> {
  if let Some(url) = item["hangoutLink"].as_str() {
    return Some(url.to_string());
  }
  item["conferenceData"]["entryPoints"]
    .as_array()?
    .iter()
    .find(|entry| entry["entryPointType"].as_str() == Some("video"))
    .and_then(|entry| entry["uri"].as_str())
    .map(str::to_string)
}

fn reminder_of(reminders: &Value) -> Reminder {
  if reminders["useDefault"].as_bool() != Some(false) {
    return Reminder::Default;
  }
  let first = reminders["overrides"]
    .as_array()
    .and_then(|list| list.iter().find_map(|entry| entry["minutes"].as_u64()))
    .and_then(|minutes| u32::try_from(minutes).ok());
  first.map_or(Reminder::None, Reminder::Minutes)
}

fn kind_of(item: &Value) -> EventKind {
  match item["eventType"].as_str() {
    Some("outOfOffice") => EventKind::OutOfOffice,
    Some("focusTime") => EventKind::FocusTime,
    Some("workingLocation") => EventKind::WorkingLocation,
    Some("birthday") => EventKind::Birthday,
    _ => EventKind::Event,
  }
}

fn event_from(item: &Value, calendar: &Calendar) -> Option<CalendarEvent> {
  if item["status"].as_str() == Some("cancelled") {
    return None;
  }
  let (start, all_day) = when(&item["start"])?;
  let (end, _) = when(&item["end"])?;
  let guests: Vec<Guest> = item["attendees"]
    .as_array()
    .map(|list| list.iter().filter_map(guest_from).collect())
    .unwrap_or_default();
  let organizer_me = item["organizer"]["self"].as_bool() == Some(true);
  let my_rsvp = guests
    .iter()
    .find(|guest| guest.me && !guest.organizer && !organizer_me)
    .map(|guest| guest.rsvp);
  let kind = kind_of(item);
  let writable = calendar.access.can_write();
  let can_edit = writable
    && kind != EventKind::Birthday
    && item["locked"].as_bool() != Some(true)
    && (organizer_me || item["guestsCanModify"].as_bool() == Some(true));
  let color_id = item["colorId"]
    .as_str()
    .and_then(|id| id.parse::<u8>().ok())
    .filter(|id| (1..=11).contains(id));
  let organizer = item["organizer"]["displayName"]
    .as_str()
    .or_else(|| item["organizer"]["email"].as_str())
    .unwrap_or_default()
    .to_string();
  Some(CalendarEvent {
    id: item["id"].as_str()?.to_string(),
    calendar_id: calendar.id.clone(),
    title: item["summary"]
      .as_str()
      .filter(|title| !title.trim().is_empty())
      .unwrap_or("(No title)")
      .to_string(),
    start,
    end: end.max(start),
    all_day,
    location: item["location"].as_str().unwrap_or_default().to_string(),
    description: plain_text(item["description"].as_str().unwrap_or_default()),
    meet_url: meet_url(item),
    link: item["htmlLink"].as_str().unwrap_or_default().to_string(),
    color: color_id.and_then(event_color),
    color_id,
    guests,
    organizer,
    series: item["recurringEventId"].as_str().map(str::to_string),
    kind,
    busy: item["transparency"].as_str() != Some("transparent"),
    visibility: match item["visibility"].as_str() {
      Some("public") => Visibility::Public,
      Some("private" | "confidential") => Visibility::Private,
      _ => Visibility::Default,
    },
    reminder: reminder_of(&item["reminders"]),
    can_edit,
    can_delete: writable && kind != EventKind::Birthday,
    my_rsvp,
  })
}

/// A description as plain text: the web app keeps them as HTML.
fn plain_text(html: &str) -> String {
  static BREAK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>|</p>|</div>|</li>").expect("valid pattern"));
  static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").expect("valid pattern"));
  if !html.contains('<') && !html.contains('&') {
    return html.to_string();
  }
  let text = BREAK.replace_all(html, "\n");
  let text = TAG.replace_all(&text, "");
  text
    .replace("&nbsp;", " ")
    .replace("&lt;", "<")
    .replace("&gt;", ">")
    .replace("&quot;", "\"")
    .replace("&#39;", "'")
    .replace("&amp;", "&")
    .trim()
    .to_string()
}

/// The start or end of a draft as the API takes it, clearing the other form so a patch can switch between them.
fn time_value(at: Micros, all_day: bool) -> Result<Value> {
  if all_day {
    let date = boundary_date(at).ok_or_else(|| anyhow!("{at} is not a date"))?;
    return Ok(json!({ "date": date.format("%Y-%m-%d").to_string(), "dateTime": null, "timeZone": null }));
  }
  Ok(json!({ "dateTime": rfc3339(at)?, "timeZone": crate::engine::device_time_zone(), "date": null }))
}

const fn weekday_code(day: Weekday) -> &'static str {
  match day {
    Weekday::Mon => "MO",
    Weekday::Tue => "TU",
    Weekday::Wed => "WE",
    Weekday::Thu => "TH",
    Weekday::Fri => "FR",
    Weekday::Sat => "SA",
    Weekday::Sun => "SU",
  }
}

/// The recurrence rule of `repeat` for an event starting at `start`; empty for `Repeat::Never`.
fn recurrence(repeat: Repeat, start: Micros) -> Result<Vec<String>> {
  let day = Local
    .timestamp_micros(start)
    .single()
    .ok_or_else(|| anyhow!("{start} is not a time"))?;
  let rule = match repeat {
    Repeat::Never => return Ok(Vec::new()),
    Repeat::Daily => "FREQ=DAILY".to_string(),
    Repeat::Weekly => format!("FREQ=WEEKLY;BYDAY={}", weekday_code(day.weekday())),
    Repeat::Monthly => format!("FREQ=MONTHLY;BYMONTHDAY={}", day.day()),
    Repeat::Yearly => "FREQ=YEARLY".to_string(),
    Repeat::Weekdays => "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".to_string(),
  };
  Ok(vec![format!("RRULE:{rule}")])
}

/// The guests of a draft: those already invited keep their entry and answer, the user's own entry stays, and the new
/// ones are added.
fn attendees(guests: &[String], existing: Option<&Value>) -> Vec<Value> {
  let held = existing
    .and_then(|event| event["attendees"].as_array())
    .cloned()
    .unwrap_or_default();
  let same =
    |attendee: &Value, email: &str| attendee["email"].as_str().is_some_and(|held| held.eq_ignore_ascii_case(email));
  let mut list: Vec<Value> = held
    .iter()
    .filter(|attendee| attendee["self"].as_bool() == Some(true) || attendee["resource"].as_bool() == Some(true))
    .cloned()
    .collect();
  for email in guests {
    if list.iter().any(|attendee| same(attendee, email)) {
      continue;
    }
    let entry = held
      .iter()
      .find(|attendee| same(attendee, email))
      .cloned()
      .unwrap_or_else(|| json!({ "email": email }));
    list.push(entry);
  }
  list
}

fn reminders_value(reminder: Reminder) -> Value {
  match reminder {
    Reminder::Default => json!({ "useDefault": true }),
    Reminder::None => json!({ "useDefault": false, "overrides": [] }),
    Reminder::Minutes(minutes) => {
      json!({ "useDefault": false, "overrides": [{ "method": "popup", "minutes": minutes }] })
    },
  }
}

/// The body that creates the event of `draft`, or with `existing` the patch that changes that event to it.
fn event_body(draft: &EventDraft, existing: Option<&Value>) -> Result<Value> {
  let mut body = Map::new();
  body.insert("summary".into(), json!(draft.title.trim()));
  body.insert("start".into(), time_value(draft.start, draft.all_day)?);
  body.insert("end".into(), time_value(draft.end.max(draft.start), draft.all_day)?);
  body.insert("transparency".into(), json!(if draft.busy { "opaque" } else { "transparent" }));
  let visibility = match draft.visibility {
    Visibility::Default => "default",
    Visibility::Public => "public",
    Visibility::Private => "private",
  };
  body.insert("visibility".into(), json!(visibility));
  body.insert("reminders".into(), reminders_value(draft.reminder));
  match draft.color_id {
    Some(id) => body.insert("colorId".into(), json!(id.to_string())),
    None => body.insert("colorId".into(), Value::Null),
  };
  // A description left as it was keeps the formatting the web app gave it.
  let held_description = existing.and_then(|event| event["description"].as_str()).map(plain_text);
  if held_description.as_deref() != Some(draft.description.trim()) {
    body.insert("description".into(), json!(draft.description.trim()));
  }
  let kind = existing.map_or(draft.kind, kind_of);
  if kind == EventKind::Event {
    body.insert("location".into(), json!(draft.location.trim()));
    body.insert("attendees".into(), Value::Array(attendees(&draft.guests, existing)));
    let has_meet = existing.and_then(meet_url).is_some();
    if draft.add_meet && !has_meet {
      let request = format!("working-{}", rand::random::<u64>());
      body.insert(
        "conferenceData".into(),
        json!({ "createRequest": { "requestId": request, "conferenceSolutionKey": { "type": "hangoutsMeet" } } }),
      );
    }
  }
  // An occurrence keeps the rule of its series.
  let occurrence = existing.is_some_and(|event| event["recurringEventId"].is_string());
  if let Some(repeat) = draft.repeat
    && !occurrence
  {
    let had_rule = existing.is_some_and(|event| event["recurrence"].is_array());
    let rule = recurrence(repeat, draft.start)?;
    if !rule.is_empty() || had_rule {
      body.insert("recurrence".into(), json!(rule));
    }
  }
  if existing.is_none() {
    match draft.kind {
      EventKind::OutOfOffice => {
        body.insert("eventType".into(), json!("outOfOffice"));
        body.insert(
          "outOfOfficeProperties".into(),
          json!({ "autoDeclineMode": "declineOnlyNewConflictingInvitations" }),
        );
      },
      EventKind::FocusTime => {
        body.insert("eventType".into(), json!("focusTime"));
        body.insert(
          "focusTimeProperties".into(),
          json!({ "autoDeclineMode": "declineOnlyNewConflictingInvitations", "chatStatus": "doNotDisturb" }),
        );
      },
      EventKind::Event | EventKind::WorkingLocation | EventKind::Birthday => {},
    }
  }
  Ok(Value::Object(body))
}

/// `draft`, made for an occurrence, moved onto its series: the series keeps its own first date and takes the times the
/// occurrence moved by.
fn shift_to_series(draft: &EventDraft, instance: &Value, master: &Value) -> Result<EventDraft> {
  let (instance_start, _) = when(&instance["start"]).ok_or_else(|| anyhow!("the occurrence has no start"))?;
  let (instance_end, _) = when(&instance["end"]).ok_or_else(|| anyhow!("the occurrence has no end"))?;
  let (master_start, _) = when(&master["start"]).ok_or_else(|| anyhow!("the series has no start"))?;
  let (master_end, _) = when(&master["end"]).ok_or_else(|| anyhow!("the series has no end"))?;
  let mut shifted = draft.clone();
  shifted.start = master_start + (draft.start - instance_start);
  shifted.end = master_end + (draft.end - instance_end);
  Ok(shifted)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn calendar(access: CalendarAccess) -> Calendar {
    Calendar {
      id: "jpark@example.com".into(),
      name: "Jordan Park".into(),
      color: DEFAULT_COLOR,
      access,
      primary: true,
      selected: true,
    }
  }

  #[test]
  fn browser_keys_keep_page_order_without_repeats() {
    let a = format!("AIzaSy{}", "a".repeat(33));
    let b = format!("AIzaSy{}", "b".repeat(33));
    let page = format!(r#"["{b}",x,"{a}","{b}"]"#);
    assert_eq!(browser_keys(&page), vec![b, a]);
  }

  #[test]
  fn palette_colors_paint_as_the_web_app_and_custom_ones_stay() {
    let pastels = HashMap::from([("15".to_string(), 0x9fc6e7)]);
    let palette =
      json!({ "id": "c", "summary": "Team", "colorId": "15", "backgroundColor": "#9fc6e7", "accessRole": "reader" });
    let custom =
      json!({ "id": "c", "summary": "Team", "colorId": "15", "backgroundColor": "#123456", "accessRole": "reader" });
    assert_eq!(calendar_from(&palette, &pastels).map(|c| c.color), Some(0x4285f4));
    assert_eq!(calendar_from(&custom, &pastels).map(|c| c.color), Some(0x123456));
  }

  #[test]
  fn timed_and_all_day_events_parse() {
    let timed = json!({
      "id": "e1", "summary": "Design review", "status": "confirmed",
      "start": { "dateTime": "2026-11-16T10:00:00Z" }, "end": { "dateTime": "2026-11-16T10:30:00Z" },
      "hangoutLink": "https://meet.google.com/abc-defg-hij",
      "organizer": { "email": "jpark@example.com", "self": true },
      "attendees": [
        { "email": "jpark@example.com", "self": true, "organizer": true, "responseStatus": "accepted" },
        { "email": "rbrooks@example.com", "displayName": "Riley Brooks", "responseStatus": "tentative" }
      ],
      "colorId": "11", "description": "Agenda<br>1. <b>Scope</b> &amp; plan"
    });
    let event = event_from(&timed, &calendar(CalendarAccess::Owner)).expect("parses");
    assert_eq!(event.start, 1_794_823_200_000_000);
    assert_eq!(event.end - event.start, 30 * 60 * 1_000_000);
    assert!(!event.all_day && event.can_edit && event.my_rsvp.is_none());
    assert_eq!(event.color, Some(0xd50000));
    assert_eq!(event.description, "Agenda\n1. Scope & plan");
    assert_eq!(event.guests.len(), 2);
    assert_eq!(event.meet_url.as_deref(), Some("https://meet.google.com/abc-defg-hij"));

    let day =
      json!({ "id": "e2", "summary": "Holiday", "start": { "date": "2026-11-26" }, "end": { "date": "2026-11-27" } });
    let event = event_from(&day, &calendar(CalendarAccess::Reader)).expect("parses");
    assert!(event.all_day && !event.can_edit && !event.can_delete);
    assert_eq!(boundary_date(event.start), NaiveDate::from_ymd_opt(2026, 11, 26));
    assert_eq!(boundary_date(event.end), NaiveDate::from_ymd_opt(2026, 11, 27));
  }

  #[test]
  fn invited_events_carry_the_users_answer() {
    let invite = json!({
      "id": "e3", "start": { "dateTime": "2026-11-16T10:00:00Z" }, "end": { "dateTime": "2026-11-16T11:00:00Z" },
      "organizer": { "email": "rbrooks@example.com" },
      "attendees": [{ "email": "jpark@example.com", "self": true, "responseStatus": "needsAction" }]
    });
    let event = event_from(&invite, &calendar(CalendarAccess::Owner)).expect("parses");
    assert_eq!(event.my_rsvp, Some(Rsvp::NeedsAction));
    assert_eq!(event.title, "(No title)");
    assert!(!event.can_edit && event.can_delete);
  }

  #[test]
  fn cancelled_occurrences_are_left_out() {
    let gone =
      json!({ "id": "e4", "status": "cancelled", "start": { "date": "2026-11-26" }, "end": { "date": "2026-11-27" } });
    assert!(event_from(&gone, &calendar(CalendarAccess::Owner)).is_none());
  }

  #[test]
  fn new_event_body_has_meet_rule_and_guests() {
    let start =
      local_midnight(NaiveDate::from_ymd_opt(2026, 11, 19).expect("date")).expect("midnight") + 9 * 3_600_000_000;
    let draft = EventDraft {
      calendar_id: "primary".into(),
      title: " Planning ".into(),
      start,
      end: start + 3_600_000_000,
      guests: vec!["rbrooks@example.com".into()],
      add_meet: true,
      repeat: Some(Repeat::Weekly),
      busy: true,
      ..EventDraft::default()
    };
    let body = event_body(&draft, None).expect("body");
    assert_eq!(body["summary"], "Planning");
    assert_eq!(body["recurrence"], json!(["RRULE:FREQ=WEEKLY;BYDAY=TH"]));
    assert_eq!(body["attendees"], json!([{ "email": "rbrooks@example.com" }]));
    assert_eq!(
      body["conferenceData"]["createRequest"]["conferenceSolutionKey"]["type"],
      "hangoutsMeet"
    );
    assert_eq!(body["transparency"], "opaque");
    assert!(body["start"]["dateTime"].is_string() && body["start"]["date"].is_null());
  }

  #[test]
  fn edits_keep_answers_and_formatted_descriptions() {
    let existing = json!({
      "description": "<b>Notes</b>",
      "hangoutLink": "https://meet.google.com/abc-defg-hij",
      "attendees": [
        { "email": "jpark@example.com", "self": true, "responseStatus": "accepted" },
        { "email": "rbrooks@example.com", "responseStatus": "accepted" },
        { "email": "gone@example.com", "responseStatus": "declined" }
      ]
    });
    let draft = EventDraft {
      description: "Notes".into(),
      guests: vec!["rbrooks@example.com".into(), "new@example.com".into()],
      add_meet: true,
      all_day: true,
      ..EventDraft::default()
    };
    let body = event_body(&draft, Some(&existing)).expect("body");
    assert!(body.get("description").is_none());
    assert!(body.get("conferenceData").is_none());
    assert_eq!(
      body["attendees"],
      json!([
        { "email": "jpark@example.com", "self": true, "responseStatus": "accepted" },
        { "email": "rbrooks@example.com", "responseStatus": "accepted" },
        { "email": "new@example.com" }
      ])
    );
    assert!(body["start"]["date"].is_string() && body["start"]["dateTime"].is_null());
  }

  #[test]
  fn a_series_change_keeps_the_series_first_date() {
    let hour = 3_600_000_000;
    let instance =
      json!({ "start": { "dateTime": "2026-11-18T10:00:00Z" }, "end": { "dateTime": "2026-11-18T10:30:00Z" } });
    let master =
      json!({ "start": { "dateTime": "2026-09-02T10:00:00Z" }, "end": { "dateTime": "2026-09-02T10:30:00Z" } });
    let (start, _) = when(&instance["start"]).expect("start");
    let draft = EventDraft {
      start: start + hour,
      end: start + hour + 3 * hour / 2,
      ..EventDraft::default()
    };
    let shifted = shift_to_series(&draft, &instance, &master).expect("shift");
    let (master_start, _) = when(&master["start"]).expect("start");
    assert_eq!(shifted.start, master_start + hour);
    assert_eq!(shifted.end - shifted.start, 3 * hour / 2);
  }
}
