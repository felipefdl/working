import { describe, expect, test } from "bun:test";
import { llms, sitemap } from "./discovery";
import { renderPage } from "./layout";
import { pages } from "./pages";
import { absolute, href } from "./site";

type Facts = {
  lang: string | null;
  title: string;
  h1: number;
  canonical: string[];
  meta: Map<string, string>;
  images: { alt: string | null; width: string | null; height: string | null }[];
  links: string[];
  schema: string[];
};

async function read(html: string): Promise<Facts> {
  const facts: Facts = {
    lang: null,
    title: "",
    h1: 0,
    canonical: [],
    meta: new Map(),
    images: [],
    links: [],
    schema: [],
  };
  let block = "";
  const rewriter = new HTMLRewriter()
    .on("html", { element: (el) => void (facts.lang = el.getAttribute("lang")) })
    .on("title", { text: (chunk) => void (facts.title += chunk.text) })
    .on("h1", { element: () => void facts.h1++ })
    .on('link[rel="canonical"]', {
      element: (el) => void facts.canonical.push(el.getAttribute("href") ?? ""),
    })
    .on("meta", {
      element: (el) => {
        const key = el.getAttribute("name") ?? el.getAttribute("property");
        if (key) facts.meta.set(key, el.getAttribute("content") ?? "");
      },
    })
    .on("img", {
      element: (el) =>
        void facts.images.push({
          alt: el.getAttribute("alt"),
          width: el.getAttribute("width"),
          height: el.getAttribute("height"),
        }),
    })
    .on("a[href]", { element: (el) => void facts.links.push(el.getAttribute("href") ?? "") })
    .on('script[type="application/ld+json"]', {
      text: (chunk) => {
        block += chunk.text;
        if (chunk.lastInTextNode) {
          facts.schema.push(block);
          block = "";
        }
      },
    });
  await rewriter.transform(new Response(html)).text();
  return facts;
}

const rendered = await Promise.all(
  pages.map(async (page) => ({ page, facts: await read(renderPage(page)) })),
);
const paths = new Set(pages.map((page) => href(page.path)));

describe.each(rendered)("$page.path", ({ page, facts }) => {
  test("has a title and a description sized for a search result", () => {
    expect(facts.title).toBe(page.title);
    expect(facts.title.length).toBeLessThanOrEqual(60);
    const description = facts.meta.get("description") ?? "";
    expect(description.length).toBeGreaterThanOrEqual(110);
    expect(description.length).toBeLessThanOrEqual(160);
  });

  test("names its language and has one h1", () => {
    expect(facts.lang).toBe("en");
    expect(facts.h1).toBe(1);
  });

  test("is canonical, or kept out of the index", () => {
    if (page.hidden) {
      expect(facts.canonical).toEqual([]);
      expect(facts.meta.get("robots")).toBe("noindex");
    } else {
      expect(facts.canonical).toEqual([absolute(page.path)]);
      expect(facts.meta.has("robots")).toBe(false);
    }
  });

  test("has a social card with absolute URLs", () => {
    expect(facts.meta.get("og:title")).toBe(page.title);
    expect(facts.meta.get("og:url")).toBe(absolute(page.path));
    expect(facts.meta.get("og:image")).toBe(absolute("/og.jpg"));
    expect(facts.meta.get("twitter:card")).toBe("summary_large_image");
  });

  test("describes and sizes every image", () => {
    for (const image of facts.images) {
      expect(image.alt?.length).toBeGreaterThan(20);
      expect(Number(image.width)).toBeGreaterThan(0);
      expect(Number(image.height)).toBeGreaterThan(0);
    }
  });

  test("links only to pages that exist", () => {
    const internal = facts.links.filter((link) => link.startsWith("/") || link.startsWith("#"));
    for (const link of internal) {
      const [path, anchor] = link.split("#");
      if (path) expect(paths).toContain(path);
      if (anchor && (!path || path === href("/"))) {
        const target = path ? pages.find((other) => other.path === "/") : page;
        expect(renderPage(target ?? page)).toContain(`id="${anchor}"`);
      }
    }
  });

  test("carries schema.org data that parses", () => {
    expect(facts.schema).toHaveLength(1);
    const graph = JSON.parse(facts.schema[0] ?? "")["@graph"] as { "@type": string }[];
    const types = graph.map((item) => item["@type"]);
    expect(types).toContain("SoftwareApplication");
    expect(types.includes("BreadcrumbList")).toBe(page.crumb !== undefined);
  });
});

test("titles and descriptions are unique", () => {
  expect(new Set(pages.map((page) => page.title)).size).toBe(pages.length);
  expect(new Set(pages.map((page) => page.description)).size).toBe(pages.length);
});

test("the sitemap and llms.txt list every indexed page and no hidden one", () => {
  for (const page of pages) {
    expect(sitemap(pages).includes(`<loc>${absolute(page.path)}</loc>`)).toBe(!page.hidden);
    expect(llms(pages).includes(`(${absolute(page.path)})`)).toBe(!page.hidden);
  }
});
