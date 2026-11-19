//! Authenticated HTTP to chat.google.com: cookie session, XSRF token, and protobuf API calls.
#![allow(
  clippy::unreadable_literal,
  clippy::string_slice,
  clippy::indexing_slicing,
  clippy::as_conversions,
  reason = "bootstrap page scraping and protobuf request headers"
)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use prost::Message;
use reqwest::cookie::{CookieStore, Jar};
use reqwest::header::HeaderValue;
use serde::Deserialize;

use crate::proto;

const ORIGIN: &str = "https://chat.google.com";
const API_BASE: &str = "https://chat.google.com/u/0/api";
/// Public web client key the Chat web app sends with every API call.
const API_KEY: &str = "AIzaSyD7InnYR3VKdb4j2rMUEbTCIr2VyEazl6k";
pub(crate) const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36";

/// One cookie of the signed-in Google session.
#[derive(Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct SessionCookie {
  /// Cookie name.
  pub name: String,
  /// Cookie value.
  pub value: String,
  /// Cookie domain.
  pub domain: String,
  /// Cookie path. Defaults to `/`.
  #[serde(default = "root_path")]
  pub path: String,
}

/// Reads session cookies from the JSON file the app or `scripts/login.sh` writes.
pub fn read_cookies(path: &std::path::Path) -> Result<Vec<SessionCookie>> {
  let text = std::fs::read_to_string(path).map_err(|_| SignedOut("Sign in to start.".into()))?;
  serde_json::from_str(&text).context("cookies.json is not valid")
}

/// Puts the chat cookies of the file at `path` into `jar`, replacing same-named ones.
fn load_cookies(jar: &Jar, path: &std::path::Path) -> Result<()> {
  let url = ORIGIN.parse::<reqwest::Url>()?;
  for cookie in &read_cookies(path)? {
    let domain = cookie.domain.trim_start_matches('.');
    // The file also holds cookies of other Google hosts, kept for Meet calls; the chat API never sends those.
    if domain != "google.com" && domain != "chat.google.com" {
      continue;
    }
    // `__Host-` cookies must not carry a Domain attribute; everything else keeps the one it was issued for.
    let line = if cookie.name.starts_with("__Host-") || domain == "chat.google.com" {
      format!("{}={}; Path={}; Secure", cookie.name, cookie.value, cookie.path)
    } else {
      format!(
        "{}={}; Domain={}; Path={}; Secure",
        cookie.name, cookie.value, domain, cookie.path
      )
    };
    jar.add_cookie_str(&line, &url);
  }
  Ok(())
}

/// The chat session's cookie jar, which also keeps what Google sets while the session runs (it refreshes
/// cookies such as `SIDCC` on ordinary answers), until `Http::save_refreshed` writes that to the session file.
/// Without it the file would hold the cookies of the sign-in for as long as the session lasts.
#[derive(Default)]
struct SessionJar {
  jar: Jar,
  refreshed: Mutex<Vec<SessionCookie>>,
}

impl SessionJar {
  fn refreshed(&self) -> std::sync::MutexGuard<'_, Vec<SessionCookie>> {
    self.refreshed.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
  }
}

impl CookieStore for SessionJar {
  fn set_cookies(&self, cookie_headers: &mut dyn Iterator<Item = &HeaderValue>, url: &reqwest::Url) {
    let headers: Vec<&HeaderValue> = cookie_headers.collect();
    {
      let mut refreshed = self.refreshed();
      let set = headers.iter().filter_map(|header| set_cookie(header.to_str().ok()?, url));
      for cookie in set {
        refreshed.retain(|held| !same_cookie(held, &cookie));
        refreshed.push(cookie);
      }
    }
    self.jar.set_cookies(&mut headers.into_iter(), url);
  }

  fn cookies(&self, url: &reqwest::Url) -> Option<HeaderValue> {
    self.jar.cookies(url)
  }
}

