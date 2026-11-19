//! The page an email's HTML is shown in, plain code with tests: the document around the email (its colors, and the
//! Content-Security-Policy that keeps the email's own scripts, frames, and forms from running), its images routed
//! through the app, and what the page's script reports back.

use std::fmt::Write as _;

/// The custom protocol each email's webview serves its page and images under.
pub const SCHEME: &str = "mailbody";

/// The page's address, as the custom protocol names it. `webview::build` loads it on every OS.
pub const PAGE_URL: &str = "mailbody://localhost/";

/// The page's path under the protocol.
pub const PAGE_PATH: &str = "/";

/// The path images are asked for at: `/image?u=<the image's address, percent-encoded>`.
const IMAGE_PATH: &str = "/image";

/// What the page may load and run: images and inline styles only, the images through the app (`'self'`). Nothing of
/// the email's runs (no scripts, frames, plugins, or forms), and nothing reaches the network but through the app.
pub const CSP: &str = "default-src 'none'; img-src 'self' mailbody: data:; style-src 'unsafe-inline'; font-src data:; \
  script-src 'none'; frame-src 'none'; child-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'; \
  connect-src 'none'; media-src 'none'";

/// The script of ours each page runs (the email's own cannot run: `CSP`). It fits an email wider than its card to the
/// card's width, scaling it down as Apple Mail does and leaving a narrower one at its own size; reports the height of
/// the email as shown, so the card sizes to it; sends the wheel's vertical turns to the app, so the reading pane
/// scrolls while the pointer is over an email; and closes the thread on Escape.
pub const SCRIPT: &str = "(function(){var last=0;function parts(){return [document.getElementById('working-frame'),\
  document.getElementById('working-mail')];}\
  function report(){var o=parts()[0];if(!o){return;}var h=Math.ceil(o.getBoundingClientRect().height);\
  if(h!==last){last=h;window.ipc.postMessage('height|'+h);}}\
  function fit(){var p=parts(),o=p[0],m=p[1];if(!o||!m){return;}m.style.transform='';m.style.width='';\
  o.style.height='';var room=o.clientWidth,wide=m.scrollWidth;if(wide>room+1){var k=room/wide;\
  m.style.width=wide+'px';m.style.transformOrigin='0 0';m.style.transform='scale('+k+')';\
  o.style.height=Math.ceil(m.offsetHeight*k)+'px';}report();}\
  document.addEventListener('DOMContentLoaded',function(){fit();var p=parts();if(p[0]&&p[1]&&window.ResizeObserver){\
  var r=new ResizeObserver(fit);r.observe(p[0]);r.observe(p[1]);}});window.addEventListener('load',fit);\
  window.addEventListener('resize',fit);\
  document.addEventListener('wheel',function(e){if(Math.abs(e.deltaX)>Math.abs(e.deltaY)){return;}\
  e.preventDefault();var d=e.deltaY*(e.deltaMode===1?16:(e.deltaMode===2?window.innerHeight:1));\
  window.ipc.postMessage('scroll|'+d);},{passive:false});\
  window.addEventListener('keydown',function(e){if(e.key==='Escape'){window.ipc.postMessage('close');}},true);})();";

/// The tallest an email's card gets, against a page that reports nonsense.
const MAX_HEIGHT: f32 = 200_000.0;

/// The colors a page follows by default, as `0xRRGGBB`: the email's own styles win over them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Colors {
  pub background: u32,
  pub text: u32,
  pub link: u32,
}

/// A light page, for an email whose own colors only read on white.
const LIGHT: Colors = Colors {
  background: 0xffffff,
  text: 0x202124,
  link: 0x1a73e8,
};

/// A dark page, for an email whose own colors only read on black.
const DARK: Colors = Colors {
  background: 0x1f1f1f,
  text: 0xe3e3e3,
  link: 0x8ab4f8,
};

impl Colors {
  /// The page for an email that reads badly on these: a light one when the background is dark, a dark one when it is
  /// light. Emails set their own text colors expecting white, so a dark theme hides the ones that set only that.
  pub const fn other(self) -> Self {
    if is_dark(self.background) {
      LIGHT
    } else {
      DARK
    }
  }
}

/// Whether a `0xRRGGBB` color is nearer black than white, by how bright it looks.
const fn is_dark(color: u32) -> bool {
  let (red, green, blue) = ((color >> 16) & 0xff, (color >> 8) & 0xff, color & 0xff);
  // Integer weights of perceived brightness, out of 1000.
  red * 299 + green * 587 + blue * 114 < 128_000
}

