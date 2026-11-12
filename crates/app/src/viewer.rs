//! In-app media viewer: a full-window modal for images (zoom, pan, context menu) and for files, which are
//! downloaded to the temp dir and shown in an embedded web view (video, audio, PDF, text) or as an info card.

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gchat_core::{Attachment, AttachmentKind, Core};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_webview::WebView;
use wry::WebViewBuilder;

use crate::images::{self, Images};
use crate::menu::{self, MenuItem};
use crate::theme::{ACCENT, ACCENT_BG, BORDER, MUTED, PANEL, ROW_HOVER, TEXT, WHITE, rgb};
use crate::ui::{Icon, icon};

/// Same height as the top bar, so the name sits beside the traffic lights.
const TOOLBAR_HEIGHT: f32 = crate::root::TOP_BAR_HEIGHT;
/// Decoded originals are bounded so a huge image stays within GPU texture limits.
const MAX_DIMENSION: u32 = 8192;
const ZOOM_STEP: f32 = 1.25;
/// Largest slice of a file answered to one range request.
const CHUNK: u64 = 4 * 1024 * 1024;

/// The window's viewer, reachable from any click handler.
pub struct ViewerHandle(pub Entity<Viewer>);

impl Global for ViewerHandle {}

/// Opens an attachment the way the user expects: images and files in the viewer, Drive files in the browser,
/// Meet links in the call window or the browser.
pub fn open_attachment(att: &Attachment, window: &mut Window, cx: &mut App) {
  let att = att.clone();
  match att.kind {
    AttachmentKind::Meet => crate::call::open(&att.url, cx),
    AttachmentKind::Drive => cx.open_url(&att.url),
    AttachmentKind::LinkPreview if att.thumb_url.is_none() => cx.open_url(&att.url),
    _ => {
      let viewer = cx.global::<ViewerHandle>().0.clone();
      viewer.update(cx, |viewer, cx| viewer.open(att, window, cx));
    },
  }
}

/// Whether the attachment is a video (shown as a poster tile with a play button).
pub fn is_video(att: &Attachment) -> bool {
  classify(att) == Class::Video
}

/// Poster image of a video: the attachment's own thumbnail, else the thumbnail endpoint of the same upload.
pub fn poster_url(att: &Attachment) -> Option<String> {
  if att.thumb_url.is_some() {
    return att.thumb_url.clone();
  }
  att
    .url
    .contains("url_type=DOWNLOAD_URL")
    .then(|| format!("{}&sz=w1024", att.url.replace("url_type=DOWNLOAD_URL", "url_type=FIFE_URL")))
}

/// Context menu of an image: copy, save, open in the browser.
pub fn image_menu(core: &Core, att: &Attachment, at: Point<Pixels>, window: &mut Window, cx: &mut App) {
  let (copy_core, copy_att) = (core.clone(), att.clone());
  let (save_core, save_att) = (core.clone(), att.clone());
  let url = att.url.clone();
  let items = vec![
    MenuItem::new("Copy Image", move |_, cx| copy_image(&copy_core, &copy_att, cx)),
    MenuItem::new("Save Image As…", move |_, cx| save_attachment(&save_core, &save_att, cx)),
    MenuItem::new("Open in Browser", move |_, cx| cx.open_url(&url)),
  ];
  menu::show(at, items, window, cx);
}

// Shared actions.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
  Image,
  Video,
  Audio,
  /// Rendered by the web view as is (PDF, plain text).
  Document,
  Other,
}

fn extension(name: &str) -> String {
  name
    .rsplit_once('.')
    .map(|(_, ext)| ext.to_ascii_lowercase())
    .unwrap_or_default()
}

fn classify(att: &Attachment) -> Class {
  let mime = att.mime.to_ascii_lowercase();
  let ext = extension(&att.name);
  if att.kind == AttachmentKind::Image || att.kind == AttachmentKind::LinkPreview || mime.starts_with("image/") {
    Class::Image
  } else if att.kind == AttachmentKind::Video
    || mime.starts_with("video/")
    || matches!(ext.as_str(), "mp4" | "mov" | "m4v" | "webm")
  {
    Class::Video
  } else if mime.starts_with("audio/") || matches!(ext.as_str(), "mp3" | "m4a" | "wav" | "aac" | "ogg" | "oga" | "flac")
  {
    Class::Audio
  } else if matches!(
    mime.as_str(),
    "application/pdf" | "text/plain" | "text/csv" | "application/json"
  ) || matches!(ext.as_str(), "pdf" | "txt" | "csv" | "json" | "log")
  {
    Class::Document
  } else if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp") {
    Class::Image
  } else {
    Class::Other
  }
}

