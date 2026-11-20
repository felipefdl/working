//! The MCP servers: one per product, for AI agents on this computer.
//!
//! They let agents use the signed-in account through a few tools, each server switched on by itself in the app: chat
//! (`/mcp/chat`), mail (`/mcp/mail`), calendar (`/mcp/calendar`), and Meet (`/mcp/meet`). Each is a whole MCP
//! endpoint with its own name, instructions, and tools.
//!
//! They speak two MCP revisions over Streamable HTTP, both without sessions: each request is one POST answered
//! with one JSON object. Revision 2026-07-28 has no handshake and carries its protocol version in every
//! request; revision 2025-11-25 opens with `initialize`. One listener on loopback serves them all, and wants the
//! secret of [`token`] on every request, either as a bearer header on the server's path or as the next part of the
//! path (`/mcp/mail/<token>`), so one link is enough. A server that is off, the bare `/mcp`, and unknown names answer
//! 404 before the secret is looked at.
#![allow(
  clippy::indexing_slicing,
  reason = "JSON-RPC envelopes are objects with known keys after validation"
)]

mod calendar;
mod chat;
mod mail;
mod meet;
mod shared;

use std::net::Ipv4Addr;
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

/// One of the servers, by the name in its path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Server {
  Chat,
  Mail,
  Calendar,
  Meet,
}

impl Server {
  /// The server at `/mcp/<name>`.
  fn named(name: &str) -> Option<Self> {
    match name {
      "chat" => Some(Self::Chat),
      "mail" => Some(Self::Mail),
      "calendar" => Some(Self::Calendar),
      "meet" => Some(Self::Meet),
      _ => None,
    }
  }

  /// The name it tells clients.
  const fn name(self) -> &'static str {
    match self {
      Self::Chat => chat::NAME,
      Self::Mail => mail::NAME,
      Self::Calendar => calendar::NAME,
      Self::Meet => meet::NAME,
    }
  }

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
  /// What agents read on the chat server since the last send; every client shares it, as the server keeps no
  /// sessions.
  reads: chat::Reads,
}

