import { join } from "node:path";
import { stat } from "node:fs/promises";
import { href } from "./site";

const root = join(import.meta.dir, "../..");
const widths = [800, 1200, 1600];

export type Shot = {
  slug: string;
  source: string;
  width: number;
  height: number;
  widths: number[];
};

// The site reuses the README's screenshots. The slug is the file name search engines see.
async function shot(file: string, slug: string): Promise<Shot> {
  const source = join(root, "docs/screenshots", file);
  const { width, height } = await new Bun.Image(source).metadata();
  return { slug, source, width, height, widths: [...widths.filter((w) => w < width), width] };
}

export const shots = {
  googleChat: await shot("google-chat.png", "google-chat-desktop-app-sidebar-layout"),
  messages: await shot("messages.png", "google-chat-desktop-app-messages-layout"),
  rail: await shot("rail.png", "google-chat-desktop-app-rail-layout"),
  calendar: await shot("calendar.png", "google-calendar-desktop-app-week-view"),
  meet: await shot("meet.png", "google-meet-desktop-app-meetings"),
  mail: await shot("mail.png", "gmail-desktop-app-reading-pane"),
  mailLabels: await shot("mail-labels.png", "gmail-desktop-app-labels"),
  themes: await shot("themes.png", "google-chat-desktop-app-themes"),
  settings: await shot("settings.png", "google-chat-desktop-app-settings"),
  windows: await shot("windows.png", "google-chat-windows-app"),
  linux: await shot("linux.png", "google-chat-linux-app"),
};

export function shotUrl(shot: Shot, width: number): string {
  return href(`/img/${shot.slug}-${width}.webp`);
}

const mark = (await Bun.file(join(root, "logo/logo.svg")).text())
  .replace(/<!--[\s\S]*?-->\s*/, "")
  .replace(' width="900" height="900"', "");

/** The two-bubble mark, drawn in the text color of where it sits. */
export const logo = mark.replace("<svg ", '<svg class="mark" aria-hidden="true" ');

const favicon = mark.replace(
  /(<svg [^>]*>)/,
  "$1<style>svg{color:#101318}@media(prefers-color-scheme:dark){svg{color:#fff}}</style>",
);

async function fresh(target: string, source: string): Promise<boolean> {
  const [made, from] = await Promise.all([stat(target).catch(() => null), stat(source)]);
  return made !== null && made.mtimeMs >= from.mtimeMs;
}

/** Writes every image the pages point at. An image newer than its source is left alone. */
export async function writeImages(out: string): Promise<void> {
  // The social card is drawn by card.ts and committed, so it is only copied.
  const jobs: Promise<unknown>[] = [
    Bun.write(join(out, "favicon.svg"), favicon),
    Bun.write(join(out, "og.jpg"), Bun.file(join(import.meta.dir, "card.jpg"))),
  ];
  const encode = async (target: string, source: string, run: () => Bun.Image): Promise<void> => {
    if (!(await fresh(target, source))) await run().write(target);
  };
  for (const s of Object.values(shots)) {
    for (const width of s.widths) {
      const image = (): Bun.Image => new Bun.Image(s.source).resize(width).webp({ quality: 84 });
      jobs.push(encode(join(out, "img", `${s.slug}-${width}.webp`), s.source, image));
    }
  }
  const icon = join(root, "logo/icon-light.png");
  jobs.push(
    encode(join(out, "apple-touch-icon.png"), icon, () => new Bun.Image(icon).resize(180).png()),
  );
  await Promise.all(jobs);
}
