# Website

The public site for Working: static HTML rendered at build time with Preact, on Bun alone. Gate: `bun run verify` (format, types, tests, build).

`.github/workflows/website.yml` runs that gate and publishes `dist/` to GitHub Pages on every push to `main` that touches the site, the screenshots, the logo, or `Cargo.toml`. CI uses the Bun version in `packageManager`.

- Bun's built-ins do the work: `bun fmt`, `bun check`, `bun test`, `Bun.build` for the CSS, `Bun.Image` for the screenshots, `HTMLRewriter` in the tests. A new npm dependency needs a reason a built-in cannot meet.
- The social card (`/og.jpg`) is `src/card.jpg`, drawn by `bun run card` with `Bun.WebView` and committed. Run it again when the tagline, the logo, or the Google Chat screenshot changes.
- Pages ship no client JavaScript and no web fonts. The stylesheet is inlined into every page.
- Every fact on a page comes from `README.md`, `docs/platforms.md`, or the code. When the app changes, change the page that says it. The version comes from the root `Cargo.toml`.
- Screenshots come from `docs/screenshots` at build time; the site keeps no copies.
- A new page goes into `src/pages/index.ts`. Its `title` (60 characters at most) and `description` (110 to 160) are what a search result shows: write them for the words people search, such as "Google Chat for Linux". `src/seo.test.ts` checks every page in that list.
- Internal links go through `href()` and absolute ones through `absolute()` in `src/site.ts`, so the site works under a path such as `/working/`. `SITE_URL` sets where the site is served; `bun run dev` sets it to localhost.
