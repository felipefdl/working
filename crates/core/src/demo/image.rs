//! Pictures the demo draws for its image attachments and link previews: a diagonal color gradient, encoded as
//! an uncompressed 24-bit BMP, which the app's decoder reads like any download.

/// A `width` × `height` gradient that starts at `hue` (degrees) and turns 60 degrees toward the far corner.
pub fn gradient(hue: u32, width: u32, height: u32) -> Vec<u8> {
  let (width, height) = (width.max(1), height.max(1));
  let row = (width * 3).next_multiple_of(4);
  let pixels = row * height;
  let mut bytes = Vec::with_capacity(usize::try_from(54 + pixels).unwrap_or_default());
  bytes.extend_from_slice(b"BM");
  bytes.extend_from_slice(&(54 + pixels).to_le_bytes());
  bytes.extend_from_slice(&0_u32.to_le_bytes());
  bytes.extend_from_slice(&54_u32.to_le_bytes());
  bytes.extend_from_slice(&40_u32.to_le_bytes());
  bytes.extend_from_slice(&width.to_le_bytes());
  bytes.extend_from_slice(&height.to_le_bytes());
  bytes.extend_from_slice(&1_u16.to_le_bytes());
  bytes.extend_from_slice(&24_u16.to_le_bytes());
  bytes.extend_from_slice(&[0; 4]);
  bytes.extend_from_slice(&pixels.to_le_bytes());
  bytes.extend_from_slice(&[0; 16]);
  let span = width + height;
  let padding = row - width * 3;
  // Rows run bottom to top.
  for y in (0..height).rev() {
    for x in 0..width {
      let t = (x + y) * 255 / span;
      let [red, green, blue] = hsv((hue + t * 60 / 255) % 360, 150, 235 - t * 90 / 255);
      bytes.extend_from_slice(&[blue, green, red]);
    }
    bytes.extend(std::iter::repeat_n(0, usize::try_from(padding).unwrap_or_default()));
  }
  bytes
}

/// RGB for a hue in degrees and saturation and value in 0..=255.
fn hsv(hue: u32, saturation: u32, value: u32) -> [u8; 3] {
  let channel = |level: u32| u8::try_from(level.min(255)).unwrap_or(u8::MAX);
  let sector = hue / 60;
  let within = (hue % 60) * 255 / 60;
  let low = value * (255 - saturation) / 255;
  let falling = value * (255 - saturation * within / 255) / 255;
  let rising = value * (255 - saturation * (255 - within) / 255) / 255;
  let (red, green, blue) = match sector {
    0 => (value, rising, low),
    1 => (falling, value, low),
    2 => (low, value, rising),
    3 => (low, falling, value),
    4 => (rising, low, value),
    _ => (value, low, falling),
  };
  [channel(red), channel(green), channel(blue)]
}

#[cfg(test)]
mod tests {
  use super::{gradient, hsv};

  #[test]
  fn bmp_size_matches_its_header() {
    let bytes = gradient(200, 33, 7);
    let declared = u32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
    assert_eq!(usize::try_from(declared).unwrap(), bytes.len());
    // 33 pixels of 3 bytes pad to 100 per row.
    assert_eq!(bytes.len(), 54 + 100 * 7);
  }

  #[test]
  fn primary_hues() {
    assert_eq!(hsv(0, 255, 255), [255, 0, 0]);
    assert_eq!(hsv(120, 255, 255), [0, 255, 0]);
    assert_eq!(hsv(240, 255, 255), [0, 0, 255]);
  }
}