/// The whole page of an email: the policy, the theme's colors, then the email in the element the script scales
/// (`working-mail`) inside the one it measures (`working-frame`).
pub fn page(html: &str, colors: Colors) -> String {
  let Colors { background, text, link } = colors;
  format!(
    "<!doctype html><html><head><meta charset=\"utf-8\">\
     <meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\">\
     <style>html,body{{margin:0;padding:0;background:#{background:06x};color:#{text:06x};overflow:hidden}}\
     body{{font:14px/1.45 -apple-system,BlinkMacSystemFont,\"Segoe UI\",system-ui,sans-serif;\
     overflow-wrap:anywhere}}a{{color:#{link:06x}}}img{{max-width:100%;height:auto}}\
     #working-frame{{overflow:hidden}}#working-mail{{display:flow-root}}</style></head>\
     <body><div id=\"working-frame\"><div id=\"working-mail\">{}</div></div></body></html>",
    route_images(html)
  )
}

/// The email with each image the core fetches (`src` and `background` attributes inside tags, `fetched_by_app`)
/// asked of the app instead, which fetches it through the core: `/image?u=…`. Other values stay as they are.
pub fn route_images(html: &str) -> String {
  let lower = html.to_ascii_lowercase();
  let mut out = String::with_capacity(html.len());
  let mut copied = 0;
  let mut from = 0;
  while let Some((start, end)) = next_remote_value(&lower, from) {
    let remote = unescape(&html[start..end]);
    out.push_str(&html[copied..start]);
    out.push_str(IMAGE_PATH);
    out.push_str("?u=");
    out.push_str(&percent_encode(remote.trim()));
    copied = end;
    from = end;
  }
  out.push_str(&html[copied..]);
  out
}

/// The span of the next `src` or `background` value inside a tag, from byte `from` on, that names an address
/// `fetched_by_app` takes. `lower` is the HTML in ASCII lowercase, so its offsets are those of the HTML.
fn next_remote_value(lower: &str, mut from: usize) -> Option<(usize, usize)> {
  let bytes = lower.as_bytes();
  loop {
    let (at, name) = ["src", "background"]
      .iter()
      .filter_map(|name| lower[from..].find(name).map(|at| (from + at, *name)))
      .min_by_key(|(at, _)| *at)?;
    from = at + name.len();
    // An attribute: after a space, inside a tag (a `<` after the last `>`).
    let after_space = at > 0 && bytes[at - 1].is_ascii_whitespace();
    // `None < Some(_)`: no `>` before the `<` still counts as inside it.
    let in_tag = lower[..at].rfind('<') > lower[..at].rfind('>');
    if !after_space || !in_tag {
      continue;
    }
    let mut cursor = from;
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
      cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'=') {
      continue;
    }
    cursor += 1;
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
      cursor += 1;
    }
    let (start, end) = match bytes.get(cursor) {
      Some(&quote @ (b'"' | b'\'')) => {
        let start = cursor + 1;
        let end = lower[start..].find(char::from(quote)).map(|len| start + len)?;
        (start, end)
      },
      Some(_) => {
        let end = lower[cursor..]
          .find(|c: char| c.is_ascii_whitespace() || c == '>')
          .map_or(lower.len(), |len| cursor + len);
        (cursor, end)
      },
      None => return None,
    };
    from = end;
    if fetched_by_app(lower[start..end].trim_start()) {
      return Some((start, end));
    }
  }
}

/// An attribute value as the browser reads it: the character references email HTML puts in addresses decoded.
fn unescape(value: &str) -> String {
  value
    .replace("&amp;", "&")
    .replace("&#38;", "&")
    .replace("&quot;", "\"")
    .replace("&#39;", "'")
    .replace("&lt;", "<")
    .replace("&gt;", ">")
}

/// `text` with every byte but letters, digits, and `-_.~` as `%XX`.
fn percent_encode(text: &str) -> String {
  let mut out = String::with_capacity(text.len());
  for byte in text.bytes() {
    if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
      out.push(char::from(byte));
    } else {
      // Writing to a `String` cannot fail.
      let _ = write!(out, "%{byte:02X}");
    }
  }
  out
}

/// The text `percent_encode` made, or `None` when it is not valid.
fn percent_decode(text: &str) -> Option<String> {
  let bytes = text.as_bytes();
  let mut out = Vec::with_capacity(bytes.len());
  let mut at = 0;
  while let Some(&byte) = bytes.get(at) {
    if byte == b'%' {
      let hex = text.get(at + 1..at + 3)?;
      out.push(u8::from_str_radix(hex, 16).ok()?);
      at += 3;
    } else {
      out.push(byte);
      at += 1;
    }
  }
  String::from_utf8(out).ok()
}

