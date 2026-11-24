//! The MCP server, `working`, for AI agents on this computer.
//!
//! It lets agents use the signed-in account through a few tools, in groups the user switches on one by one in the
//! app: chat, mail, calendar, and Meet. The server lists and runs only the tools of the groups that are on, and its
//! instructions speak only of those.
//!
//! It speaks two MCP revisions over Streamable HTTP, both without sessions: each request is one POST answered with
//! one JSON object. Revision 2026-07-28 has no handshake and carries its protocol version in every request; revision
//! 2025-11-25 opens with `initialize`. It listens at the address and port the user set (loopback, 127.0.0.1:3021, by
//! default), path `/mcp`, and wants the secret of [`token`] on every request on any address, either as a bearer header
//! or as the next part of the path (`/mcp/<token>`), so one link is enough. Any other path answers 404 before the
//! secret is looked at, and a request from a web page that is not on loopback is refused before that.
#![allow(
  clippy::indexing_slicing,
  reason = "JSON-RPC envelopes are objects with known keys after validation"
)]

mod calendar;
mod chat;
mod mail;
mod meet;
mod shared;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::header::{ALLOW, AUTHORIZATION, CONTENT_TYPE, ORIGIN, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::engine::Engine;
use crate::model::McpConfig;

/// MCP protocol revision this server speaks first (no initialize handshake).
pub const PROTOCOL_VERSION: &str = "2026-07-28";
/// The revision with the `initialize` handshake, for clients that do not speak the one above yet.
const LEGACY_VERSION: &str = "2025-11-25";

/// The name the server tells clients.
const NAME: &str = "working";

/// What the instructions say before the parts of the groups that are on.
const OPENING: &str = "Tools for the Google account of the person signed in to the Working app on this computer. \
Times are this computer's local time.";

/// A group of tools, switched on by itself in [`McpConfig`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
  Chat,
  Mail,
  Calendar,
  Meet,
}

impl Group {
  /// Every group, in the order their tools are listed.
  const ALL: [Self; 4] = [Self::Chat, Self::Mail, Self::Calendar, Self::Meet];

  /// Its part of the server's instructions.
  const fn instructions(self) -> &'static str {
    match self {
      Self::Chat => chat::INSTRUCTIONS,
      Self::Mail => mail::INSTRUCTIONS,
      Self::Calendar => calendar::INSTRUCTIONS,
      Self::Meet => meet::INSTRUCTIONS,
    }
  }

  /// The user switched it on.
  const fn on(self, config: &McpConfig) -> bool {
    match self {
      Self::Chat => config.chat,
      Self::Mail => config.mail,
      Self::Calendar => config.calendar,
      Self::Meet => config.meet,
    }
  }

  /// Its tool descriptors under `config`.
  fn tools(self, config: &McpConfig) -> Vec<Value> {
    match self {
      Self::Chat => chat::list(config.allow_send),
      Self::Mail => mail::list(config.mail_write),
      Self::Calendar => calendar::list(config.calendar_write),
      Self::Meet => meet::list(),
    }
  }

  /// It runs a tool of this name (a write tool too, which refuses while its write switch is off).
  fn owns(self, name: &str) -> bool {
    match self {
      Self::Chat => chat::owns(name),
      Self::Mail => mail::owns(name),
      Self::Calendar => calendar::owns(name),
      Self::Meet => meet::owns(name),
    }
  }

  /// The groups that are on, in order.
  fn on_in(config: &McpConfig) -> impl Iterator<Item = Self> + '_ {
    Self::ALL.into_iter().filter(|group| group.on(config))
  }

  /// The group that is on and runs the tool `name`.
  fn owner(config: &McpConfig, name: &str) -> Option<Self> {
    Self::on_in(config).find(|group| group.owns(name))
  }
}

/// The tools of every group that is on, in order.
fn tools(config: &McpConfig) -> Vec<Value> {
  Group::on_in(config).flat_map(|group| group.tools(config)).collect()
}