/// Two cookies are one when name, domain, and path agree.
pub(crate) fn same_cookie(a: &SessionCookie, b: &SessionCookie) -> bool {
  a.name == b.name && a.domain == b.domain && a.path == b.path
}

/// Reads a `Set-Cookie` header from `url` as a cookie of the session file: one for `chat.google.com` alone or
/// for every `google.com` host, written as the file writes them. A header that takes a cookie away, or sets one
/// for any other host, reads as none, and so does one for `google.com` that a host outside it sent: the file
/// keeps what this reads, whatever the requests' own jar refuses.
fn set_cookie(header: &str, url: &reqwest::Url) -> Option<SessionCookie> {
  let mut parts = header.split(';').map(str::trim);
  let (name, value) = parts.next()?.split_once('=')?;
  let mut domain = None;
  let mut path = None;
  for part in parts {
    let (key, setting) = part.split_once('=').unwrap_or((part, ""));
    match key.to_ascii_lowercase().as_str() {
      "domain" => domain = Some(setting.trim_start_matches('.').to_ascii_lowercase()),
      "path" => path = Some(setting),
      // How a cookie is taken away: no time left to live, or an expiry at the start of the epoch.
      "max-age" if setting.parse::<i64>().is_ok_and(|seconds| seconds <= 0) => return None,
      "expires" if setting.contains("1970") => return None,
      _ => {},
    }
  }
  if name.is_empty() || value.is_empty() {
    return None;
  }
  let domain = match domain.as_deref() {
    Some("google.com") if is_google(url) => ".google.com",
    None if url.host_str() == Some("chat.google.com") => "chat.google.com",
    _ => return None,
  };
  Some(SessionCookie {
    name: name.into(),
    value: value.into(),
    domain: domain.into(),
    path: path.filter(|path| !path.is_empty()).unwrap_or("/").into(),
  })
}

/// Whether `url` is an HTTPS address of `google.com` or a host under it: the only ones that set a cookie for
/// every `google.com` host.
fn is_google(url: &reqwest::Url) -> bool {
  url.scheme() == "https"
    && url
      .host_str()
      .is_some_and(|host| host == "google.com" || host.ends_with(".google.com"))
}

fn root_path() -> String {
  "/".into()
}

/// The session was refused: the cookies are missing, expired, or signed out.
#[derive(Debug)]
pub struct SignedOut(pub String);

impl std::fmt::Display for SignedOut {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(&self.0)
  }
}

impl std::error::Error for SignedOut {}

pub struct Http {
  /// Short requests: API calls and downloads.
  client: reqwest::Client,
  /// Long-lived streaming requests, without an overall timeout.
  stream_client: reqwest::Client,
  /// The session cookies, shared with both clients, and the file they come from. A Meet call rotates the
  /// session's cookies and saves the new ones to that file; they are read again when the old ones stop working.
  jar: Arc<SessionJar>,
  cookie_path: std::path::PathBuf,
  xsrf: RwLock<Xsrf>,
  /// One bootstrap read at a time: callers that wait on it take what the read before them found.
  refreshing: tokio::sync::Mutex<()>,
  /// Token the web app's page RPCs require, read with the XSRF token.
  page_token: RwLock<Option<String>>,
  request_id: AtomicU64,
  /// The session was refused and `session.log` has a line about it: one line a sign-out, however many
  /// requests run into it.
  refused: AtomicBool,
}

impl Http {
  pub fn from_cookie_file(path: &std::path::Path) -> Result<Self> {
    let jar = Arc::new(SessionJar::default());
    load_cookies(&jar.jar, path)?;

    // No default Origin or Referer: with them, attachment URLs answer 204 instead of redirecting to the file.
    let builder = || {
      reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .cookie_provider(jar.clone())
        .connect_timeout(Duration::from_secs(15))
        .pool_idle_timeout(Duration::from_secs(120))
        .tcp_nodelay(true)
    };
    Ok(Self {
      client: builder().timeout(Duration::from_secs(45)).build()?,
      stream_client: builder().build()?,
      jar,
      cookie_path: path.to_path_buf(),
      xsrf: RwLock::new(Xsrf::default()),
      refreshing: tokio::sync::Mutex::new(()),
      page_token: RwLock::new(None),
      request_id: AtomicU64::new(1),
      refused: AtomicBool::new(false),
    })
  }