/// The image a request of the page asks for, from its path and query: an address `fetched_by_app` takes, or `None`
/// for anything else.
pub fn image_request(path: &str, query: Option<&str>) -> Option<String> {
  if path != IMAGE_PATH {
    return None;
  }
  let encoded = query?.split('&').find_map(|pair| pair.strip_prefix("u="))?;
  let url = percent_decode(encoded)?;
  fetched_by_app(&url).then_some(url)
}

/// Whether an email's image address is one the app fetches through `Core::fetch`: `http` and `https`, and the
/// demo's photos (`demo:image/…`), which `Core::fetch` serves in demo mode and refuses otherwise. Every other scheme
/// is never asked for: `file:` and the demo's other addresses read the disk, `javascript:` is no image, and `data:`
/// images stay in the page as they are.
fn fetched_by_app(url: &str) -> bool {
  let lower = url.trim_start().to_ascii_lowercase();
  ["http://", "https://", "demo:image/"]
    .iter()
    .any(|scheme| lower.starts_with(scheme))
}

/// The content type of an image by its first bytes, for the page's image responses.
pub fn image_type(bytes: &[u8]) -> &'static str {
  match bytes {
    [0x89, b'P', b'N', b'G', ..] => "image/png",
    [0xff, 0xd8, 0xff, ..] => "image/jpeg",
    [b'G', b'I', b'F', b'8', ..] => "image/gif",
    [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
    [b'B', b'M', ..] => "image/bmp",
    _ if bytes.trim_ascii_start().starts_with(b"<svg") || bytes.trim_ascii_start().starts_with(b"<?xml") => {
      "image/svg+xml"
    },
    _ => "application/octet-stream",
  }
}

/// Whether a link of an email opens in the browser: web pages and mail addresses only.
pub fn opens_in_browser(url: &str) -> bool {
  let lower = url.to_ascii_lowercase();
  lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
}

/// What the page's script says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Report {
  /// The email is this tall, in pixels.
  Height(f32),
  /// The wheel turned this far down (up when negative), in pixels.
  Scroll(f32),
  /// Escape was pressed in the page.
  Close,
}

