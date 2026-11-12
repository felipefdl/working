//! The MCP server: lets AI agents on this Mac use the signed-in chat through a few tools.
//!
//! It speaks two MCP revisions over Streamable HTTP, both without sessions: each request is one POST answered
//! with one JSON object. Revision 2026-07-28 has no handshake and carries its protocol version in every
//! request; revision 2025-11-25 opens with `initialize`. The server listens on loopback only and wants the
//! secret of [`token`] on every request, either as a bearer header on `/mcp` or as the last part of the path
//! (`/mcp/<token>`), so one link is enough.
#![allow(
  clippy::indexing_slicing,
  reason = "JSON-RPC envelopes are objects with known keys after validation"
)]

mod tools;

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
const SERVER_NAME: &str = "working";
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

/// The secret agents present to the server. Made on first use and kept in `~/.config/working/mcp-token`.
pub fn token() -> std::io::Result<String> {
  match std::fs::read_to_string(token_path()) {
    Ok(saved) if !saved.trim().is_empty() => Ok(saved.trim().to_string()),
    _ => rotate_token(),
  }
}

/// Replaces the secret: whatever was connected with the old one is refused from the next restart of the server.
pub fn rotate_token() -> std::io::Result<String> {
  use std::io::Write;
  use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

  let secret: [u8; 32] = rand::random();
  let token = format!("wk_{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret));
  let path = token_path();
  std::fs::create_dir_all(crate::config_dir())?;
  let mut file = std::fs::OpenOptions::new()
    .write(true)
    .create(true)
    .truncate(true)
    .mode(0o600)
    .open(&path)?;
  // `mode` only applies to a file this call creates.
  file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
  file.write_all(token.as_bytes())?;
  Ok(token)
}

struct Context {
  engine: Arc<Engine>,
  token: String,
  allow_send: bool,
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
    token: config.token,
    allow_send: config.allow_send,
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
  // A web page reaching for a local server (DNS rebinding) announces itself with its origin.
  if !origin_allowed(&parts.headers) {
    return StatusCode::FORBIDDEN.into_response();
  }
  let presented = match parts.uri.path().strip_prefix("/mcp") {
    Some("" | "/") => bearer(&parts.headers),
    Some(rest) => rest.strip_prefix('/'),
    None => return StatusCode::NOT_FOUND.into_response(),
  };
  if !presented.is_some_and(|presented| same_secret(presented, &context.token)) {
    return (StatusCode::UNAUTHORIZED, [(WWW_AUTHENTICATE, "Bearer")]).into_response();
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
        result["_meta"] = json!({ (META_SERVER): { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") } });
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
      "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
      "instructions": tools::INSTRUCTIONS,
    })),
    "ping" => Ok(json!({})),
    "tools/list" => Ok(json!({ "tools": tools::list(context.allow_send) })),
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
      "instructions": tools::INSTRUCTIONS,
      "ttlMs": 3_600_000,
      "cacheScope": "public",
    })),
    // The list changes only when the user turns sending on or off, which restarts the server.
    "tools/list" => Ok(json!({ "tools": tools::list(context.allow_send), "ttlMs": 60_000, "cacheScope": "public" })),
    "tools/call" => {
      let name = params["name"].as_str().unwrap_or_default();
      let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
      let unknown = || Refusal::new(StatusCode::OK, INVALID_PARAMS, format!("Unknown tool: {name}"));
      tools::call(&context.engine, context.allow_send, name, arguments)
        .await
        .ok_or_else(unknown)
    },
    _ => Err(Refusal::new(
      StatusCode::NOT_FOUND,
      METHOD_NOT_FOUND,
      format!("Method not found: {method}"),
    )),
  }
}

#[cfg(test)]
mod tests {
  use super::{bearer, header_text, origin_allowed, same_secret};
  use axum::http::HeaderMap;
  use axum::http::header::{AUTHORIZATION, ORIGIN};
  use base64::Engine as _;

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