  /// Takes the session file's cookies again, after a sign-in or a Meet call wrote it. What Google set for the
  /// session held before is dropped: it is not the file's session any more.
  pub fn reload_cookies(&self) -> Result<()> {
    self.jar.refreshed().clear();
    load_cookies(&self.jar.jar, &self.cookie_path)
  }

  /// Writes the cookies Google set since the last call into the session file, so the next launch, and the next
  /// Meet call, start from the session as it is now. Does nothing while there is no file: signed out.
  pub fn save_refreshed(&self) {
    let refreshed = std::mem::take(&mut *self.jar.refreshed());
    if refreshed.is_empty() || !self.cookie_path.is_file() {
      return;
    }
    if let Err(error) = crate::merge_session_cookies(&self.cookie_path, refreshed) {
      tracing::warn!("could not save the session's refreshed cookies: {error}");
    }
  }

  pub const fn stream_client(&self) -> &reqwest::Client {
    &self.stream_client
  }

  pub fn xsrf(&self) -> Option<String> {
    self
      .xsrf
      .read()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .token
      .clone()
  }

  /// The token held and the generation it belongs to, read together.
  fn xsrf_state(&self) -> (Option<String>, u64) {
    let xsrf = self.xsrf.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    (xsrf.token.clone(), xsrf.generation)
  }

  pub fn set_xsrf(&self, token: String) {
    let mut xsrf = self.xsrf.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    xsrf.token = Some(token);
    xsrf.refused = None;
    xsrf.generation += 1;
  }

  /// Reads the XSRF token from the web app's bootstrap page. Also the cheapest check that the session is alive.
  /// A read already under way is waited for and its answer taken, rather than a second one made.
  pub async fn refresh_xsrf(&self) -> Result<String> {
    let (_, generation) = self.xsrf_state();
    self.refresh_since(generation).await
  }

  /// Reads the bootstrap page again unless a read finished after the token of `seen` was held: then its new
  /// token, or its refusal, is the answer.
  async fn refresh_since(&self, seen: u64) -> Result<String> {
    let _refreshing = self.refreshing.lock().await;
    {
      let xsrf = self.xsrf.read().unwrap_or_else(std::sync::PoisonError::into_inner);
      if xsrf.generation != seen {
        if let Some(reason) = &xsrf.refused {
          return Err(SignedOut(reason.clone()).into());
        }
        if let Some(token) = &xsrf.token {
          return Ok(token.clone());
        }
      }
    }
    let result = match self.read_bootstrap().await {
      Err(error) if error.downcast_ref::<SignedOut>().is_some() => {
        // The cookies in use were rotated away (by a Meet call): take the saved ones and try once more.
        load_cookies(&self.jar.jar, &self.cookie_path)?;
        self.read_bootstrap().await
      },
      result => result,
    };
    match &result {
      Ok(_) => self.refused.store(false, Ordering::Relaxed),
      Err(error) => {
        if let Some(SignedOut(reason)) = error.downcast_ref::<SignedOut>() {
          let mut xsrf = self.xsrf.write().unwrap_or_else(std::sync::PoisonError::into_inner);
          xsrf.refused = Some(reason.clone());
          xsrf.generation += 1;
          drop(xsrf);
          self.note_refused();
        }
      },
    }
    result
  }