impl Report {
  /// A message of `SCRIPT`, or `None` for anything else.
  pub fn parse(message: &str) -> Option<Self> {
    if message == "close" {
      return Some(Self::Close);
    }
    let (kind, value) = message.split_once('|')?;
    let value: f32 = value.parse().ok().filter(|value: &f32| value.is_finite())?;
    match kind {
      "height" => Some(Self::Height(value.clamp(0., MAX_HEIGHT))),
      "scroll" => Some(Self::Scroll(value)),
      _ => None,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const COLORS: Colors = Colors {
    background: 0x131314,
    text: 0xe3e3e3,
    link: 0x8ab4f8,
  };

  #[test]
  fn the_page_carries_the_policy_and_the_theme_before_the_email() {
    let page = page("<p>Hi Jordan</p>", COLORS);
    let policy = page.find("Content-Security-Policy").unwrap();
    let email = page.find("<p>Hi Jordan</p>").unwrap();
    assert!(policy < email);
    assert!(page.contains("script-src 'none'") && page.contains("frame-src 'none'"));
    assert!(page.contains("form-action 'none'"));
    assert!(page.contains("background:#131314") && page.contains("color:#e3e3e3") && page.contains("color:#8ab4f8"));
    assert!(page.contains("<div id=\"working-mail\"><p>Hi Jordan</p></div>"));
  }

  #[test]
  fn the_other_page_is_light_under_a_dark_theme_and_dark_under_a_light_one() {
    let dark = Colors {
      background: 0x131314,
      text: 0xe3e3e3,
      link: 0x8ab4f8,
    };
    let light = Colors {
      background: 0xf8fafd,
      text: 0x1f1f1f,
      link: 0x0b57d0,
    };
    assert_eq!(dark.other(), LIGHT);
    assert_eq!(light.other(), DARK);
    // Flipping twice does not come back to the theme: the control does, by dropping the flip.
    assert_eq!(dark.other().other(), DARK);
  }

  #[test]
  fn remote_images_go_through_the_app() {
    let html = "<p>Beans</p><img alt=\"x\" src=\"https://images.example.com/a.png?w=1&amp;h=2\">\
                <td background='http://images.example.com/bg.gif'><img src=data:image/png;base64,AA== >";
    let routed = route_images(html);
    assert!(routed.contains("src=\"/image?u=https%3A%2F%2Fimages.example.com%2Fa.png%3Fw%3D1%26h%3D2\""));
    assert!(routed.contains("background='/image?u=http%3A%2F%2Fimages.example.com%2Fbg.gif'"));
    assert!(routed.contains("src=data:image/png;base64,AA=="));
    assert!(routed.starts_with("<p>Beans</p><img alt=\"x\" "));
  }

  #[test]
  fn text_that_reads_like_an_attribute_is_left_alone() {
    let html = "<p>Set src=https://example.com in the file</p><img data-src=\"https://images.example.com/a.png\">";
    assert_eq!(route_images(html), html);
  }

  #[test]
  fn an_unquoted_value_ends_at_the_tag() {
    let routed = route_images("<IMG SRC=https://images.example.com/a.png>");
    assert_eq!(routed, "<IMG SRC=/image?u=https%3A%2F%2Fimages.example.com%2Fa.png>");
  }

  #[test]
  fn an_image_request_names_its_remote_address() {
    let routed = route_images("<img src=\"https://images.example.com/a b.png?x=1&amp;y=2\">");
    let query = routed.split("?u=").nth(1).and_then(|rest| rest.split('"').next()).unwrap();
    let url = image_request("/image", Some(&format!("u={query}"))).unwrap();
    assert_eq!(url, "https://images.example.com/a b.png?x=1&y=2");
    assert_eq!(image_request("/image", Some("u=file%3A%2F%2F%2Fetc%2Fpasswd")), None);
    assert_eq!(image_request("/image", Some("u=%ZZ")), None);
    assert_eq!(image_request("/other", Some("u=https%3A%2F%2Fexample.com")), None);
    assert_eq!(image_request("/image", None), None);
  }

  #[test]
  fn the_demo_photos_go_through_the_app_and_nothing_that_reads_the_disk_does() {
    let routed = route_images("<p><img src=\"demo:image/coffee-beans.jpg\" alt=\"Coffee beans\"></p>");
    assert_eq!(
      routed,
      "<p><img src=\"/image?u=demo%3Aimage%2Fcoffee-beans.jpg\" alt=\"Coffee beans\"></p>"
    );
    let query = routed.split("?u=").nth(1).and_then(|rest| rest.split('"').next()).unwrap();
    let url = image_request("/image", Some(&format!("u={query}")));
    assert_eq!(url.as_deref(), Some("demo:image/coffee-beans.jpg"));
    for kept in [
      "<img src=\"file:///etc/passwd\">",
      "<img src=\"demo:file/notes.txt\">",
      "<img src=\"demo:upload//tmp/x.png\">",
      "<img src=\"javascript:alert(1)\">",
    ] {
      assert_eq!(route_images(kept), kept);
    }
    assert_eq!(image_request("/image", Some("u=demo%3Afile%2Fnotes.txt")), None);
    assert_eq!(image_request("/image", Some("u=demo%3Aupload%2F%2Ftmp%2Fx.png")), None);
    assert_eq!(image_request("/image", Some("u=javascript%3Aalert(1)")), None);
  }

  #[test]
  fn images_are_typed_by_their_first_bytes() {
    assert_eq!(image_type(&[0x89, b'P', b'N', b'G', 0x0d]), "image/png");
    assert_eq!(image_type(&[0xff, 0xd8, 0xff, 0xe0]), "image/jpeg");
    assert_eq!(image_type(b"GIF89a"), "image/gif");
    assert_eq!(image_type(b"RIFF\0\0\0\0WEBPVP8 "), "image/webp");
    assert_eq!(image_type(b"  <svg xmlns=\"http://www.w3.org/2000/svg\"/>"), "image/svg+xml");
    assert_eq!(image_type(b"???"), "application/octet-stream");
  }

  #[test]
  fn only_web_and_mail_links_leave_for_the_browser() {
    assert!(opens_in_browser("https://fieldnotes.example.org/112"));
    assert!(opens_in_browser("HTTP://example.com"));
    assert!(opens_in_browser("mailto:jpark@example.com"));
    assert!(!opens_in_browser("javascript:alert(1)"));
    assert!(!opens_in_browser("file:///etc/passwd"));
  }

  #[test]
  fn the_script_reports_parse() {
    assert_eq!(Report::parse("height|412"), Some(Report::Height(412.)));
    assert_eq!(Report::parse("height|1e9"), Some(Report::Height(MAX_HEIGHT)));
    assert_eq!(Report::parse("scroll|-48.5"), Some(Report::Scroll(-48.5)));
    assert_eq!(Report::parse("close"), Some(Report::Close));
    assert_eq!(Report::parse("height|NaN"), None);
    assert_eq!(Report::parse("width|10"), None);
    assert_eq!(Report::parse("anything"), None);
  }
}
