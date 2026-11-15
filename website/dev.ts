import { watch } from "node:fs";
import { join } from "node:path";

const port = Number(process.env.PORT ?? 3000);
const out = join(import.meta.dir, "dist");
const env = { ...process.env, SITE_URL: `http://localhost:${port}/` };

// Each build is a new process, so edited modules are read again.
function build(): void {
  Bun.spawnSync(["bun", "build.ts"], {
    cwd: import.meta.dir,
    env,
    stdio: ["ignore", "inherit", "inherit"],
  });
}

build();

let pending: Timer | undefined;
watch(join(import.meta.dir, "src"), { recursive: true }, () => {
  clearTimeout(pending);
  pending = setTimeout(build, 100);
});

Bun.serve({
  port,
  async fetch(request) {
    const { pathname } = new URL(request.url);
    const page = Bun.file(join(out, pathname.endsWith("/") ? `${pathname}index.html` : pathname));
    if (await page.exists()) return new Response(page);
    return new Response(Bun.file(join(out, "404.html")), { status: 404 });
  },
});

console.log(`http://localhost:${port}/`);