  /// Google refused the session, with the saved cookies too. `session.log`, beside the session file, gets a line
  /// with when, and how long ago the session file was written: a file written moments ago was refused by Google
  /// while fresh, an old one may only have gone stale. Never a cookie's name or value.
  fn note_refused(&self) {
    if self.refused.swap(true, Ordering::Relaxed) {
      return;
    }
    let written = std::fs::metadata(&self.cookie_path)
      .and_then(|file| file.modified())
      .ok()
      .and_then(|at| at.elapsed().ok())
      .map_or_else(|| "at an unknown time".to_string(), |age| format!("{} s before", age.as_secs()));
    let line = format!(
      "{} Google refused the session; the session file was written {written}\n",
      chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    tracing::warn!("{}", line.trim_end());
    let log = std::fs::OpenOptions::new()
      .create(true)
      .append(true)
      .open(self.cookie_path.with_file_name("session.log"));
    if let Ok(mut log) = log {
      let _ = std::io::Write::write_all(&mut log, line.as_bytes());
    }
  }

  async fn read_bootstrap(&self) -> Result<String> {
    let hs = r#"["h_hs",null,null,[1,0],null,null,"gmail.pinto-server_20230730.06_p0",1,null,[15,38,36,35,26,30,41,18,24,11,21,14,6],null,null,"3Mu86PSulM4.en..es5",0,null,null,[0]]"#;
    let response = self
      .client
      .get("https://chat.google.com/u/0/mole/world")
      .query(&[
        ("origin", "https://mail.google.com"),
        ("shell", "9"),
        ("hl", "en"),
        ("wfi", "gtn-roster-iframe-id"),
        ("hs", hs),
      ])
      .header("referer", "https://mail.google.com/")
      .send()
      .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
      bail!("bootstrap page answered {status}");
    }
    let marker = ">window.WIZ_global_data = ";
    let start = body
      .find(marker)
      .ok_or_else(|| anyhow!("bootstrap page has no WIZ_global_data"))?
      + marker.len();
    let end = body[start..]
      .find(";</script>")
      .ok_or_else(|| anyhow!("bootstrap data is not terminated"))?
      + start;
    let wiz: serde_json::Value = serde_json::from_str(&body[start..end]).context("bootstrap data is not JSON")?;
    if wiz["qwAQke"].as_str() == Some("AccountsSignInUi") {
      return Err(SignedOut("The Google session expired. Sign in again.".into()).into());
    }
    let token = wiz["SMqcke"]
      .as_str()
      .ok_or_else(|| anyhow!("bootstrap data has no XSRF token"))?
      .to_string();
    self.set_xsrf(token.clone());
    *self.page_token.write().unwrap_or_else(std::sync::PoisonError::into_inner) =
      wiz["SNlM0e"].as_str().map(str::to_string);
    Ok(token)
  }

