import { rm } from "node:fs/promises";
import { join } from "node:path";
import { writeImages } from "./src/images";
import { renderPage } from "./src/layout";
import { pages } from "./src/pages";
import { llms, robots, sitemap } from "./src/discovery";

const out = join(import.meta.dir, "dist");
if (Bun.argv.includes("--clean")) await rm(out, { recursive: true, force: true });

const file = (path: string): string => (path.endsWith("/") ? `${path}index.html` : path);

await Promise.all([
  ...pages.map((page) => Bun.write(join(out, file(page.path)), renderPage(page))),
  Bun.write(join(out, "sitemap.xml"), sitemap(pages)),
  Bun.write(join(out, "robots.txt"), robots()),
  Bun.write(join(out, "llms.txt"), llms(pages)),
  writeImages(out),
]);

console.log(`Built ${pages.length} pages into ${out}`);