/// Content type served to the web view: the attachment's own, else by extension.
fn content_type(att: &Attachment) -> String {
  if att.mime.contains('/') && att.mime != "application/octet-stream" {
    return att.mime.clone();
  }
  match extension(&att.name).as_str() {
    "mp4" | "m4v" => "video/mp4",
    "mov" => "video/quicktime",
    "webm" => "video/webm",
    "mp3" => "audio/mpeg",
    "m4a" => "audio/mp4",
    "aac" => "audio/aac",
    "wav" => "audio/wav",
    "ogg" | "oga" => "audio/ogg",
    "flac" => "audio/flac",
    "pdf" => "application/pdf",
    "csv" => "text/csv",
    "json" => "application/json",
    "txt" | "log" => "text/plain",
    _ => "application/octet-stream",
  }
  .to_string()
}

/// Where the full-resolution image comes from: the download URL, or for a link preview its image.
fn image_sources(att: &Attachment) -> Vec<String> {
  let mut sources = Vec::with_capacity(2);
  if att.kind == AttachmentKind::LinkPreview {
    sources.extend(att.thumb_url.clone());
  } else {
    sources.push(att.url.clone());
    sources.extend(att.thumb_url.clone().filter(|thumb| *thumb != att.url));
  }
  sources
}

/// Bytes of the best image available: the original, else the thumbnail.
async fn fetch_image_bytes(core: &Core, att: &Attachment) -> Option<Arc<Vec<u8>>> {
  for url in image_sources(att) {
    match core.fetch(url).await {
      Ok(bytes) if image::guess_format(&bytes).is_ok() => return Some(bytes),
      Ok(_) => tracing::warn!("attachment is not an image: {}", att.name),
      Err(err) => tracing::warn!("attachment fetch failed: {err}"),
    }
  }
  None
}

/// Image data in a format other apps paste: PNG and JPEG as they are, anything else re-encoded as PNG.
fn clipboard_image(bytes: &[u8]) -> Option<Image> {
  match image::guess_format(bytes).ok()? {
    image::ImageFormat::Png => Some(Image::from_bytes(ImageFormat::Png, bytes.to_vec())),
    image::ImageFormat::Jpeg => Some(Image::from_bytes(ImageFormat::Jpeg, bytes.to_vec())),
    _ => {
      let decoded = image::load_from_memory(bytes).ok()?;
      let mut out = std::io::Cursor::new(Vec::new());
      decoded.write_to(&mut out, image::ImageFormat::Png).ok()?;
      Some(Image::from_bytes(ImageFormat::Png, out.into_inner()))
    },
  }
}

pub fn copy_image(core: &Core, att: &Attachment, cx: &mut App) {
  let (core, att) = (core.clone(), att.clone());
  cx.spawn(async move |cx| {
    let Some(bytes) = fetch_image_bytes(&core, &att).await else {
      return;
    };
    let image = cx.background_executor().spawn(async move { clipboard_image(&bytes) }).await;
    if let Some(image) = image {
      cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_image(&image)));
    }
  })
  .detach();
}

/// Asks where to save with the native panel (named after the attachment), then writes the file.
pub fn save_attachment(core: &Core, att: &Attachment, cx: &mut App) {
  let downloads = std::env::var_os("HOME")
    .map(PathBuf::from)
    .unwrap_or_default()
    .join("Downloads");
  let name = if att.name.is_empty() {
    "attachment".to_string()
  } else {
    att.name.clone()
  };
  let choice = cx.prompt_for_new_path(&downloads, Some(&name));
  let (core, att) = (core.clone(), att.clone());
  cx.spawn(async move |cx| {
    let Ok(Ok(Some(path))) = choice.await else {
      return;
    };
    let bytes = if classify(&att) == Class::Image {
      fetch_image_bytes(&core, &att).await
    } else {
      core
        .fetch(att.url.clone())
        .await
        .map_err(|err| tracing::warn!("attachment fetch failed: {err}"))
        .ok()
    };
    let Some(bytes) = bytes else { return };
    let written = cx
      .background_executor()
      .spawn(async move { std::fs::write(&path, bytes.as_slice()) })
      .await;
    if let Err(err) = written {
      tracing::warn!("saving the attachment failed: {err}");
    }
  })
  .detach();
}

fn human_size(bytes: u64) -> String {
  const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
  let mut value = bytes as f64;
  let mut unit = 0;
  while value >= 1024. && unit + 1 < UNITS.len() {
    value /= 1024.;
    unit += 1;
  }
  if unit == 0 {
    format!("{bytes} B")
  } else {
    format!("{value:.1} {}", UNITS[unit])
  }
}

// Files in the web view.

fn temp_file(att: &Attachment, bytes: &[u8]) -> std::io::Result<PathBuf> {
  let mut hasher = DefaultHasher::new();
  att.url.hash(&mut hasher);
  let name: String = att
    .name
    .chars()
    .map(|c| {
      if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
        c
      } else {
        '_'
      }
    })
    .collect();
  let dir = std::env::temp_dir().join("working-media");
  std::fs::create_dir_all(&dir)?;
  let path = dir.join(format!("{:016x}-{name}", hasher.finish()));
  if std::fs::metadata(&path).ok().map(|meta| meta.len()) != Some(bytes.len() as u64) {
    std::fs::write(&path, bytes)?;
  }
  Ok(path)
}

