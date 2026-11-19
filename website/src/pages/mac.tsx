import type { VNode } from "preact";
import { Code, Download, Files, Screenshot } from "../components";
import { shots } from "../images";
import { Article, type Page } from "../layout";
import { site } from "../site";

function Mac(): VNode {
  return (
    <Article
      crumb="Mac"
      title="Google Chat and Gmail for Mac"
      lede="Working is a native macOS app for Google Chat, Gmail, Google Calendar, and Meet in one window: a Dock badge for unread chats and mail, system notifications, the menu bar and shortcuts of a Mac app, and Meet calls in their own window."
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
          <li>
            Opens Home, Calendar, Meet, and Mail with ⌘1 to ⌘4, and jumps to any conversation with
            ⌘K.
          </li>
          <li>
            Reads and writes Gmail in the same window: the Inbox, Starred, Sent, All mail, your
            labels, and Gmail search, with new emails, replies, archive, and mark as unread.
          </li>
          <li>Counts unread chats and unread inbox threads on the Dock icon.</li>
          <li>Posts to Notification Center for new messages, new mail, and event reminders.</li>
          <li>
            Runs calls in their own window, with camera, microphone, the call's chat, and screen
            sharing through the macOS picker.
          </li>
          <li>
            Follows the system between a light theme and a dark one, and previews themes with ⌘⇧T.
          </li>
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
  title: "Google Chat and Gmail for Mac: a native macOS app | Working",
  description:
    "Working is a native Mac app for Google Chat, Gmail, Calendar, and Meet on macOS 13 or newer. Homebrew or dmg, with a Dock badge, notifications, and Meet calls.",
  body: Mac,
};