  /// Calls one of the web app's page RPCs (`batchexecute`), which carry what the protobuf API lacks, such as
  /// message search. `arguments` is the RPC's JSON argument list; the answer is its JSON payload.
  pub async fn page_rpc(&self, rpc: &str, arguments: &serde_json::Value) -> Result<serde_json::Value> {
    let held = self
      .page_token
      .read()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .clone();
    let token = if let Some(token) = held {
      token
    } else {
      self.refresh_xsrf().await?;
      self
        .page_token
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .ok_or_else(|| anyhow!("bootstrap data has no page token"))?
    };
    let request = serde_json::json!([[[rpc, arguments.to_string(), null, "generic"]]]).to_string();
    let body = format!("f.req={}&at={}", urlencoding::encode(&request), urlencoding::encode(&token));
    let response = self
      .client
      .post("https://chat.google.com/u/0/_/DynamiteWebUi/data/batchexecute")
      .query(&[("rpcids", rpc), ("hl", "en"), ("rt", "c")])
      .header("content-type", "application/x-www-form-urlencoded;charset=UTF-8")
      .header("x-same-domain", "1")
      .header("referer", "https://chat.google.com/")
      .body(body)
      .send()
      .await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
      // The page token expires with the page; the next call reads a new one.
      *self.page_token.write().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
      bail!("{rpc} answered {status}");
    }
    // The answer is a stream of length-prefixed JSON lines; the one for this RPC starts with `[["wrb.fr"`.
    // Writes such as do-not-disturb often have no payload; the HTTP status is the answer.
    for line in text.lines().filter(|line| line.starts_with("[[\"wrb.fr\"")) {
      let envelope: serde_json::Value = serde_json::from_str(line)?;
      if let Some(payload) = envelope[0][2].as_str() {
        return Ok(serde_json::from_str(payload)?);
      }
    }
    Ok(serde_json::Value::Null)
  }

  /// Calls a Chat API endpoint with a binary protobuf body and decodes the protobuf answer.
  pub async fn api<Req: Message, Resp: Message + Default>(&self, endpoint: &str, request: &Req) -> Result<Resp> {
    let body = prost::bytes::Bytes::from(request.encode_to_vec());
    let result = self.api_once(endpoint, body.clone()).await;
    match stale(&result) {
      Some(seen) => {
        self.refresh_since(seen).await?;
        self.api_once(endpoint, body).await
      },
      None => result,
    }
  }

  /// The token to send and its generation, read from the bootstrap page when none is held.
  async fn token(&self) -> Result<(String, u64)> {
    match self.xsrf_state() {
      (Some(token), generation) => Ok((token, generation)),
      (None, generation) => {
        self.refresh_since(generation).await?;
        match self.xsrf_state() {
          (Some(token), generation) => Ok((token, generation)),
          (None, _) => bail!("bootstrap data has no XSRF token"),
        }
      },
    }
  }

  async fn api_once<Resp: Message + Default>(&self, endpoint: &str, body: prost::bytes::Bytes) -> Result<Resp> {
    let (token, generation) = self.token().await?;
    let counter = self.request_id.fetch_add(1, Ordering::Relaxed).to_string();
    let response = self
      .client
      .post(format!("{API_BASE}/{endpoint}"))
      .query(&[("c", counter.as_str()), ("rt", "b"), ("alt", "proto"), ("key", API_KEY)])
      .header("content-type", "application/x-protobuf")
      .header("x-framework-xsrf-token", token)
      .header("x-goog-encode-response-if-executable", "base64")
      .body(body)
      .send()
      .await?;
    let status = response.status();
    let base64_encoded = response
      .headers()
      .get("x-goog-safety-encoding")
      .is_some_and(|value| value == "base64");
    let bytes = response.bytes().await?;
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
      return Err(StaleToken(generation).into());
    }
    if !status.is_success() {
      let body = String::from_utf8_lossy(&bytes);
      let body = body.chars().take(300).collect::<String>();
      bail!("{endpoint} answered {status}: {body}");
    }
    let decoded = if base64_encoded {
      Resp::decode(BASE64.decode(&bytes)?.as_slice())
    } else {
      Resp::decode(bytes)
    };
    decoded.with_context(|| format!("{endpoint} answer does not decode"))
  }

  /// Calls a Chat API endpoint the way the web app does for requests the protobuf schema here cannot express: the
  /// body and the answer are the positional JSON form of the protobuf messages (index `n` is field `n + 1`).
  pub async fn api_json(&self, endpoint: &str, request: &serde_json::Value) -> Result<serde_json::Value> {
    let result = self.api_json_once(endpoint, request).await;
    match stale(&result) {
      Some(seen) => {
        self.refresh_since(seen).await?;
        self.api_json_once(endpoint, request).await
      },
      None => result,
    }
  }

  async fn api_json_once(&self, endpoint: &str, request: &serde_json::Value) -> Result<serde_json::Value> {
    let (token, generation) = self.token().await?;
    let counter = self.request_id.fetch_add(1, Ordering::Relaxed).to_string();
    let response = self
      .client
      .post(format!("{API_BASE}/{endpoint}"))
      .query(&[("c", counter.as_str())])
      .header("content-type", "application/json")
      .header("x-framework-xsrf-token", token)
      .header("referer", "https://chat.google.com/")
      .body(request.to_string())
      .send()
      .await?;
    let status = response.status();
    let text = response.text().await?;
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
      return Err(StaleToken(generation).into());
    }
    if !status.is_success() {
      let body = text.chars().take(300).collect::<String>();
      bail!("{endpoint} answered {status}: {body}");
    }
    // The answer starts with an anti-JSON-hijacking line.
    let json = text.trim_start_matches(")]}'").trim_start();
    serde_json::from_str(json).with_context(|| format!("{endpoint} answer is not JSON"))
  }

  /// Plain authenticated GET, following redirects.
  pub async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
    let response = self.client.get(url).send().await?;
    let status = response.status();
    if !status.is_success() {
      bail!("GET answered {status}");
    }
    // Reuses the answer's buffer when it holds it alone.
    Ok(Vec::from(response.bytes().await?))
  }

  /// Uploads a file to attach to a message in the conversation with the plain id `group`. The answer is what
  /// the message carries as its attachment.
  pub async fn upload(&self, group: &str, name: &str, mime: &str, bytes: Vec<u8>) -> Result<proto::UploadMetadata> {
    // Two steps, as the web app does: ask where the bytes go, then put them there.
    let start = self
      .client
      .post("https://chat.google.com/uploads")
      .query(&[("group_id", group), ("key", API_KEY)])
      .header("x-goog-upload-protocol", "resumable")
      .header("x-goog-upload-command", "start")
      .header("x-goog-upload-content-length", bytes.len())
      .header("x-goog-upload-content-type", mime)
      .header(
        "x-goog-upload-file-name",
        reqwest::header::HeaderValue::from_bytes(name.as_bytes())?,
      )
      .send()
      .await?;
    if !start.status().is_success() {
      bail!("the upload was refused ({})", start.status());
    }
    let target = start.headers().get("x-goog-upload-url").and_then(|value| value.to_str().ok());
    let target = target
      .ok_or_else(|| anyhow!("the upload was not given a destination"))?
      .to_string();
    // No overall timeout: a large file takes as long as the connection needs.
    let sent = self
      .stream_client
      .put(target)
      .header("x-goog-upload-protocol", "resumable")
      .header("x-goog-upload-command", "upload, finalize")
      .header("x-goog-upload-offset", "0")
      .body(bytes)
      .send()
      .await?;
    let status = sent.status();
    let answer = sent.bytes().await?;
    if !status.is_success() {
      bail!("the upload failed ({status})");
    }
    let decoded = BASE64.decode(&answer).context("the upload answer is not base64")?;
    proto::UploadMetadata::decode(decoded.as_slice()).context("the upload answer does not decode")
  }
}