const fn media_page(class: Class) -> &'static str {
  match class {
    Class::Audio => concat!(
      "<!doctype html><meta charset=utf-8><style>html,body{margin:0;height:100%;background:#1e1f20;display:flex;",
      "align-items:center;justify-content:center}audio{width:min(560px,90%)}</style>",
      "<audio controls autoplay src=\"/media\"></audio>"
    ),
    _ => concat!(
      "<!doctype html><meta charset=utf-8><style>html,body{margin:0;height:100%;background:#000;overflow:hidden}",
      "video{width:100%;height:100%;object-fit:contain;background:#000;outline:none}</style>",
      "<video controls autoplay playsinline src=\"/media\"></video>"
    ),
  }
}

fn parse_range(header: &str, len: u64) -> Option<(u64, u64)> {
  let (from, to) = header.strip_prefix("bytes=")?.split_once('-')?;
  let (start, end) = if from.is_empty() {
    (len.saturating_sub(to.parse().ok()?), len.checked_sub(1)?)
  } else {
    (
      from.parse().ok()?,
      if to.is_empty() {
        len.checked_sub(1)?
      } else {
        to.parse().ok()?
      },
    )
  };
  if start >= len || end < start {
    return None;
  }
  Some((start, end.min(len - 1).min(start + CHUNK - 1)))
}

fn read_range(path: &Path, start: u64, end: u64) -> std::io::Result<Vec<u8>> {
  let mut file = std::fs::File::open(path)?;
  file.seek(SeekFrom::Start(start))?;
  let mut out = Vec::with_capacity((end - start + 1) as usize);
  file.take(end - start + 1).read_to_end(&mut out)?;
  Ok(out)
}

/// Serves the page and the file (with byte ranges, which video seeking needs) under `media://localhost/`.
fn serve(
  path: PathBuf,
  len: u64,
  content_type: String,
  page: Option<&'static str>,
) -> impl Fn(wry::WebViewId, wry::http::Request<Vec<u8>>) -> wry::http::Response<Cow<'static, [u8]>> + 'static {
  move |_, request| {
    let response = wry::http::Response::builder();
    match request.uri().path() {
      "/index.html" if page.is_some() => response
        .header("Content-Type", "text/html; charset=utf-8")
        .body(Cow::Borrowed(page.unwrap_or_default().as_bytes())),
      "/media" => {
        let range = request
          .headers()
          .get("range")
          .and_then(|value| value.to_str().ok())
          .and_then(|value| parse_range(value, len));
        let (status, start, end) = match range {
          Some((start, end)) => (206, start, end),
          None => (200, 0, len.saturating_sub(1)),
        };
        let body = if len == 0 {
          Ok(Vec::new())
        } else {
          read_range(&path, start, end)
        };
        match body {
          Ok(body) => {
            let response = response
              .status(status)
              .header("Content-Type", content_type.as_str())
              .header("Accept-Ranges", "bytes");
            let response = if status == 206 {
              response.header("Content-Range", format!("bytes {start}-{end}/{len}"))
            } else {
              response
            };
            response.body(Cow::Owned(body))
          },
          Err(_) => response.status(500).body(Cow::Borrowed(&[][..])),
        }
      },
      _ => response.status(404).body(Cow::Borrowed(&[][..])),
    }
    .unwrap_or_else(|_| wry::http::Response::new(Cow::Borrowed(&[][..])))
  }
}

// State.

struct ImageView {
  thumb_url: Option<String>,
  full: Option<Arc<RenderImage>>,
  loading: bool,
  /// Natural size in logical pixels; filled from the attachment, else from the first decoded image.
  natural: Option<(f32, f32)>,
  /// Display scale where 1.0 is natural size; None fits the window.
  scale: Option<f32>,
  /// Offset of the image center from the viewport center.
  pan: (f32, f32),
  /// Pointer and pan at the start of a drag.
  drag: Option<((f32, f32), (f32, f32))>,
}

enum FileState {
  Loading,
  Failed(String),
  /// Cannot be previewed.
  Info {
    len: u64,
  },
  Preview {
    web: Entity<WebView>,
    _escape: Task<()>,
  },
}

enum Body {
  Image(ImageView),
  File(FileState),
}

struct Open {
  att: Attachment,
  generation: u64,
  body: Body,
  temp: Option<PathBuf>,
}

pub struct Viewer {
  core: Core,
  images: Images,
  state: Option<Open>,
  generation: u64,
  focus: FocusHandle,
  previous_focus: Option<FocusHandle>,
}

fn fit_scale(natural: (f32, f32), viewport: (f32, f32)) -> f32 {
  (viewport.0 / natural.0).min(viewport.1 / natural.1).min(1.0)
}

impl Viewer {
  pub fn new(core: Core, images: Images, cx: &mut Context<Self>) -> Self {
    Self {
      core,
      images,
      state: None,
      generation: 0,
      focus: cx.focus_handle(),
      previous_focus: None,
    }
  }