/// The running server.
pub(crate) struct Running {
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
  let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, config.port)).await?;
  let context = Arc::new(Context {
    engine,
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
  Ok(Running { stop, task })
}

async fn handle(State(context): State<Arc<Context>>, request: Request) -> Response {
  let (parts, body) = request.into_parts();
  let server = match admit(parts.uri.path(), &parts.headers, &context.config) {
    Ok(server) => server,
    Err(StatusCode::UNAUTHORIZED) => return (StatusCode::UNAUTHORIZED, [(WWW_AUTHENTICATE, "Bearer")]).into_response(),
    Err(status) => return status.into_response(),
  };
  if parts.method != Method::POST {
    return (StatusCode::METHOD_NOT_ALLOWED, [(ALLOW, "POST")]).into_response();
  }
  let Ok(body) = axum::body::to_bytes(body, MAX_BODY).await else {
    return StatusCode::PAYLOAD_TOO_LARGE.into_response();
  };
  match respond(&context, server, &parts.headers, &body).await {
    (status, Some(message)) => (status, [(CONTENT_TYPE, "application/json")], message.to_string()).into_response(),
    (status, None) => status.into_response(),
  }
}

/// Which server a request is for, or the status that turns it away: 403 for a web page's origin (checked first),
/// 404 for an unknown path or a server that is off (before the secret is looked at), 401 without the secret.
fn admit(path: &str, headers: &HeaderMap, config: &McpConfig) -> Result<Server, StatusCode> {
  // A web page reaching for a local server (DNS rebinding) announces itself with its origin.
  if !origin_allowed(headers) {
    return Err(StatusCode::FORBIDDEN);
  }
  let (server, rest) = route(path).ok_or(StatusCode::NOT_FOUND)?;
  if !server.on(config) {
    return Err(StatusCode::NOT_FOUND);
  }
  let presented = match rest {
    "" | "/" => bearer(headers),
    rest => rest.strip_prefix('/'),
  };
  if !presented.is_some_and(|presented| same_secret(presented, &config.token)) {
    return Err(StatusCode::UNAUTHORIZED);
  }
  Ok(server)
}

/// The server of `/mcp/<name>` and what follows its name (empty, `/`, or `/<token>`).
fn route(path: &str) -> Option<(Server, &str)> {
  let rest = path.strip_prefix("/mcp/")?;
  let (name, rest) = rest.find('/').map_or((rest, ""), |slash| rest.split_at(slash));
  Some((Server::named(name)?, rest))
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

async fn respond(context: &Context, server: Server, headers: &HeaderMap, body: &[u8]) -> (StatusCode, Option<Value>) {
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
    Ok(true) => dispatch_legacy(context, server, method, params).await,
    Ok(false) => match validate(headers, method, params) {
      Ok(()) => dispatch(context, server, method, params).await.map(|mut result| {
        result["resultType"] = json!("complete");
        result["_meta"] = json!({ (META_SERVER): { "name": server.name(), "version": env!("CARGO_PKG_VERSION") } });
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
async fn dispatch_legacy(context: &Context, server: Server, method: &str, params: &Value) -> Result<Value, Refusal> {
  match method {
    // Whatever version the client asks for, this is the one it gets.
    "initialize" => Ok(json!({
      "protocolVersion": LEGACY_VERSION,
      "capabilities": { "tools": {} },
      "serverInfo": { "name": server.name(), "version": env!("CARGO_PKG_VERSION") },
      "instructions": server.instructions(),
    })),
    "ping" => Ok(json!({})),
    "tools/list" => Ok(json!({ "tools": server.tools(&context.config) })),
    "tools/call" => dispatch(context, server, method, params).await,
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

async fn dispatch(context: &Context, server: Server, method: &str, params: &Value) -> Result<Value, Refusal> {
  match method {
    "server/discover" => Ok(json!({
      "supportedVersions": [PROTOCOL_VERSION],
      "capabilities": { "tools": {} },
      "instructions": server.instructions(),
      "ttlMs": 3_600_000,
      "cacheScope": "public",
    })),
    // A list changes only when the user changes the settings, which restarts the servers.
    "tools/list" => Ok(json!({ "tools": server.tools(&context.config), "ttlMs": 60_000, "cacheScope": "public" })),
    "tools/call" => {
      let name = params["name"].as_str().unwrap_or_default();
      let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
      let unknown = || Refusal::new(StatusCode::OK, INVALID_PARAMS, format!("Unknown tool: {name}"));
      call(context, server, name, arguments).await.ok_or_else(unknown)
    },
    _ => Err(Refusal::new(
      StatusCode::NOT_FOUND,
      METHOD_NOT_FOUND,
      format!("Method not found: {method}"),
    )),
  }
}

/// Runs a tool of `server`. `None` when it has no tool of that name.
async fn call(context: &Context, server: Server, name: &str, arguments: Value) -> Option<Value> {
  let engine = &context.engine;
  match server {
    Server::Chat => chat::call(engine, &context.reads, context.config.allow_send, name, arguments).await,
    Server::Mail => mail::call(engine, context.config.mail_write, name, arguments).await,
    Server::Calendar => calendar::call(engine, context.config.calendar_write, name, arguments).await,
    Server::Meet => meet::call(engine, name, arguments).await,
  }
}

#[cfg(test)]
mod tests {
  use super::{Server, admit, bearer, header_text, origin_allowed, route, same_secret};
  use crate::model::McpConfig;
  use axum::http::header::{AUTHORIZATION, ORIGIN};
  use axum::http::{HeaderMap, StatusCode};
  use base64::Engine as _;

  /// The settings with the servers `on` switched on.
  fn config(on: &[Server]) -> McpConfig {
    McpConfig {
      port: 3021,
      token: "wk_secret".into(),
      chat: on.contains(&Server::Chat),
      allow_send: false,
      mail: on.contains(&Server::Mail),
      mail_write: false,
      calendar: on.contains(&Server::Calendar),
      calendar_write: false,
      meet: on.contains(&Server::Meet),
    }
  }

  const ALL: [Server; 4] = [Server::Chat, Server::Mail, Server::Calendar, Server::Meet];

  #[test]
  fn paths_name_one_server_each() {
    assert_eq!(route("/mcp/chat"), Some((Server::Chat, "")));
    assert_eq!(route("/mcp/mail/"), Some((Server::Mail, "/")));
    assert_eq!(route("/mcp/calendar/wk_secret"), Some((Server::Calendar, "/wk_secret")));
    assert_eq!(route("/mcp/meet"), Some((Server::Meet, "")));
    assert_eq!(route("/mcp"), None);
    assert_eq!(route("/mcp/"), None);
    assert_eq!(route("/mcp/wk_secret"), None);
    assert_eq!(route("/mcp/drive"), None);
    assert_eq!(route("/other/chat"), None);
  }

  #[test]
  fn a_server_answers_only_when_on_and_with_the_secret() {
    let mut bearer_headers = HeaderMap::new();
    bearer_headers.insert(AUTHORIZATION, "Bearer wk_secret".parse().unwrap());
    let none = HeaderMap::new();
    let only_mail = config(&[Server::Mail]);
    assert_eq!(admit("/mcp/mail", &bearer_headers, &only_mail), Ok(Server::Mail));
    assert_eq!(admit("/mcp/mail/wk_secret", &none, &only_mail), Ok(Server::Mail));
    assert_eq!(admit("/mcp/mail", &none, &only_mail), Err(StatusCode::UNAUTHORIZED));
    assert_eq!(admit("/mcp/mail/wk_wrong", &none, &only_mail), Err(StatusCode::UNAUTHORIZED));
    for off in ["/mcp/chat", "/mcp/calendar/wk_secret", "/mcp/meet"] {
      assert_eq!(
        admit(off, &bearer_headers, &only_mail),
        Err(StatusCode::NOT_FOUND),
        "{off} is off"
      );
      assert_eq!(
        admit(off, &none, &only_mail),
        Err(StatusCode::NOT_FOUND),
        "404 before the secret"
      );
    }
    assert_eq!(admit("/mcp", &bearer_headers, &only_mail), Err(StatusCode::NOT_FOUND));
    assert_eq!(admit("/mcp/wk_secret", &none, &only_mail), Err(StatusCode::NOT_FOUND));
    let all = config(&ALL);
    for (path, server) in [
      ("/mcp/chat/wk_secret", Server::Chat),
      ("/mcp/calendar/wk_secret", Server::Calendar),
      ("/mcp/meet/wk_secret", Server::Meet),
    ] {
      assert_eq!(admit(path, &none, &all), Ok(server));
    }
    let mut page = bearer_headers.clone();
    page.insert(ORIGIN, "https://evil.example".parse().unwrap());
    assert_eq!(admit("/mcp/drive", &page, &all), Err(StatusCode::FORBIDDEN), "the origin first");
  }

  #[test]
  fn servers_have_their_own_names_and_tools() {
    let mut all = config(&ALL);
    let names = |server: Server, config: &McpConfig| -> Vec<String> {
      server
        .tools(config)
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_string))
        .collect()
    };
    assert_eq!(Server::Chat.name(), "working-chat");
    assert_eq!(Server::Mail.name(), "working-mail");
    assert_eq!(Server::Calendar.name(), "working-calendar");
    assert_eq!(Server::Meet.name(), "working-meet");
    assert!(!names(Server::Chat, &all).contains(&"send_message".to_string()));
    assert!(!names(Server::Calendar, &all).contains(&"calendar_create_event".to_string()));
    assert_eq!(
      names(Server::Mail, &all),
      [
        "mail_inbox",
        "mail_search",
        "mail_read_thread",
        "mail_labels",
        "mail_attachment"
      ]
    );
    all.allow_send = true;
    all.calendar_write = true;
    all.mail_write = true;
    assert!(names(Server::Chat, &all).contains(&"send_message".to_string()));
    assert!(names(Server::Calendar, &all).contains(&"calendar_create_event".to_string()));
    assert_eq!(
      names(Server::Mail, &all),
      [
        "mail_inbox",
        "mail_search",
        "mail_read_thread",
        "mail_labels",
        "mail_attachment",
        "mail_send",
        "mail_reply",
        "mail_forward",
        "mail_draft",
        "mail_send_draft",
        "mail_archive",
        "mail_mark",
        "mail_star",
        "mail_label"
      ]
    );
    assert_eq!(names(Server::Meet, &all), ["meet_create_meeting", "meet_upcoming"]);
    let chat = names(Server::Chat, &all);
    assert!(
      chat
        .iter()
        .all(|name| !name.starts_with("mail_") && !name.starts_with("calendar_"))
    );
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
