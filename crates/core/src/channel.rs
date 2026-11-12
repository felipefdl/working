//! Realtime stream: Google's WebChannel (long-lived HTTP streaming) carrying base64 protobuf events.
//!
//! Flow: `register` sets the stream cookie; an `init` request opens a session (its SID arrives in a response
//! header) and keeps streaming; when a response ends, a new request resumes from the last acknowledged array id.
#![allow(
  clippy::as_conversions,
  clippy::indexing_slicing,
  clippy::string_slice,
  reason = "WebChannel JSON arrays and protobuf i32 enums"
)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use prost::Message;

use crate::http::Http;
use crate::proto;

const EVENTS_URL: &str = "https://chat.google.com/webchannel/events_encoded";
const REGISTER_URL: &str = "https://chat.google.com/webchannel/register";
/// The server sends a keep-alive every 15 to 30 s; two missed in a row means the stream is dead.
const SILENCE_LIMIT: Duration = Duration::from_secs(60);

pub enum ChannelEvent {
  Online,
  Offline,
  Event(Box<proto::Event>),
}

#[derive(Default)]
struct Session {
  sid: Option<String>,
  /// Last array id received, acknowledged on the next request.
  aid: u64,
  /// Count of maps sent on this session.
  ofs: u64,
}

pub struct Channel {
  session: Mutex<Session>,
  request_id: AtomicU64,
  /// `WORKING_TRACE=1`: print one line per stream array to stderr (types only, never content).
  trace: bool,
}

impl Channel {
  pub fn new() -> Self {
    Self {
      session: Mutex::new(Session::default()),
      request_id: AtomicU64::new(rand::random_range(10_000..99_999)),
      trace: std::env::var("WORKING_TRACE").is_ok_and(|value| value == "1"),
    }
  }

  /// Runs forever: connects, streams, reconnects with backoff. Events go to `sink`.
  pub async fn run(&self, http: &Http, sink: &tokio::sync::mpsc::UnboundedSender<ChannelEvent>) {
    let mut failures = 0u32;
    loop {
      let had_session = self
        .session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .sid
        .is_some();
      let result = if had_session {
        self.poll(http, sink, false).await
      } else {
        self.open(http, sink).await
      };
      if matches!(result, Ok(())) {
        failures = 0;
      } else {
        failures += 1;
        // Any failure may mean the session is unknown to the server: start a new one.
        *self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Session::default();
        let _ = sink.send(ChannelEvent::Offline);
        tokio::time::sleep(Duration::from_secs(1 << failures.min(5))).await;
      }
    }
  }

  async fn open(&self, http: &Http, sink: &tokio::sync::mpsc::UnboundedSender<ChannelEvent>) -> Result<()> {
    let register = http
      .stream_client()
      .post(REGISTER_URL)
      .header("content-type", "application/x-protobuf")
      .send()
      .await?;
    if !register.status().is_success() {
      bail!("stream registration answered {}", register.status());
    }
    self.poll(http, sink, true).await
  }

  /// One streaming request: reads arrays until the server ends the response.
  async fn poll(&self, http: &Http, sink: &tokio::sync::mpsc::UnboundedSender<ChannelEvent>, init: bool) -> Result<()> {
    let request = http.stream_client().get(EVENTS_URL);
    let request = if init {
      request.query(&[
        ("VER", "8"),
        ("RID", "0"),
        ("CVER", "22"),
        ("TYPE", "init"),
        ("$req", "count=0"),
        ("SID", "null"),
        ("t", "1"),
      ])
    } else {
      let (sid, aid) = {
        let session = self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        (session.sid.clone().unwrap_or_default(), session.aid.to_string())
      };
      request.query(&[
        ("VER", "8"),
        ("RID", "rpc"),
        ("SID", sid.as_str()),
        ("AID", aid.as_str()),
        ("CI", "0"),
        ("TYPE", "xmlhttp"),
        ("t", "1"),
      ])
    };
    let mut response = request.send().await?;
    if !response.status().is_success() {
      bail!("stream answered {}", response.status());
    }

    if let Some(initial) = response.headers().get("x-http-initial-response") {
      // [[0,["c","<sid>","",8,12,30000]]]
      let initial: serde_json::Value = serde_json::from_slice(initial.as_bytes())?;
      let sid = initial[0][1][1]
        .as_str()
        .ok_or_else(|| anyhow!("stream gave no session id"))?
        .to_string();
      *self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
        Session { sid: Some(sid), aid: 0, ofs: 0 };
      self.send(http, &ping()).await?;
    }
    let _ = sink.send(ChannelEvent::Online);

    let mut parser = ChunkParser::default();
    loop {
      let chunk = tokio::time::timeout(SILENCE_LIMIT, response.chunk())
        .await
        .map_err(|_| anyhow!("stream went silent"))??;
      let Some(chunk) = chunk else { return Ok(()) };
      parser.push(&chunk);
      while let Some(payload) = parser.next() {
        self.handle_payload(&payload, sink)?;
      }
    }
  }