  pub const fn is_open(&self) -> bool {
    self.state.is_some()
  }

  pub fn open(&mut self, att: Attachment, window: &mut Window, cx: &mut Context<Self>) {
    if self.state.is_none() {
      self.previous_focus = window.focused(cx);
    }
    self.drop_state(cx);
    self.generation += 1;
    let generation = self.generation;
    window.focus(&self.focus, cx);
    if classify(&att) == Class::Image {
      self.open_image(att, generation, window, cx);
    } else {
      self.open_file(att, generation, window, cx);
    }
    cx.notify();
  }

  pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.state.is_none() {
      return;
    }
    self.drop_state(cx);
    if let Some(handle) = self.previous_focus.take() {
      window.focus(&handle, cx);
    }
    cx.notify();
  }

  /// Drops the open item, stopping any playback first, and deletes its temp file in the background.
  fn drop_state(&mut self, cx: &mut Context<Self>) {
    let Some(open) = self.state.take() else {
      return;
    };
    if let Body::File(FileState::Preview { web, .. }) = &open.body {
      let raw = web.read(cx).raw();
      raw.evaluate_script("document.querySelectorAll('video,audio').forEach(function(m){m.pause();m.removeAttribute('src');m.load();});").ok();
      raw.load_url("about:blank").ok();
    }
    if let Some(path) = open.temp {
      cx.background_executor()
        .spawn(async move { std::fs::remove_file(path).ok() })
        .detach();
    }
  }

  fn open_image(&mut self, att: Attachment, generation: u64, window: &mut Window, cx: &mut Context<Self>) {
    let scale_factor = window.scale_factor();
    let natural = match (att.width, att.height) {
      (Some(w), Some(h)) => Some((w as f32 / scale_factor, h as f32 / scale_factor)),
      _ => None,
    };
    let view = ImageView {
      thumb_url: att.thumb_url.clone(),
      full: None,
      loading: true,
      natural,
      scale: None,
      pan: (0., 0.),
      drag: None,
    };
    self.state = Some(Open {
      att: att.clone(),
      generation,
      body: Body::Image(view),
      temp: None,
    });
    let core = self.core.clone();
    cx.spawn(async move |this, cx| {
      let mut original = None;
      for url in image_sources(&att) {
        let Ok(bytes) = core.fetch(url).await else {
          continue;
        };
        original = cx
          .background_executor()
          .spawn(async move { images::decode(&bytes, MAX_DIMENSION) })
          .await;
        if original.is_some() {
          break;
        }
      }
      this.update(cx, |this, cx| this.original_ready(generation, original, cx)).ok();
    })
    .detach();
  }

  fn original_ready(&mut self, generation: u64, image: Option<Arc<RenderImage>>, cx: &mut Context<Self>) {
    let Some(Open {
      generation: current,
      body: Body::Image(view),
      ..
    }) = &mut self.state
    else {
      return;
    };
    if *current != generation {
      return;
    }
    view.loading = false;
    view.full = image;
    cx.notify();
  }

  fn open_file(&mut self, att: Attachment, generation: u64, window: &mut Window, cx: &mut Context<Self>) {
    self.state = Some(Open {
      att: att.clone(),
      generation,
      body: Body::File(FileState::Loading),
      temp: None,
    });
    let core = self.core.clone();
    let background = cx.background_executor().clone();
    cx.spawn_in(window, async move |this, cx| {
      let downloaded = match core.fetch(att.url.clone()).await.map_err(|err| err.to_string()) {
        Ok(bytes) => {
          let att = att.clone();
          background
            .spawn(async move {
              temp_file(&att, &bytes)
                .map(|path| (path, bytes.len() as u64))
                .map_err(|err| err.to_string())
            })
            .await
        },
        Err(err) => Err(err),
      };
      cx.update(|window, cx| this.update(cx, |this, cx| this.file_ready(generation, downloaded, window, cx)))
        .ok();
    })
    .detach();
  }

  fn file_ready(
    &mut self,
    generation: u64,
    downloaded: Result<(PathBuf, u64), String>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(open) = &mut self.state else { return };
    if open.generation != generation {
      return;
    }
    let state = match downloaded {
      Err(err) => FileState::Failed(err),
      Ok((path, len)) => {
        open.temp = Some(path.clone());
        match classify(&open.att) {
          Class::Other => FileState::Info { len },
          class => match self.preview(&path, len, class, window, cx) {
            Ok(state) => state,
            Err(err) => {
              tracing::warn!("web view preview failed: {err}");
              FileState::Info { len }
            },
          },
        }
      },
    };
    if let Some(Open { body, .. }) = &mut self.state {
      *body = Body::File(state);
    }
    cx.notify();
  }

  fn preview(
    &self,
    path: &Path,
    len: u64,
    class: Class,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Result<FileState, String> {
    let Some(open) = &self.state else {
      return Err("closed".into());
    };
    let page = match class {
      Class::Video | Class::Audio => Some(media_page(class)),
      _ => None,
    };
    let url = if page.is_some() {
      "media://localhost/index.html"
    } else {
      "media://localhost/media"
    };
    let (escape_tx, escape_rx) = flume::unbounded::<()>();
    let builder = WebViewBuilder::new()
      .with_autoplay(true)
      .with_background_color((0, 0, 0, 255))
      .with_custom_protocol("media".into(), serve(path.to_path_buf(), len, content_type(&open.att), page))
      .with_initialization_script(
        "window.addEventListener('keydown',function(e){if(e.key==='Escape'){window.ipc.postMessage('close');}},true);",
      )
      .with_ipc_handler(move |_| {
        let _ = escape_tx.send(());
      })
      .with_url(url);
    let web = WebView::build(builder, window, cx).map_err(|err| err.to_string())?;
    web.set_theme(wry::Theme::Dark).ok();
    let web = cx.new(|_| web);
    let escape = cx.spawn_in(window, async move |this, cx| {
      while escape_rx.recv_async().await.is_ok() {
        cx.update(|window, cx| this.update(cx, |this, cx| this.close(window, cx))).ok();
      }
    });
    Ok(FileState::Preview { web, _escape: escape })
  }

  // Zoom.

  fn viewport(window: &Window) -> (f32, f32) {
    let size = window.viewport_size();
    (f32::from(size.width), f32::from(size.height) - TOOLBAR_HEIGHT)
  }

  /// Offset of `position` from the viewport center.
  fn pivot(position: Point<Pixels>, viewport: (f32, f32)) -> (f32, f32) {
    (
      f32::from(position.x) - viewport.0 / 2.,
      f32::from(position.y) - TOOLBAR_HEIGHT - viewport.1 / 2.,
    )
  }

  const fn image_view(&mut self) -> Option<&mut ImageView> {
    match &mut self.state {
      Some(Open { body: Body::Image(view), .. }) => Some(view),
      _ => None,
    }
  }

  /// Sets the display scale, keeping the image point under `pivot` (relative to the viewport center) in place.
  fn zoom_to(&mut self, target: f32, pivot: (f32, f32), window: &Window, cx: &mut Context<Self>) {
    let viewport = Self::viewport(window);
    let Some(view) = self.image_view() else {
      return;
    };
    let Some(natural) = view.natural else { return };
    let fit = fit_scale(natural, viewport);
    let old = view.scale.unwrap_or(fit);
    let new = target.clamp(fit, (fit * 16.).max(8.));
    if new <= fit * 1.001 {
      view.scale = None;
      view.pan = (0., 0.);
    } else {
      let ratio = new / old;
      view.pan = (
        (pivot.0 - view.pan.0).mul_add(-ratio, pivot.0),
        (pivot.1 - view.pan.1).mul_add(-ratio, pivot.1),
      );
      view.scale = Some(new);
      clamp_pan(view, natural, viewport);
    }
    cx.notify();
  }

  fn zoom_by(&mut self, factor: f32, pivot: (f32, f32), window: &Window, cx: &mut Context<Self>) {
    let viewport = Self::viewport(window);
    let Some(view) = self.image_view() else {
      return;
    };
    let Some(natural) = view.natural else { return };
    let current = view.scale.unwrap_or(fit_scale(natural, viewport));
    self.zoom_to(current * factor, pivot, window, cx);
  }

  fn zoom_fit(&mut self, cx: &mut Context<Self>) {
    if let Some(view) = self.image_view() {
      view.scale = None;
      view.pan = (0., 0.);
      cx.notify();
    }
  }

  fn toggle_zoom(&mut self, pivot: (f32, f32), window: &Window, cx: &mut Context<Self>) {
    let viewport = Self::viewport(window);
    let Some(view) = self.image_view() else {
      return;
    };
    let Some(natural) = view.natural else { return };
    if view.scale.is_some() {
      self.zoom_fit(cx);
    } else {
      let fit = fit_scale(natural, viewport);
      self.zoom_to(if fit < 0.999 { 1.0 } else { 2.0 }, pivot, window, cx);
    }
  }

  fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
    let modifiers = event.keystroke.modifiers;
    if modifiers.platform || modifiers.control || modifiers.alt {
      return;
    }
    match event.keystroke.key.as_str() {
      "escape" => self.close(window, cx),
      "+" | "=" => self.zoom_by(ZOOM_STEP, (0., 0.), window, cx),
      "-" => self.zoom_by(1. / ZOOM_STEP, (0., 0.), window, cx),
      "0" => self.zoom_fit(cx),
      "1" => self.zoom_to(1.0, (0., 0.), window, cx),
      _ => return,
    }
    cx.stop_propagation();
  }

  fn on_wheel(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
    if self.image_view().is_none() {
      return;
    }
    let dy = f32::from(event.delta.pixel_delta(px(18.)).y);
    let pivot = Self::pivot(event.position, Self::viewport(window));
    self.zoom_by((dy * 0.004).exp(), pivot, window, cx);
    cx.stop_propagation();
  }

  fn on_pinch(&mut self, event: &PinchEvent, window: &mut Window, cx: &mut Context<Self>) {
    if self.image_view().is_none() {
      return;
    }
    let pivot = Self::pivot(event.position, Self::viewport(window));
    self.zoom_by((1. + event.delta).max(0.1), pivot, window, cx);
    cx.stop_propagation();
  }

  fn on_drag(&mut self, event: &MouseMoveEvent, window: &Window, cx: &mut Context<Self>) {
    let viewport = Self::viewport(window);
    let Some(view) = self.image_view() else {
      return;
    };
    let Some((start, start_pan)) = view.drag else {
      return;
    };
    if event.pressed_button != Some(MouseButton::Left) {
      view.drag = None;
      return;
    }
    let Some(natural) = view.natural else { return };
    view.pan = (
      start_pan.0 + f32::from(event.position.x) - start.0,
      start_pan.1 + f32::from(event.position.y) - start.1,
    );
    clamp_pan(view, natural, viewport);
    cx.notify();
  }

  // Rendering.

  fn button(id: &'static str, glyph: Icon) -> Stateful<Div> {
    div()
      .id(id)
      .size(px(36.))
      .flex()
      .items_center()
      .justify_center()
      .rounded_full()
      .cursor_pointer()
      .hover(|s| s.bg(rgba(0xffffff1f)))
      .child(icon(glyph, 18., WHITE))
  }

  fn toolbar(&self, zoom_percent: Option<u32>, cx: &mut Context<Self>) -> Div {
    let Some(open) = &self.state else {
      return div();
    };
    let name = if open.att.name.is_empty() {
      open.att.url.clone()
    } else {
      open.att.name.clone()
    };
    let is_image = matches!(open.body, Body::Image(_));
    let (core, att) = (self.core.clone(), open.att.clone());
    let (copy_core, copy_att) = (core.clone(), att.clone());
    let url = att.url.clone();

    h_flex()
      .h(px(TOOLBAR_HEIGHT))
      .w_full()
      .flex_shrink_0()
      .pl(px(80.))
      .pr_4()
      .bg(gpui_kit::rgb(0x0b0b0c))
      .gap_1()
      .items_center()
      .text_color(gpui_kit::rgb(0xffffff))
      .child(
        div()
          .flex_1()
          .min_w_0()
          .truncate()
          .text_size(px(14.))
          .font_weight(FontWeight::MEDIUM)
          .child(name),
      )
      .when_some(zoom_percent, |bar, percent| {
        bar
          .child(
            Self::button("zoom-out", Icon::ZoomOut)
              .on_click(cx.listener(|this, _, window, cx| this.zoom_by(1. / ZOOM_STEP, (0., 0.), window, cx))),
          )
          .child(
            div()
              .id("zoom-100")
              .h(px(36.))
              .px_2()
              .flex()
              .items_center()
              .rounded_full()
              .cursor_pointer()
              .text_size(px(12.))
              .hover(|s| s.bg(rgba(0xffffff1f)))
              .on_click(cx.listener(|this, _, window, cx| this.zoom_to(1.0, (0., 0.), window, cx)))
              .child(format!("{percent}%")),
          )
          .child(
            Self::button("zoom-in", Icon::ZoomIn)
              .on_click(cx.listener(|this, _, window, cx| this.zoom_by(ZOOM_STEP, (0., 0.), window, cx))),
          )
          .child(Self::button("zoom-fit", Icon::Fit).on_click(cx.listener(|this, _, _, cx| this.zoom_fit(cx))))
          .child(div().w(px(1.)).h(px(20.)).mx_2().bg(rgba(0xffffff33)))
      })
      .when(is_image, |bar| {
        bar.child(Self::button("copy", Icon::Copy).on_click(move |_, _, cx| copy_image(&copy_core, &copy_att, cx)))
      })
      .child(Self::button("download", Icon::Download).on_click(move |_, _, cx| save_attachment(&core, &att, cx)))
      .child(Self::button("browser", Icon::ExternalLink).on_click(move |_, _, cx| cx.open_url(&url)))
      .child(Self::button("close", Icon::X).on_click(cx.listener(|this, _, window, cx| this.close(window, cx))))
  }

  fn render_image(&mut self, window: &mut Window, cx: &mut Context<Self>) -> (Option<u32>, AnyElement) {
    let viewport = Self::viewport(window);
    let scale_factor = window.scale_factor();
    let Some(Open { att, body: Body::Image(view), .. }) = &mut self.state else {
      return (None, div().into_any_element());
    };
    let thumb = view.thumb_url.as_deref().and_then(|url| self.images.get(url, 1600, cx));
    let image = view.full.clone().or(thumb);
    if view.natural.is_none()
      && let Some(image) = &image
    {
      let size = image.size(0);
      view.natural = Some((size.width.0 as f32 / scale_factor, size.height.0 as f32 / scale_factor));
    }
    let message = |text: &'static str| {
      div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgba(0xffffffb3))
        .child(text)
        .into_any_element()
    };
    let (Some(image), Some(natural)) = (image, view.natural) else {
      let text = if view.loading {
        "Loading…"
      } else {
        "Couldn’t load this image"
      };
      return (None, self.viewport_frame(message(text), cx));
    };

    let fit = fit_scale(natural, viewport);
    let scale = view.scale.unwrap_or(fit);
    let (width, height) = (natural.0 * scale, natural.1 * scale);
    let left = viewport.0 / 2. - width / 2. + view.pan.0;
    let top = viewport.1 / 2. - height / 2. + view.pan.1;
    let loading = view.loading && view.full.is_none();
    let (core, att) = (self.core.clone(), att.clone());

    let picture = div()
      .id("viewer-image")
      .absolute()
      .left(px(left))
      .top(px(top))
      .w(px(width))
      .h(px(height))
      .on_mouse_down(
        MouseButton::Left,
        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
          cx.stop_propagation();
          if event.click_count == 2 {
            let pivot = Self::pivot(event.position, Self::viewport(window));
            this.toggle_zoom(pivot, window, cx);
          } else if let Some(view) = this.image_view() {
            view.drag = Some(((f32::from(event.position.x), f32::from(event.position.y)), view.pan));
          }
        }),
      )
      .on_mouse_down(MouseButton::Right, move |event, window, cx| {
        cx.stop_propagation();
        image_menu(&core, &att, event.position, window, cx);
      })
      .child(img(image).size_full().object_fit(ObjectFit::Fill));

    let mut frame = div().size_full().relative().child(picture);
    if loading {
      frame = frame.child(
        div().absolute().bottom(px(16.)).w_full().flex().justify_center().child(
          div()
            .px_3()
            .py_1()
            .rounded_full()
            .bg(rgba(0x000000a6))
            .text_size(px(12.))
            .text_color(rgba(0xffffffcc))
            .child("Loading full resolution…"),
        ),
      );
    }
    (
      Some((scale * 100.).round() as u32),
      self.viewport_frame(frame.into_any_element(), cx),
    )
  }

  /// The area under the toolbar. A click on empty backdrop closes the viewer; a drag pans a zoomed image.
  fn viewport_frame(&self, content: AnyElement, cx: &mut Context<Self>) -> AnyElement {
    div()
      .flex_1()
      .min_h_0()
      .w_full()
      .relative()
      .overflow_hidden()
      .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close(window, cx)))
      .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| this.on_drag(event, window, cx)))
      .on_mouse_up(
        MouseButton::Left,
        cx.listener(|this, _, _, _| {
          if let Some(view) = this.image_view() {
            view.drag = None;
          }
        }),
      )
      .child(content)
      .into_any_element()
  }

  fn pill(id: &'static str, label: &'static str, primary: bool) -> Stateful<Div> {
    div()
      .id(id)
      .h(px(34.))
      .px_4()
      .flex()
      .items_center()
      .rounded_full()
      .border_1()
      .border_color(rgb(if primary { ACCENT } else { BORDER }))
      .when(primary, |d| d.bg(rgb(ACCENT_BG)))
      .text_size(px(13.))
      .text_color(rgb(TEXT))
      .cursor_pointer()
      .hover(|s| s.bg(rgb(ROW_HOVER)))
      .child(label)
  }

  fn render_file(&mut self, cx: &mut Context<Self>) -> AnyElement {
    let Some(Open { att, body: Body::File(state), .. }) = &self.state else {
      return div().into_any_element();
    };
    let centered = |child: AnyElement| div().size_full().flex().items_center().justify_center().child(child);
    let title = if att.name.is_empty() {
      att.url.clone()
    } else {
      att.name.clone()
    };
    let (core, att) = (self.core.clone(), att.clone());
    let url = att.url.clone();

    let card = |detail: String, with_save: bool| {
      let (core, att, url) = (core.clone(), att.clone(), url.clone());
      v_flex()
        .w(px(400.))
        .p_6()
        .gap_3()
        .items_center()
        .rounded(px(14.))
        .bg(rgb(PANEL))
        .border_1()
        .border_color(rgb(BORDER))
        .text_color(rgb(TEXT))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(icon(Icon::File, 40., MUTED))
        .child(
          div()
            .max_w_full()
            .text_size(px(16.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_center()
            .child(title.clone()),
        )
        .child(div().text_size(px(12.)).text_color(rgb(MUTED)).text_center().child(detail))
        .child(
          h_flex()
            .gap_2()
            .pt_2()
            .when(with_save, |row| {
              row.child(
                Self::pill("save-as", "Save As…", true).on_click(move |_, _, cx| save_attachment(&core, &att, cx)),
              )
            })
            .child(
              Self::pill("open-browser", "Open in Browser", !with_save).on_click(move |_, _, cx| cx.open_url(&url)),
            ),
        )
        .into_any_element()
    };

    let content = match state {
      FileState::Loading => centered(
        div()
          .text_color(rgba(0xffffffb3))
          .text_size(px(14.))
          .child(format!("Downloading {title}…"))
          .into_any_element(),
      )
      .into_any_element(),
      FileState::Failed(err) => centered(card(format!("Couldn’t download this file: {err}"), false)).into_any_element(),
      FileState::Info { len } => {
        let mime = if att.mime.is_empty() {
          "Unknown type".to_string()
        } else {
          att.mime.clone()
        };
        centered(card(format!("{mime} · {}\nNo preview available.", human_size(*len)), true)).into_any_element()
      },
      FileState::Preview { web, .. } => div().size_full().child(web.clone()).into_any_element(),
    };
    let backdrop_close = !matches!(state, FileState::Preview { .. });
    div()
      .flex_1()
      .min_h_0()
      .w_full()
      .relative()
      .overflow_hidden()
      .when(backdrop_close, |d| {
        d.on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close(window, cx)))
      })
      .child(content)
      .into_any_element()
  }
}

