import type { Page } from "./layout";
import { absolute, site } from "./site";

const listed = (pages: Page[]): Page[] => pages.filter((page) => !page.hidden);

/** Every page a search engine should index. */
export function sitemap(pages: Page[]): string {
  const urls = listed(pages).map((page) => `  <url><loc>${absolute(page.path)}</loc></url>`);
  return [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">',
    ...urls,
    "</urlset>",
    "",
  ].join("\n");
}

export function robots(): string {
  return `User-agent: *\nAllow: /\n\nSitemap: ${absolute("/sitemap.xml")}\n`;
}

/** The site as a short Markdown index, for AI assistants that look for /llms.txt. */
export function llms(pages: Page[]): string {
  const links = listed(pages).map(
    (page) => `- [${page.title}](${absolute(page.path)}): ${page.description}`,
  );
  return [
    `# ${site.name}`,
    "",
    `> ${site.tagline}. A free, open-source native Google Chat client for macOS, Linux, and Windows, written in Rust. Not affiliated with or endorsed by Google.`,
    "",
    "## Pages",
    "",
    ...links,
    "",
    "## Source",
    "",
    `- [Repository](${site.repo}): code, issues, and build instructions`,
    `- [Latest release](${site.releases}): downloads for every system`,
    "",
  ].join("\n");
}
