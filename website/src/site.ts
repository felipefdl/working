import cargo from "../../Cargo.toml";

// Where the site is served. A path in the URL (GitHub Pages serves a project under /<repo>/) becomes the prefix of
// every internal link.
const origin = new URL(process.env.SITE_URL ?? "https://felipefdl.github.io/working/");
const base = origin.pathname.replace(/\/$/, "");

export const site = {
  name: "Working",
  tagline: "Google Chat as a real desktop app",
  version: cargo.workspace.package.version as string,
  repo: "https://github.com/felipefdl/working",
  releases: "https://github.com/felipefdl/working/releases/latest",
  license: "https://github.com/felipefdl/working/blob/main/LICENSE.md",
  brew: "brew install --cask felipefdl/tap/working",
};

/** A link to a page or file of this site: `path` starts with `/`. */
export function href(path: string): string {
  return base + path;
}

/** The same link with the site's origin, for canonical URLs, the sitemap, and social cards. */
export function absolute(path: string): string {
  return origin.origin + base + path;
}
