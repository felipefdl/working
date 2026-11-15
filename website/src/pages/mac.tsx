import type { VNode } from "preact";
import { Code, Download, Files, Screenshot } from "../components";
import { shots } from "../images";
import { Article, type Page } from "../layout";
import { site } from "../site";

function Mac(): VNode {
  return (
    <Article
      crumb="Mac"
      title="Google Chat for Mac"
      lede="Working is a native Google Chat app for macOS: a Dock badge for unread messages, system notifications, the menu bar and shortcuts of a Mac app, and Meet calls in their own window."
    >
      <Download>Download for macOS</Download>
      <Screenshot
        shot={shots.rail}
        alt="Working on macOS in the Rail layout: tabs with unread counts, the chat list, and a bot conversation with build and approval cards"
        sizes="(min-width: 48rem) 46rem, 100vw"
        eager
      />

      <section>
        <h2>Install</h2>
        <p>With Homebrew:</p>
        <Code>{site.brew}</Code>
        <p>
          Or download the <code>.dmg</code> from the <a href={site.releases}>latest release</a> and
          open it. Then sign in with Google on the welcome screen.
        </p>
      </section>

      <section>
        <h2>Requirements</h2>
        <p>
          macOS 13 or newer, on Apple silicon or Intel. The download is one universal app, signed
          and notarized.
        </p>
      </section>

      <section>
        <h2>What the Mac app does</h2>
        <ul>
          <li>Shows unread messages on the Dock icon and through system notifications.</li>
          <li>Jumps to any conversation with ⌘K, and previews themes with ⌘⇧T.</li>
          <li>Opens Google Calendar with ⌘2 and Google Meet with ⌘3, in the same window.</li>
          <li>Runs calls in their own window, with camera, microphone, and screen sharing.</li>
          <li>Follows the system between a light theme and a dark one.</li>
          <li>Updates itself.</li>
        </ul>
      </section>

      <Files settings="~/.config/working" cache="~/Library/Caches/working" />
    </Article>
  );
}

export const mac: Page = {
  path: "/mac/",
  crumb: "Mac",
  title: "Google Chat for Mac: a native macOS app | Working",
  description:
    "Install Working, a native Google Chat app for macOS 13 or newer on Apple silicon and Intel. Homebrew or dmg, with a Dock badge, notifications, and Meet calls.",
  body: Mac,
};