/// The API refused the XSRF token of the generation it carries.
#[derive(Debug)]
struct StaleToken(u64);

impl std::fmt::Display for StaleToken {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str("the request token was refused")
  }
}

impl std::error::Error for StaleToken {}

/// The generation of the token the API refused, when that is why `result` failed.
fn stale<T>(result: &Result<T>) -> Option<u64> {
  result.as_ref().err()?.downcast_ref::<StaleToken>().map(|stale| stale.0)
}

/// The XSRF token, with a generation that moves each time the token changes or a bootstrap read is refused.
#[derive(Default)]
struct Xsrf {
  token: Option<String>,
  generation: u64,
  /// Why the last bootstrap read was refused, until a token arrives.
  refused: Option<String>,
}

/// Header every API request carries: who we are and which features the answers may use.
pub fn request_header() -> proto::RequestHeader {
  use proto::client_feature_capabilities::CapabilityLevel::FullySupported;
  let full = Some(FullySupported as i32);
  proto::RequestHeader {
    client_type: Some(proto::request_header::ClientType::Web as i32),
    client_version: Some(2440378181258),
    client_feature_capabilities: Some(proto::ClientFeatureCapabilities {
      // `tombstone_level` is left out on purpose: with it, `list_topics` answers 500 for spaces.
      spam_room_invites_level: full,
      rich_text_viewing_level: full,
      quoted_message_support_level: full,
      custom_hyperlink_level: full,
      snippets_for_named_rooms: full,
      longer_group_snippets_level: full,
      can_handle_batch_reaction_update: full,
      dm_inline_threading_support_level: full,
      named_gdms_support_level: full,
      // Without it, messages come back without their labels, and so without the user's star.
      starred_shortcut_level: full,
      ..Default::default()
    }),
    ..Default::default()
  }
}