/// The opening, then the part of each group that is on.
fn instructions(config: &McpConfig) -> String {
  let mut text = OPENING.to_string();
  for group in Group::on_in(config) {
    text.push_str("\n\n");
    text.push_str(group.instructions());
  }
  text
}

/// No request of this server comes near this size.
const MAX_BODY: usize = 1 << 20;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const HEADER_MISMATCH: i64 = -32020;
const UNSUPPORTED_VERSION: i64 = -32022;

const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const META_SERVER: &str = "io.modelcontextprotocol/serverInfo";

fn token_path() -> PathBuf {
  crate::config_dir().join("mcp-token")
}

/// The secret agents present to the server. Made on first use and kept in `mcp-token` in [`crate::config_dir`].
pub fn token() -> std::io::Result<String> {
  match std::fs::read_to_string(token_path()) {
    Ok(saved) if !saved.trim().is_empty() => Ok(saved.trim().to_string()),
    _ => rotate_token(),
  }
}

/// Replaces the secret: whatever was connected with the old one is refused from the next restart of the server.
pub fn rotate_token() -> std::io::Result<String> {
  use std::io::Write;

  let secret: [u8; 32] = rand::random();
  let token = format!("wk_{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret));
  let path = token_path();
  std::fs::create_dir_all(crate::config_dir())?;
  let mut file = crate::paths::create_private(&path)?;
  file.write_all(token.as_bytes())?;
  Ok(token)
}

struct Context {
  engine: Arc<Engine>,
  config: McpConfig,
  /// The instructions for the groups that are on, made once: the settings do not change while the server runs.
  instructions: String,
  /// What agents read with the chat tools since the last send; every client shares it, as the server keeps no
  /// sessions.
  reads: chat::Reads,
}

/// The running server.
pub(crate) struct Running {
  /// The address it is bound to.
  pub address: SocketAddr,
  stop: oneshot::Sender<()>,
  task: JoinHandle<()>,
}

impl Running {
  /// Stops listening and closes every connection, after the requests under way are answered.
  pub async fn stop(self) {
    let _ = self.stop.send(());
    let _ = self.task.await;
  }
}

pub(crate) async fn serve(engine: Arc<Engine>, config: McpConfig) -> std::io::Result<Running> {
  let listener = tokio::net::TcpListener::bind((config.address, config.port)).await?;
  let address = listener.local_addr()?;
  let context = Arc::new(Context {
    engine,
    instructions: instructions(&config),
    config,
    reads: chat::Reads::default(),
  });
  let router = Router::new().fallback(handle).with_state(context);
  let (stop, stopped) = oneshot::channel::<()>();
  let task = tokio::spawn(async move {
    let served = axum::serve(listener, router).with_graceful_shutdown(async {
      let _ = stopped.await;
    });
    if let Err(error) = served.await {
      tracing::warn!("the MCP server stopped: {error}");
    }
  });
  Ok(Running { address, stop, task })
}

async fn handle(State(context): State<Arc<Context>>, request: Request) -> Response {
  let (parts, body) = request.into_parts();
  match admit(parts.uri.path(), &parts.headers, &context.config.token) {
    Ok(()) => {},
    Err(StatusCode::UNAUTHORIZED) => return (StatusCode::UNAUTHORIZED, [(WWW_AUTHENTICATE, "Bearer")]).into_response(),
    Err(status) => return status.into_response(),
  }
  if parts.method != Method::POST {
    return (StatusCode::METHOD_NOT_ALLOWED, [(ALLOW, "POST")]).into_response();
  }
  let Ok(body) = axum::body::to_bytes(body, MAX_BODY).await else {
    return StatusCode::PAYLOAD_TOO_LARGE.into_response();
  };
  match respond(&context, &parts.headers, &body).await {
    (status, Some(message)) => (status, [(CONTENT_TYPE, "application/json")], message.to_string()).into_response(),
    (status, None) => status.into_response(),
  }
}

/// Lets a request in, or answers the status that turns it away: 403 for a web page's origin (checked first), 404 for
/// a path that is not the server's (before the secret is looked at), 401 without the secret.
fn admit(path: &str, headers: &HeaderMap, token: &str) -> Result<(), StatusCode> {
  // A web page reaching for a local server (DNS rebinding) announces itself with its origin.
  if !origin_allowed(headers) {
    return Err(StatusCode::FORBIDDEN);
  }
  let presented = match route(path).ok_or(StatusCode::NOT_FOUND)? {
    Route::Bare => bearer(headers),
    Route::Token(segment) => Some(segment),
  };
  if !presented.is_some_and(|presented| same_secret(presented, token)) {
    return Err(StatusCode::UNAUTHORIZED);
  }
  Ok(())
}

/// A path of the server.
#[derive(Debug, PartialEq, Eq)]
enum Route<'a> {
  /// `/mcp` or `/mcp/`: the secret comes in a header.
  Bare,
  /// `/mcp/<token>`: the secret is the segment after `/mcp`.
  Token(&'a str),
}

/// The server's path `path` is, if it is one: `/mcp`, `/mcp/`, or `/mcp/<one segment>`.
fn route(path: &str) -> Option<Route<'_>> {
  match path.strip_prefix("/mcp")? {
    "" | "/" => Some(Route::Bare),
    rest => {
      let segment = rest.strip_prefix('/')?;
      (!segment.contains('/')).then_some(Route::Token(segment))
    },
  }
}