  fn handle_payload(&self, payload: &str, sink: &tokio::sync::mpsc::UnboundedSender<ChannelEvent>) -> Result<()> {
    let arrays: serde_json::Value = serde_json::from_str(payload)?;
    for array in arrays.as_array().into_iter().flatten() {
      if let Some(aid) = array[0].as_u64() {
        let mut session = self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        session.aid = session.aid.max(aid);
      }
      // Either "noop" (keep-alive) or {"data": "<base64 StreamEventsResponse>"}.
      let Some(data) = array[1][0]["data"].as_str() else {
        if self.trace {
          tracing::debug!("stream: {}", array[1][0].as_str().unwrap_or("array without data"));
        }
        continue;
      };
      let decoded = BASE64
        .decode(data)
        .ok()
        .and_then(|bytes| proto::StreamEventsResponse::decode(bytes.as_slice()).ok());
      let Some(response) = decoded else {
        if self.trace {
          tracing::debug!("stream: undecodable event of {} base64 chars", data.len());
        }
        continue;
      };
      if let Some(event) = response.event {
        if self.trace {
          let kinds: Vec<i32> = event
            .body
            .iter()
            .chain(&event.bodies)
            .filter_map(|body| body.event_type)
            .collect();
          tracing::debug!("stream: event type {:?}, body types {kinds:?}", event.r#type);
        }
        let _ = sink.send(ChannelEvent::Event(Box::new(event)));
      }
    }
    Ok(())
  }

  /// Sends one request up the stream (ping, group subscription).
  pub async fn send(&self, http: &Http, request: &proto::StreamEventsRequest) -> Result<()> {
    let (sid, aid, ofs) = {
      let mut session = self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      let Some(sid) = session.sid.clone() else {
        bail!("the stream is not connected")
      };
      let ofs = session.ofs;
      session.ofs += 1;
      (sid, session.aid.to_string(), ofs)
    };
    let rid = self.request_id.fetch_add(1, Ordering::Relaxed).to_string();
    let data = format!(r#"{{"data": "{}"}}"#, BASE64.encode(request.encode_to_vec()));
    let body = format!("count=1&ofs={ofs}&req0___data__={}", urlencoding::encode(&data));
    let response = http
      .stream_client()
      .post(EVENTS_URL)
      .query(&[
        ("VER", "8"),
        ("RID", rid.as_str()),
        ("SID", sid.as_str()),
        ("AID", aid.as_str()),
        ("CI", "0"),
        ("t", "1"),
      ])
      .header("content-type", "application/x-www-form-urlencoded")
      .timeout(Duration::from_secs(30))
      .body(body)
      .send()
      .await?;
    if !response.status().is_success() {
      bail!("stream send answered {}", response.status());
    }
    Ok(())
  }
}

/// Tells the server this client is active and in the foreground, which turns event delivery on.
pub fn ping() -> proto::StreamEventsRequest {
  proto::StreamEventsRequest {
    ping_event: Some(proto::PingEvent {
      state: Some(proto::ping_event::State::Active as i32),
      application_focus_state: Some(proto::ping_event::ApplicationFocusState::FocusStateForeground as i32),
      client_interactive_state: Some(proto::ping_event::ClientInteractiveState::Interactive as i32),
      client_notifications_enabled: Some(true),
      ..Default::default()
    }),
    ..Default::default()
  }
}

/// Splits the stream into payloads. Each is prefixed by its length in UTF-16 code units and a newline.
#[derive(Default)]
struct ChunkParser {
  buffer: Vec<u8>,
}

impl ChunkParser {
  fn push(&mut self, bytes: &[u8]) {
    self.buffer.extend_from_slice(bytes);
  }

  fn next(&mut self) -> Option<String> {
    let newline = self.buffer.iter().position(|&byte| byte == b'\n')?;
    let units: usize = std::str::from_utf8(&self.buffer[..newline]).ok()?.trim().parse().ok()?;
    let body = &self.buffer[newline + 1..];
    // The tail may end inside a multi-byte character; only the valid prefix counts.
    let text = match std::str::from_utf8(body) {
      Ok(text) => text,
      Err(error) => std::str::from_utf8(&body[..error.valid_up_to()]).ok()?,
    };
    let mut seen = 0;
    let mut end = None;
    for (index, character) in text.char_indices() {
      if seen == units {
        end = Some(index);
        break;
      }
      seen += character.len_utf16();
    }
    let end = match end {
      Some(end) => end,
      None if seen == units => text.len(),
      None => return None,
    };
    let payload = text[..end].to_string();
    self.buffer.drain(..newline + 1 + end);
    Some(payload)
  }
}