fn clamp_pan(view: &mut ImageView, natural: (f32, f32), viewport: (f32, f32)) {
  let scale = view.scale.unwrap_or_else(|| fit_scale(natural, viewport));
  let max_x = (natural.0.mul_add(scale, -viewport.0) / 2.).max(0.);
  let max_y = (natural.1.mul_add(scale, -viewport.1) / 2.).max(0.);
  view.pan = (view.pan.0.clamp(-max_x, max_x), view.pan.1.clamp(-max_y, max_y));
}

impl Render for Viewer {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let Some(open) = &self.state else {
      return div().into_any_element();
    };
    let (zoom_percent, body) = match open.body {
      Body::Image(_) => self.render_image(window, cx),
      Body::File(_) => (None, self.render_file(cx)),
    };
    let toolbar = self.toolbar(zoom_percent, cx);

    div()
      .absolute()
      .top_0()
      .left_0()
      .size_full()
      .bg(rgba(0x000000e6))
      .occlude()
      .track_focus(&self.focus)
      .key_context("Viewer")
      .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| this.on_key(event, window, cx)))
      .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| this.on_wheel(event, window, cx)))
      .on_pinch(cx.listener(|this, event: &PinchEvent, window, cx| this.on_pinch(event, window, cx)))
      .child(v_flex().size_full().child(toolbar).child(body))
      .into_any_element()
  }
}