fn origin_allowed(headers: &HeaderMap) -> bool {
  let Some(origin) = headers.get(ORIGIN) else {
    return true;
  };
  let Ok(origin) = origin.to_str() else {
    return false;
  };
  let host = origin.split_once("://").map_or(origin, |(_, host)| host);
  let host = match host.rsplit_once(':') {
    Some((name, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => name,
    _ => host,
  };
  matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
  let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
  let (scheme, token) = value.split_once(' ')?;
  scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

/// Compares without stopping at the first difference, so timing says nothing about the secret.
fn same_secret(presented: &str, secret: &str) -> bool {
  presented.len() == secret.len() && presented.bytes().zip(secret.bytes()).fold(0, |diff, (a, b)| diff | (a ^ b)) == 0
}

/// A request the protocol turns down, as the HTTP status and the JSON-RPC error that go back.
struct Refusal {
  status: StatusCode,
  code: i64,
  message: String,
  data: Option<Value>,
}

impl Refusal {
  fn new(status: StatusCode, code: i64, message: impl Into<String>) -> Self {
    Self {
      status,
      code,
      message: message.into(),
      data: None,
    }
  }

  fn bad(code: i64, message: impl Into<String>) -> Self {
    Self::new(StatusCode::BAD_REQUEST, code, message)
  }

  fn unsupported_version(requested: Option<&str>) -> Self {
    Self {
      data: Some(json!({ "supported": [PROTOCOL_VERSION, LEGACY_VERSION], "requested": requested })),
      ..Self::bad(
        UNSUPPORTED_VERSION,
        format!("Unsupported protocol version: this server speaks {PROTOCOL_VERSION} and {LEGACY_VERSION}"),
      )
    }
  }

  fn answer(self, id: Option<Value>) -> (StatusCode, Option<Value>) {
    let mut error = json!({ "code": self.code, "message": self.message });
    if let Some(data) = self.data {
      error["data"] = data;
    }
    let mut message = json!({ "jsonrpc": "2.0", "error": error });
    if let Some(id) = id {
      message["id"] = id;
    }
    (self.status, Some(message))
  }
}

async fn respond(context: &Context, headers: &HeaderMap, body: &[u8]) -> (StatusCode, Option<Value>) {
  let Ok(message) = serde_json::from_slice::<Value>(body) else {
    return Refusal::bad(PARSE_ERROR, "The body is not JSON").answer(None);
  };
  let id = message.get("id").filter(|id| !id.is_null()).cloned();
  let method = message.get("method").and_then(Value::as_str);
  let (Some(method), Some("2.0")) = (method, message.get("jsonrpc").and_then(Value::as_str)) else {
    return Refusal::bad(INVALID_REQUEST, "Not a JSON-RPC 2.0 request").answer(id);
  };
  // A notification: nothing a client sends this way needs an answer, so it is taken and dropped.
  let Some(id) = id else {
    return (StatusCode::ACCEPTED, None);
  };
  let params = message.get("params").unwrap_or(&Value::Null);
  let result = match legacy(headers, method, params) {
    Ok(true) => dispatch_legacy(context, method, params).await,
    Ok(false) => match validate(headers, method, params) {
      Ok(()) => dispatch(context, method, params).await.map(|mut result| {
        result["resultType"] = json!("complete");
        result["_meta"] = json!({ (META_SERVER): { "name": NAME, "version": env!("CARGO_PKG_VERSION") } });
        result
      }),
      Err(refusal) => Err(refusal),
    },
    Err(refusal) => Err(refusal),
  };
  match result {
    Ok(result) => (StatusCode::OK, Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))),
    Err(refusal) => refusal.answer(Some(id)),
  }
}

/// Which revision a request is in. The handshake is the older one's; after it, the newer one names itself in
/// `_meta` and the older one only in the header, which its clients may leave out.
fn legacy(headers: &HeaderMap, method: &str, params: &Value) -> Result<bool, Refusal> {
  if method == "initialize" {
    return Ok(true);
  }
  if params["_meta"][META_VERSION].is_string() {
    return Ok(false);
  }
  match headers.get("mcp-protocol-version").and_then(|value| value.to_str().ok()) {
    None | Some(LEGACY_VERSION) => Ok(true),
    Some(PROTOCOL_VERSION) => Ok(false),
    Some(other) => Err(Refusal::unsupported_version(Some(other))),
  }
}

/// Revision 2025-11-25: the handshake, then plain requests. Its errors travel with status 200.
async fn dispatch_legacy(context: &Context, method: &str, params: &Value) -> Result<Value, Refusal> {
  match method {
    // Whatever version the client asks for, this is the one it gets.
    "initialize" => Ok(json!({
      "protocolVersion": LEGACY_VERSION,
      "capabilities": { "tools": {} },
      "serverInfo": { "name": NAME, "version": env!("CARGO_PKG_VERSION") },
      "instructions": context.instructions,
    })),
    "ping" => Ok(json!({})),
    "tools/list" => Ok(json!({ "tools": tools(&context.config) })),
    "tools/call" => dispatch(context, method, params).await,
    _ => Err(Refusal::new(
      StatusCode::OK,
      METHOD_NOT_FOUND,
      format!("Method not found: {method}"),
    )),
  }
}

/// The per-request rules of the revision: the protocol version and what the client can do travel in `_meta`,
/// and the headers that mirror the body for proxies have to agree with it.
fn validate(headers: &HeaderMap, method: &str, params: &Value) -> Result<(), Refusal> {
  let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
  let Some(version_header) = header("mcp-protocol-version") else {
    return Err(Refusal::bad(
      HEADER_MISMATCH,
      "Header mismatch: the MCP-Protocol-Version header is missing",
    ));
  };
  let meta = &params["_meta"];
  let Some(version) = meta[META_VERSION].as_str() else {
    return Err(Refusal::bad(
      INVALID_PARAMS,
      format!("params._meta is missing \"{META_VERSION}\""),
    ));
  };
  if version_header != version {
    let message = format!("Header mismatch: MCP-Protocol-Version is '{version_header}' but the body says '{version}'");
    return Err(Refusal::bad(HEADER_MISMATCH, message));
  }
  if version != PROTOCOL_VERSION {
    return Err(Refusal::unsupported_version(Some(version)));
  }
  if !meta[META_CAPABILITIES].is_object() {
    return Err(Refusal::bad(
      INVALID_PARAMS,
      format!("params._meta is missing \"{META_CAPABILITIES}\""),
    ));
  }
  if header("mcp-method") != Some(method) {
    let message = format!("Header mismatch: the Mcp-Method header has to be '{method}', as in the body");
    return Err(Refusal::bad(HEADER_MISMATCH, message));
  }
  if method == "tools/call" {
    let name = params["name"].as_str().unwrap_or_default();
    if header("mcp-name").and_then(header_text).as_deref() != Some(name) {
      let message = format!("Header mismatch: the Mcp-Name header has to be '{name}', as in the body");
      return Err(Refusal::bad(HEADER_MISMATCH, message));
    }
  }
  Ok(())
}

/// A mirrored header value: plain, or wrapped as `=?base64?…?=` when it holds more than plain ASCII.
fn header_text(value: &str) -> Option<String> {
  match value.strip_prefix("=?base64?").and_then(|rest| rest.strip_suffix("?=")) {
    Some(encoded) => String::from_utf8(base64::engine::general_purpose::STANDARD.decode(encoded).ok()?).ok(),
    None => Some(value.to_string()),
  }
}

async fn dispatch(context: &Context, method: &str, params: &Value) -> Result<Value, Refusal> {
  match method {
    "server/discover" => Ok(json!({
      "supportedVersions": [PROTOCOL_VERSION],
      "capabilities": { "tools": {} },
      "instructions": context.instructions,
      "ttlMs": 3_600_000,
      "cacheScope": "public",
    })),
    // The list changes only when the user changes the settings, which restarts the server.
    "tools/list" => Ok(json!({ "tools": tools(&context.config), "ttlMs": 60_000, "cacheScope": "public" })),
    "tools/call" => {
      let name = params["name"].as_str().unwrap_or_default();
      let unknown = || Refusal::new(StatusCode::OK, INVALID_PARAMS, format!("Unknown tool: {name}"));
      let group = Group::owner(&context.config, name).ok_or_else(unknown)?;
      let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
      call(context, group, name, arguments).await.ok_or_else(unknown)
    },
    _ => Err(Refusal::new(
      StatusCode::NOT_FOUND,
      METHOD_NOT_FOUND,
      format!("Method not found: {method}"),
    )),
  }
}

/// Runs a tool of `group`. `None` when it has no tool of that name.
async fn call(context: &Context, group: Group, name: &str, arguments: Value) -> Option<Value> {
  let engine = &context.engine;
  match group {
    Group::Chat => chat::call(engine, &context.reads, context.config.allow_send, name, arguments).await,
    Group::Mail => mail::call(engine, context.config.mail_write, name, arguments).await,
    Group::Calendar => calendar::call(engine, context.config.calendar_write, name, arguments).await,
    Group::Meet => meet::call(engine, name, arguments).await,
  }
}

#[cfg(test)]
mod tests {
  use super::{Group, Route, admit, bearer, header_text, instructions, origin_allowed, route, same_secret, tools};
  use crate::model::McpConfig;
  use axum::http::header::{AUTHORIZATION, ORIGIN};
  use axum::http::{HeaderMap, StatusCode};
  use base64::Engine as _;

  /// The settings with the groups `on` switched on and every write switch off.
  fn config(on: &[Group]) -> McpConfig {
    McpConfig {
      address: std::net::Ipv4Addr::LOCALHOST.into(),
      port: 3021,
      token: "wk_secret".into(),
      chat: on.contains(&Group::Chat),
      allow_send: false,
      mail: on.contains(&Group::Mail),
      mail_write: false,
      calendar: on.contains(&Group::Calendar),
      calendar_write: false,
      meet: on.contains(&Group::Meet),
    }
  }

  fn names(config: &McpConfig) -> Vec<String> {
    tools(config)
      .iter()
      .filter_map(|tool| tool["name"].as_str().map(str::to_string))
      .collect()
  }

  const MAIL_READS: [&str; 7] = [
    "mail_inbox",
    "mail_search",
    "mail_read_thread",
    "mail_html",
    "mail_eml",
    "mail_labels",
    "mail_attachment",
  ];

  #[test]
  fn the_server_answers_at_mcp_only() {
    assert_eq!(route("/mcp"), Some(Route::Bare));
    assert_eq!(route("/mcp/"), Some(Route::Bare));
    assert_eq!(route("/mcp/wk_secret"), Some(Route::Token("wk_secret")));
    assert_eq!(route("/mcp/wk_secret/"), None);
    assert_eq!(route("/mcp/wk_secret/more"), None);
    assert_eq!(route("/mcpx"), None);
    assert_eq!(route("/"), None);
    assert_eq!(route("/other/mcp"), None);
  }

  #[test]
  fn a_request_needs_the_path_then_the_secret() {
    let mut bearer_headers = HeaderMap::new();
    bearer_headers.insert(AUTHORIZATION, "Bearer wk_secret".parse().unwrap());
    let none = HeaderMap::new();
    assert_eq!(admit("/mcp", &bearer_headers, "wk_secret"), Ok(()));
    assert_eq!(admit("/mcp/", &bearer_headers, "wk_secret"), Ok(()));
    assert_eq!(admit("/mcp/wk_secret", &none, "wk_secret"), Ok(()));
    assert_eq!(admit("/mcp", &none, "wk_secret"), Err(StatusCode::UNAUTHORIZED));
    assert_eq!(admit("/mcp/wk_wrong", &none, "wk_secret"), Err(StatusCode::UNAUTHORIZED));
    assert_eq!(
      admit("/mcp/wk_wrong", &bearer_headers, "wk_secret"),
      Err(StatusCode::UNAUTHORIZED),
      "a secret in the path is the one looked at"
    );
    assert_eq!(
      admit("/other", &none, "wk_secret"),
      Err(StatusCode::NOT_FOUND),
      "404 before the secret"
    );
    assert_eq!(
      admit("/mcp/wk_secret/more", &bearer_headers, "wk_secret"),
      Err(StatusCode::NOT_FOUND)
    );
    let mut page = bearer_headers.clone();
    page.insert(ORIGIN, "https://evil.example".parse().unwrap());
    assert_eq!(
      admit("/other", &page, "wk_secret"),
      Err(StatusCode::FORBIDDEN),
      "the origin first"
    );
    assert_eq!(admit("/mcp", &page, "wk_secret"), Err(StatusCode::FORBIDDEN));
  }

  #[test]
  fn the_list_follows_the_group_and_write_switches() {
    assert_eq!(names(&config(&[])), Vec::<String>::new());
    assert_eq!(names(&config(&[Group::Mail])), MAIL_READS);
    assert_eq!(names(&config(&[Group::Meet])), ["meet_create_meeting", "meet_upcoming"]);

    let mut all = config(&Group::ALL);
    let listed = names(&all);
    assert!(!listed.contains(&"send_message".to_string()));
    assert!(!listed.contains(&"mail_send".to_string()));
    assert!(!listed.contains(&"calendar_create_event".to_string()));
    // Chat first, then mail, calendar, and Meet.
    assert_eq!(listed.first().map(String::as_str), Some("inbox"));
    let at = |name: &str| listed.iter().position(|listed| listed == name).unwrap();
    assert!(at("search_messages") < at("mail_inbox"));
    assert!(at("mail_attachment") < at("calendar_list"));
    assert!(at("calendar_get_event") < at("meet_create_meeting"));

    all.allow_send = true;
    all.mail_write = true;
    all.calendar_write = true;
    let listed = names(&all);
    for write in [
      "send_message",
      "mail_send",
      "mail_label",
      "calendar_create_event",
      "calendar_respond",
    ] {
      assert!(listed.contains(&write.to_string()), "{write} is listed");
    }

    let mut only_mail = config(&[Group::Mail]);
    only_mail.allow_send = true;
    only_mail.calendar_write = true;
    assert_eq!(
      names(&only_mail),
      MAIL_READS,
      "a write switch lists nothing of a group that is off"
    );
  }

  #[test]
  fn a_tool_runs_only_in_a_group_that_is_on() {
    let only_mail = config(&[Group::Mail]);
    assert_eq!(Group::owner(&only_mail, "mail_inbox"), Some(Group::Mail));
    for name in ["inbox", "send_message", "calendar_events", "meet_upcoming", "drive_files", ""] {
      assert_eq!(Group::owner(&only_mail, name), None, "{name} is unknown");
    }
    let all = config(&Group::ALL);
    assert_eq!(Group::owner(&all, "inbox"), Some(Group::Chat));
    assert_eq!(Group::owner(&all, "calendar_events"), Some(Group::Calendar));
    assert_eq!(Group::owner(&all, "meet_create_meeting"), Some(Group::Meet));
    assert_eq!(Group::owner(&config(&[]), "inbox"), None);
  }

  #[test]
  fn each_tool_belongs_to_the_one_group_that_lists_it() {
    let mut all = config(&Group::ALL);
    all.allow_send = true;
    all.mail_write = true;
    all.calendar_write = true;
    for group in Group::ALL {
      for tool in group.tools(&all) {
        let name = tool["name"].as_str().unwrap();
        let owners: Vec<_> = Group::ALL.into_iter().filter(|other| other.owns(name)).collect();
        assert_eq!(owners, [group], "{name}");
      }
    }
  }

  #[test]
  fn instructions_speak_only_of_the_groups_that_are_on() {
    let only_mail = instructions(&config(&[Group::Mail]));
    assert!(only_mail.contains("mail_inbox"));
    assert!(!only_mail.contains("read_conversation"));
    assert!(!only_mail.contains("calendar_events"));
    assert!(!only_mail.contains("meet_upcoming"));
    let all = instructions(&config(&Group::ALL));
    for name in ["read_conversation", "mail_inbox", "calendar_events", "meet_upcoming"] {
      assert!(all.contains(name), "{name}");
    }
  }

  #[test]
  fn origin_allows_loopback_and_missing() {
    let mut headers = HeaderMap::new();
    assert!(origin_allowed(&headers));
    headers.insert(ORIGIN, "http://127.0.0.1:3021".parse().unwrap());
    assert!(origin_allowed(&headers));
    headers.insert(ORIGIN, "https://localhost".parse().unwrap());
    assert!(origin_allowed(&headers));
    headers.insert(ORIGIN, "https://evil.example".parse().unwrap());
    assert!(!origin_allowed(&headers));
  }

  #[test]
  fn bearer_reads_the_token() {
    let mut headers = HeaderMap::new();
    assert_eq!(bearer(&headers), None);
    headers.insert(AUTHORIZATION, "Bearer  wk_abc ".parse().unwrap());
    assert_eq!(bearer(&headers), Some("wk_abc"));
    headers.insert(AUTHORIZATION, "Basic wk_abc".parse().unwrap());
    assert_eq!(bearer(&headers), None);
  }

  #[test]
  fn same_secret_is_length_safe() {
    assert!(same_secret("abc", "abc"));
    assert!(!same_secret("abc", "abd"));
    assert!(!same_secret("abc", "ab"));
  }

  #[test]
  fn header_text_unwraps_base64() {
    assert_eq!(header_text("plain"), Some("plain".into()));
    let wrapped = format!("=?base64?{}?=", base64::engine::general_purpose::STANDARD.encode("café"));
    assert_eq!(header_text(&wrapped), Some("café".into()));
  }
}
