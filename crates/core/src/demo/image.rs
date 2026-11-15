//! Photos the demo shows as image attachments and link-preview pictures: JPEGs in `photos/`, embedded in the
//! binary and served from memory, so the demo stays offline. `photos/README.md` says where each one came from.

/// A photo the demo serves at `demo:image/<file>`, with the pixel size its attachments declare.
pub struct Photo {
  pub file: &'static str,
  pub width: u32,
  pub height: u32,
  pub bytes: &'static [u8],
}

/// A bowl of ramen, for the lunch plan in Riley's DM.
pub const RAMEN: Photo = Photo {
  file: "ramen-bowl.jpg",
  width: 1152,
  height: 864,
  bytes: include_bytes!("photos/ramen-bowl.jpg"),
};

/// A bright desk with a light dashboard on a laptop: the launch's light hero and its landing page's preview.
pub const HERO_LIGHT: Photo = Photo {
  file: "launch-hero-light.jpg",
  width: 1248,
  height: 832,
  bytes: include_bytes!("photos/launch-hero-light.jpg"),
};

/// The same kind of desk at night with a dark dashboard: the launch's dark hero.
pub const HERO_DARK: Photo = Photo {
  file: "launch-hero-dark.jpg",
  width: 1248,
  height: 832,
  bytes: include_bytes!("photos/launch-hero-dark.jpg"),
};

/// Coffee beans on the office kitchen counter, for the coffee chat.
pub const BEANS: Photo = Photo {
  file: "coffee-beans.jpg",
  width: 1152,
  height: 864,
  bytes: include_bytes!("photos/coffee-beans.jpg"),
};

/// Every photo the demo serves.
pub const PHOTOS: [&Photo; 4] = [&RAMEN, &HERO_LIGHT, &HERO_DARK, &BEANS];

impl Photo {
  /// The URL its attachments carry and `fetch` answers.
  pub fn url(&self) -> String {
    format!("demo:image/{}", self.file)
  }
}

/// The photo a `demo:image/` URL names.
pub fn photo(url: &str) -> Option<&'static Photo> {
  let file = url.strip_prefix("demo:image/")?;
  PHOTOS.into_iter().find(|photo| photo.file == file)
}