#[cfg(test)]
mod tests {
  use reqwest::cookie::CookieStore;
  use reqwest::header::HeaderValue;

  use super::{SessionJar, set_cookie};

  fn from(host: &str, header: &str) -> Option<(String, String, String, String)> {
    let url = format!("https://{host}/u/0/").parse().expect("url");
    set_cookie(header, &url).map(|cookie| (cookie.name, cookie.value, cookie.domain, cookie.path))
  }

  #[test]
  fn set_cookie_reads_what_the_session_file_holds() {
    let all = |name: &str, value: &str, domain: &str| Some((name.into(), value.into(), domain.into(), "/".into()));
    assert_eq!(
      from(
        "chat.google.com",
        "SIDCC=a=b; expires=Tue, 16-Nov-2027 03:20:21 GMT; path=/; domain=.google.com"
      ),
      all("SIDCC", "a=b", ".google.com")
    );
    // No Domain: the cookie of the host that set it.
    assert_eq!(
      from("chat.google.com", "COMPASS=x; path=/; Secure; HttpOnly"),
      all("COMPASS", "x", "chat.google.com")
    );
    assert_eq!(
      from("chat.google.com", "OSID=x; Domain=google.com"),
      all("OSID", "x", ".google.com")
    );
  }

  #[test]
  fn set_cookie_skips_removals_and_other_hosts() {
    assert_eq!(from("chat.google.com", "SID=; path=/; domain=.google.com"), None);
    assert_eq!(from("chat.google.com", "SID=x; Max-Age=0; domain=.google.com"), None);
    assert_eq!(
      from(
        "chat.google.com",
        "SID=EXPIRED; expires=Thu, 01-Jan-1970 00:00:00 GMT; domain=.google.com"
      ),
      None
    );
    assert_eq!(from("lh3.googleusercontent.com", "NID=x; path=/"), None);
    assert_eq!(from("chat.google.com", "NID=x; domain=.example.com"), None);
    // Any host can name `google.com` in a header; only a `google.com` host over HTTPS sets its cookies.
    assert_eq!(from("images.example.com", "SID=x; path=/; domain=.google.com"), None);
    assert_eq!(from("notgoogle.com", "SID=x; domain=google.com"), None);
    assert_eq!(from("google.com.example.com", "SID=x; domain=google.com"), None);
    let plain = "http://chat.google.com/".parse().expect("url");
    assert!(set_cookie("SID=x; domain=google.com", &plain).is_none());
  }

  #[test]
  fn the_jar_keeps_no_google_cookie_from_another_host() {
    let jar = SessionJar::default();
    let url = "https://images.example.com/card.png".parse().expect("url");
    let header = HeaderValue::from_static("SID=x; path=/; domain=.google.com");
    jar.set_cookies(&mut std::iter::once(&header), &url);
    assert!(jar.refreshed().is_empty());
    assert!(jar.cookies(&"https://chat.google.com/".parse().expect("url")).is_none());
  }

  #[test]
  fn the_jar_keeps_the_newest_of_each_refreshed_cookie() {
    let jar = SessionJar::default();
    let url = "https://chat.google.com/u/0/".parse().expect("url");
    let headers = ["SIDCC=one; path=/; domain=.google.com", "SIDCC=two; path=/; domain=.google.com"]
      .map(|header| HeaderValue::from_str(header).expect("header"));
    jar.set_cookies(&mut headers.iter(), &url);
    let refreshed = jar.refreshed();
    assert_eq!(refreshed.len(), 1);
    assert_eq!(refreshed[0].value, "two");
    drop(refreshed);
    // The requests send it too.
    let sent = jar.cookies(&url).expect("cookie header");
    assert_eq!(sent.to_str().expect("text"), "SIDCC=two");
  }
}