#[cfg(test)]
mod tests {
  use super::{Class, classify, extension, human_size, parse_range, poster_url};
  use gchat_core::{Attachment, AttachmentKind};

  fn att(kind: AttachmentKind, name: &str, mime: &str) -> Attachment {
    Attachment {
      kind,
      name: name.into(),
      mime: mime.into(),
      url: "https://chat.google.com/api/get_attachment_url?url_type=DOWNLOAD_URL&x=1".into(),
      thumb_url: None,
      width: None,
      height: None,
    }
  }

  #[test]
  fn human_size_steps() {
    assert_eq!(human_size(512), "512 B");
    assert_eq!(human_size(1536), "1.5 KB");
    assert_eq!(human_size(1024 * 1024), "1.0 MB");
  }

  #[test]
  fn classify_from_kind_mime_and_extension() {
    assert_eq!(classify(&att(AttachmentKind::Image, "x.bin", "")), Class::Image);
    assert_eq!(classify(&att(AttachmentKind::File, "clip.mp4", "")), Class::Video);
    assert_eq!(classify(&att(AttachmentKind::File, "song.mp3", "audio/mpeg")), Class::Audio);
    assert_eq!(classify(&att(AttachmentKind::File, "doc.pdf", "")), Class::Document);
    assert_eq!(classify(&att(AttachmentKind::File, "notes.docx", "")), Class::Other);
    assert_eq!(extension("Photo.PNG"), "png");
  }

  #[test]
  fn parse_range_bytes() {
    assert_eq!(parse_range("bytes=0-10", 100), Some((0, 10)));
    assert_eq!(parse_range("bytes=0-", 100), Some((0, 99)));
    assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
    assert_eq!(parse_range("bytes=50-10", 100), None);
    assert_eq!(parse_range("bytes=100-200", 100), None);
    assert_eq!(parse_range("items=0-1", 100), None);
  }

  #[test]
  fn poster_url_swaps_download_to_fife() {
    let mut video = att(AttachmentKind::Video, "a.mp4", "video/mp4");
    assert_eq!(
      poster_url(&video).as_deref(),
      Some("https://chat.google.com/api/get_attachment_url?url_type=FIFE_URL&x=1&sz=w1024")
    );
    video.thumb_url = Some("https://thumb".into());
    assert_eq!(poster_url(&video).as_deref(), Some("https://thumb"));
  }
}
