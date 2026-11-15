// Draws the picture a shared link shows (src/card.jpg, published as /og.jpg): `bun run card`.
// The result is committed rather than built in CI, because it is set in this Mac's system typeface.
import { join } from "node:path";
import { logo, shots } from "./src/images";
import { site } from "./src/site";

const width = 1200;
const height = 630;
const scale = 2;

const html = `<!doctype html>
<meta charset="utf-8">
<style>
  * { box-sizing: border-box; margin: 0; }
  html { zoom: ${scale}; }
  body {
    position: relative;
    width: ${width}px;
    height: ${height}px;
    overflow: hidden;
    background: #d9ebfc;
    color: #101318;
    font-family: system-ui, sans-serif;
    -webkit-font-smoothing: antialiased;
  }
  .brand {
    position: absolute;
    left: 72px;
    top: 64px;
    display: flex;
    align-items: center;
    gap: 14px;
    font-size: 34px;
    font-weight: 700;
    letter-spacing: -0.02em;
  }
  .mark { width: 52px; height: 52px; }
  h1 {
    position: absolute;
    left: 72px;
    top: 196px;
    width: 500px;
    font-size: 86px;
    font-weight: 700;
    letter-spacing: -0.04em;
    line-height: 0.98;
  }
  p {
    position: absolute;
    left: 72px;
    bottom: 64px;
    width: 470px;
    color: #3a4a60;
    font-size: 27px;
    line-height: 1.3;
  }
  img {
    position: absolute;
    left: 600px;
    top: 84px;
    width: 1080px;
    filter: drop-shadow(0 30px 50px rgb(16 19 24 / 0.35));
  }
</style>
<div class="brand">${logo}${site.name}</div>
<h1>Google Chat as a real desktop app</h1>
<p>Native on macOS, Linux, and Windows. Free and open source.</p>
<img src="/shot.png" alt="">`;

const server = Bun.serve({
  port: 0,
  routes: {
    "/": new Response(html, { headers: { "content-type": "text/html" } }),
    "/shot.png": new Response(Bun.file(shots.googleChat.source)),
  },
});

{
  await using view = new Bun.WebView({ width: width * scale, height: height * scale });
  await view.navigate(server.url.href);
  await view.evaluate("document.images[0].decode()");
  const capture = await view.screenshot({ encoding: "buffer" });
  const target = join(import.meta.dir, "src/card.jpg");
  await new Bun.Image(capture).resize(width, height).jpeg({ quality: 90 }).write(target);
  console.log(`Wrote ${target}`);
}

await server.stop();
