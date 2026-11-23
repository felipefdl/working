import type { ComponentChildren, VNode } from "preact";
import { renderToString } from "preact-render-to-string";
import { logo, shots } from "./images";
import { absolute, href, site } from "./site";
import { css } from "./styles";

export type Page = {
  /** Where the page is served: `/`, `/linux/`, or a file such as `/404.html`. */
  path: string;
  /** The search result's headline: at most 60 characters. */
  title: string;
  /** The search result's text: 110 to 160 characters. */
  description: string;
  /** The page's name in breadcrumbs. The home page has none. */
  crumb?: string;
  /** Left out of search results and the sitemap. */
  hidden?: boolean;
  /** Extra schema.org objects for this page. */
  schema?: object[];
  body: () => VNode;
};

const nav = [
  { path: "/mac/", label: "Mac" },
  { path: "/linux/", label: "Linux" },
  { path: "/windows/", label: "Windows" },
  { path: "/mcp/", label: "MCP server" },
];

const app = {
  "@type": "SoftwareApplication",
  "@id": absolute("/#app"),
  name: site.name,
  description:
    "A native desktop app for Google Chat, Gmail, Google Calendar, and Google Meet on macOS, Linux, and Windows, written in Rust. Free and open source.",
  url: absolute("/"),
  applicationCategory: "CommunicationApplication",
  operatingSystem: "macOS 13 or newer, Linux, Windows",
  softwareVersion: site.version,
  downloadUrl: site.releases,
  license: site.license,
  isAccessibleForFree: true,
  offers: { "@type": "Offer", price: "0", priceCurrency: "USD" },
  image: absolute("/og.jpg"),
  screenshot: absolute(`/img/${shots.googleChat.slug}-${shots.googleChat.width}.webp`),
  sameAs: [site.repo],
};

function schema(page: Page): object {
  const graph: object[] = [
    app,
    { "@type": "WebSite", "@id": absolute("/#site"), name: site.name, url: absolute("/") },
    ...(page.schema ?? []),
  ];
  if (page.crumb) {
    graph.push({
      "@type": "BreadcrumbList",
      itemListElement: [
        { "@type": "ListItem", position: 1, name: site.name, item: absolute("/") },
        { "@type": "ListItem", position: 2, name: page.crumb, item: absolute(page.path) },
      ],
    });
  }
  return { "@context": "https://schema.org", "@graph": graph };
}

// `<` is escaped so page text can never close the script element.
function Json({ value }: { value: object }): VNode {
  const json = JSON.stringify(value).replaceAll("<", "\\u003c");
  return <script type="application/ld+json" dangerouslySetInnerHTML={{ __html: json }} />;
}

function Head({ page }: { page: Page }): VNode {
  const url = absolute(page.path);
  const image = absolute("/og.jpg");
  return (
    <head>
      <meta charset="utf-8" />
      <meta name="viewport" content="width=device-width, initial-scale=1" />
      <title>{page.title}</title>
      <meta name="description" content={page.description} />
      {page.hidden ? <meta name="robots" content="noindex" /> : <link rel="canonical" href={url} />}
      <meta name="color-scheme" content="light dark" />
      <meta name="theme-color" content="#ffffff" media="(prefers-color-scheme: light)" />
      <meta name="theme-color" content="#0d0f14" media="(prefers-color-scheme: dark)" />
      <link rel="icon" href={href("/favicon.svg")} type="image/svg+xml" />
      <link rel="apple-touch-icon" href={href("/apple-touch-icon.png")} />
      <meta property="og:type" content="website" />
      <meta property="og:site_name" content={site.name} />
      <meta property="og:title" content={page.title} />
      <meta property="og:description" content={page.description} />
      <meta property="og:url" content={url} />
      <meta property="og:image" content={image} />
      <meta property="og:image:type" content="image/jpeg" />
      <meta property="og:image:width" content="1200" />
      <meta property="og:image:height" content="630" />
      <meta
        property="og:image:alt"
        content={`Working: ${site.tagline}, beside the app's window with the Google Chat sidebar and a space showing a link preview`}
      />
      <meta name="twitter:card" content="summary_large_image" />
      <meta name="twitter:title" content={page.title} />
      <meta name="twitter:description" content={page.description} />
      <meta name="twitter:image" content={image} />
      <Json value={schema(page)} />
      <style dangerouslySetInnerHTML={{ __html: css }} />
    </head>
  );
}

function Header({ page }: { page: Page }): VNode {
  return (
    <header class="top">
      <a class="brand" href={href("/")} aria-label="Working home">
        <span dangerouslySetInnerHTML={{ __html: logo }} />
        Working
      </a>
      <nav aria-label="Main">
        {nav.map((item) => (
          <a href={href(item.path)} aria-current={item.path === page.path ? "page" : undefined}>
            {item.label}
          </a>
        ))}
        <a href={site.repo}>GitHub</a>
      </nav>
      <a class="button small" href={href("/#install")}>
        Download
      </a>
    </header>
  );
}

function Footer(): VNode {
  return (
    <footer class="bottom">
      <nav aria-label="Footer">
        <a href={href("/")}>Working</a>
        <a href={href("/mac/")}>Google Chat and Gmail for Mac</a>
        <a href={href("/linux/")}>Google Chat and Gmail for Linux</a>
        <a href={href("/windows/")}>Google Chat and Gmail for Windows</a>
        <a href={href("/mcp/")}>MCP server for AI agents</a>
        <a href={site.repo}>Source on GitHub</a>
        <a href={`${site.repo}/issues`}>Report a problem</a>
      </nav>
      <p>
        Working is an independent project. It is not affiliated with or endorsed by Google. Google
        Chat, Gmail, Google Calendar, and Google Meet are trademarks of Google LLC.
      </p>
      <p>
        Free and open source under the <a href={site.license}>MIT license</a>.
      </p>
    </footer>
  );
}

function Document({ page }: { page: Page }): VNode {
  return (
    <html lang="en">
      <Head page={page} />
      <body>
        <a class="skip" href="#main">
          Skip to content
        </a>
        <Header page={page} />
        <main id="main">{page.body()}</main>
        <Footer />
      </body>
    </html>
  );
}

/** A page of prose under the home page: breadcrumb, title, and lede above its sections. */
export function Article(props: {
  crumb: string;
  title: string;
  lede: ComponentChildren;
  children: ComponentChildren;
}): VNode {
  return (
    <article class="doc">
      <nav class="crumbs" aria-label="Breadcrumb">
        <a href={href("/")}>Working</a>
        <span aria-hidden="true">/</span>
        <span aria-current="page">{props.crumb}</span>
      </nav>
      <h1>{props.title}</h1>
      <p class="lede">{props.lede}</p>
      {props.children}
    </article>
  );
}

export function renderPage(page: Page): string {
  return `<!doctype html>${renderToString(<Document page={page} />)}`;
}
