//! Authenticated HTTP to chat.google.com: cookie session, XSRF token, and protobuf API calls.
#![allow(
  clippy::unreadable_literal,
  clippy::string_slice,
  clippy::indexing_slicing,
  clippy::as_conversions,
  reason = "bootstrap page scraping and protobuf request headers"
)]

use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use prost::Message;
use reqwest::cookie::Jar;
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
  jar: Arc<Jar>,
  cookie_path: std::path::PathBuf,
  xsrf: RwLock<Option<String>>,
  /// Token the web app's page RPCs require, read with the XSRF token.
  page_token: RwLock<Option<String>>,
  request_id: AtomicU64,
}

impl Http {
  pub fn from_cookie_file(path: &std::path::Path) -> Result<Self> {
    let jar = Arc::new(Jar::default());
    load_cookies(&jar, path)?;

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
      xsrf: RwLock::new(None),
      page_token: RwLock::new(None),
      request_id: AtomicU64::new(1),
    })
  }

  pub fn reload_cookies(&self) -> Result<()> {
    load_cookies(&self.jar, &self.cookie_path)
  }

  pub const fn stream_client(&self) -> &reqwest::Client {
    &self.stream_client
  }

  pub fn xsrf(&self) -> Option<String> {
    self.xsrf.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
  }

  pub fn set_xsrf(&self, token: String) {
    *self.xsrf.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(token);
  }

  /// Reads the XSRF token from the web app's bootstrap page. Also the cheapest check that the session is alive.
  pub async fn refresh_xsrf(&self) -> Result<String> {
    match self.read_bootstrap().await {
      Err(error) if error.downcast_ref::<SignedOut>().is_some() => {
        // The cookies in use were rotated away (by a Meet call): take the saved ones and try once more.
        load_cookies(&self.jar, &self.cookie_path)?;
        self.read_bootstrap().await
      },
      result => result,
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
    let body = request.encode_to_vec();
    match self.api_once(endpoint, body.clone()).await {
      Err(error) if error.downcast_ref::<StaleToken>().is_some() => {
        self.refresh_xsrf().await?;
        self.api_once(endpoint, body).await
      },
      other => other,
    }
  }

  async fn api_once<Resp: Message + Default>(&self, endpoint: &str, body: Vec<u8>) -> Result<Resp> {
    let token = match self.xsrf() {
      Some(token) => token,
      None => self.refresh_xsrf().await?,
    };
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
      return Err(StaleToken.into());
    }
    if !status.is_success() {
      let body = String::from_utf8_lossy(&bytes);
      let body = body.chars().take(300).collect::<String>();
      bail!("{endpoint} answered {status}: {body}");
    }
    let decoded = if base64_encoded {
      BASE64.decode(&bytes)?
    } else {
      bytes.to_vec()
    };
    Resp::decode(decoded.as_slice()).with_context(|| format!("{endpoint} answer does not decode"))
  }

  /// Calls a Chat API endpoint the way the web app does for requests the protobuf schema here cannot express: the
  /// body and the answer are the positional JSON form of the protobuf messages (index `n` is field `n + 1`).
  pub async fn api_json(&self, endpoint: &str, request: &serde_json::Value) -> Result<serde_json::Value> {
    match self.api_json_once(endpoint, request).await {
      Err(error) if error.downcast_ref::<StaleToken>().is_some() => {
        self.refresh_xsrf().await?;
        self.api_json_once(endpoint, request).await
      },
      other => other,
    }
  }

  async fn api_json_once(&self, endpoint: &str, request: &serde_json::Value) -> Result<serde_json::Value> {
    let token = match self.xsrf() {
      Some(token) => token,
      None => self.refresh_xsrf().await?,
    };
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
      return Err(StaleToken.into());
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
    Ok(response.bytes().await?.to_vec())
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

#[derive(Debug)]
struct StaleToken;

impl std::fmt::Display for StaleToken {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str("the request token was refused")
  }
}

impl std::error::Error for StaleToken {}

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
