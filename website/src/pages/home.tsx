import type { VNode } from "preact";
import { Code, Screenshot } from "../components";
import { shots } from "../images";
import type { Page } from "../layout";
import { href, site } from "../site";

const half = "(min-width: 60rem) 34rem, 100vw";

const tour = [
  {
    shot: shots.messages,
    alt: "Working in the Messages layout: every chat in one list with previews, beside a conversation with Jordan Park",
    title: "Messages.",
    text: "Every chat in one list with previews, beside the conversation. Or keep the Google Chat sidebar you already know.",
  },
  {
    shot: shots.rail,
    alt: "Working in the Rail layout: tabs with unread counts, the chat list, and a bot conversation with build and approval cards",
    title: "Rail.",
    text: "People, Groups, Spaces, and Apps one click away, with unread counts on each.",
  },
  {
    shot: shots.calendar,
    alt: "The Calendar week view in Working, with an event card offering Join with Google Meet and a Going? reply",
    title: "Calendar.",
    text: "Google Calendar in the same window with ⌘2: day to year views, event cards with Join and going or not, and an editor for new events.",
  },
  {
    shot: shots.meet,
    alt: "The Meet view in Working: a day's meetings beside the field that joins with a code or nickname",
    title: "Meet.",
    text: "⌘3 shows the day's meetings, a field to join with a code or a link, and new meetings in a click.",
  },
  {
    shot: shots.filters,
    alt: "The People filter chip turned on and the sort menu open above the chat list in Working",
    title: "Find people fast.",
    text: "Filter to people, groups, spaces, apps, or unread, then sort by recent or A to Z.",
  },
  {
    shot: shots.themes,
    alt: "The theme picker in Working previewing Ayu Dark, with Working Dark marked as the current theme",
    title: "28 themes.",
    text: "Ayu, Catppuccin, Dracula, Tokyo Night, and more, previewed live with ⌘⇧T.",
  },
];

const features = [
  [
    "Every conversation in one place",
    "DMs, groups, spaces, and apps, with Mentions and Starred views across all of them.",
  ],
  [
    "Three layouts",
    "Google Chat's sidebar, a Messages-style list with previews, or a Rail of categories.",
  ],
  ["Fast", "A disk cache paints the last state on launch, and ⌘K jumps to any conversation."],
  [
    "Full conversations",
    "Formatted text, threads, quotes, edits, reactions, cards, attachments, and read receipts.",
  ],
  [
    "Format as you write",
    "Bold, italic, strikethrough, lists, and code, from a bar or from the keyboard.",
  ],
  ["Calendar and Meet in the same window", "⌘2 opens Google Calendar, ⌘3 opens Google Meet."],
  [
    "Calls in their own window",
    "Camera, microphone, and screen sharing. On Linux, meetings open in your browser.",
  ],
  ["Media viewer", "Images, video, audio, PDF, and text files open inside the app."],
  [
    "Notifications and presence",
    "System notifications, an unread badge, do-not-disturb schedules, and a custom status.",
  ],
  [
    "Rewrite a draft",
    "⌘⇧R rewrites your message through the Claude Code or Codex CLI you already have.",
  ],
  ["28 themes and your own fonts", "One theme for light, one for dark."],
  [
    "Local MCP server",
    "Optional: AI agents on your computer can read and search your chats, and send if you allow it.",
  ],
  ["Updates itself", "A new release installs from inside the app."],
] as const;

const questions = [
  [
    "Is there a Google Chat desktop app?",
    "Google Chat itself lives in a browser tab or a wrapped web page. Working is a native client for macOS, Linux, and Windows: it opens from a local cache, keeps up through Google's realtime stream, and uses your system's notifications and unread badge.",
  ],
  [
    "Is Working free?",
    "Yes. Working is open source under the MIT license, and the code is on GitHub.",
  ],
  [
    "Is Working made by Google?",
    "No. Working is an independent project. It is not affiliated with or endorsed by Google.",
  ],
  [
    "Does Working run on Linux?",
    "Yes. Every release has an AppImage for x86_64 and aarch64. It needs GTK 3, WebKitGTK 4.1, libsoup 3, and an X11 or XWayland session. Meet calls open in your browser.",
  ],
  [
    "Where does Working keep my session?",
    "On your computer. Settings, the Google session, and the message cache live in folders on your own disk.",
  ],
  [
    "Can AI agents read my Google Chat through Working?",
    "Only if you turn it on. Working has an optional MCP server that listens on your computer only and needs a token. Agents can list, search, and read conversations. Sending is a separate switch.",
  ],
] as const;

function Home(): VNode {
  return (
    <>
      <section class="hero">
        <h1>{site.tagline}</h1>
        <p class="lede">
          Working is a native Google Chat client for macOS, Linux, and Windows, written in Rust. It
          opens from a local cache before the network answers, and behaves like an app of your
          system: an unread badge, system notifications, keyboard first, light and dark.
        </p>
        <p class="actions">
          <a class="button" href="#install">
            Download Working
          </a>
          <a href={site.repo}>Source on GitHub</a>
        </p>
        <p class="fine">Free and open source. Version {site.version}.</p>
        <Screenshot
          shot={shots.googleChat}
          alt="Working, a native Google Chat desktop app, in the Google Chat layout: the sidebar with its sections beside the Product launch space, where a link preview of the new landing page has a reaction"
          sizes="(min-width: 72rem) 70rem, 100vw"
          eager
        />
      </section>

      <section class="band" aria-labelledby="layouts">
        <h2 id="layouts">Your chats, your way</h2>
        <p class="lede">
          Pick the layout that fits how you work, keep your calendar and meetings in the same
          window, and sort and filter every list the way you think about people.
        </p>
        <div class="tour">
          {tour.map((stop) => (
            <figure>
              <Screenshot shot={stop.shot} alt={stop.alt} sizes={half} />
              <figcaption>
                <strong>{stop.title}</strong> {stop.text}
              </figcaption>
            </figure>
          ))}
        </div>
      </section>

      <section class="band" aria-labelledby="features">
        <h2 id="features">What you get</h2>
        <dl class="features">
          {features.map(([name, text]) => (
            <div>
              <dt>{name}</dt>
              <dd>{text}</dd>
            </div>
          ))}
        </dl>
        <p class="fine">
          Shortcuts are written the macOS way. On Linux and Windows, Ctrl takes the place of Cmd.
        </p>
      </section>

      <section class="band" id="install" aria-labelledby="install-title">
        <h2 id="install-title">Install Working</h2>
        <p class="lede">
          Download a build for your system, then sign in with Google on the welcome screen.
        </p>
        <div class="systems">
          <section>
            <h3>macOS</h3>
            <p>macOS 13 or newer, on Apple silicon and Intel. Install with Homebrew:</p>
            <Code>{site.brew}</Code>
            <p>
              Or open the <code>.dmg</code> from the release.
            </p>
            <a href={href("/mac/")}>Google Chat for Mac</a>
          </section>
          <section>
            <h3>Linux</h3>
            <p>
              The <code>.AppImage</code> for x86_64 or aarch64. Make it executable, then run it:
            </p>
            <Code>chmod +x Working_*.AppImage</Code>
            <p>Needs GTK 3, WebKitGTK 4.1, libsoup 3, and X11 or XWayland.</p>
            <a href={href("/linux/")}>Google Chat for Linux</a>
          </section>
          <section>
            <h3>Windows</h3>
            <p>
              The <code>-setup.exe</code> for x64 or Arm64. It installs for your user only, with no
              administrator rights.
            </p>
            <p>
              SmartScreen warns because the installer is not code-signed: choose More info, then Run
              anyway.
            </p>
            <a href={href("/windows/")}>Google Chat for Windows</a>
          </section>
        </div>
        <p class="actions">
          <a class="button" href={site.releases}>
            Get the latest release
          </a>
          <a href={`${site.repo}#build-from-source`}>Build from source</a>
        </p>
      </section>

      <section class="band" aria-labelledby="questions">
        <h2 id="questions">Common questions</h2>
        <div class="thread">
          {questions.map(([question, answer]) => (
            <>
              <h3 class="bubble">{question}</h3>
              <p class="bubble mine">{answer}</p>
            </>
          ))}
        </div>
      </section>
    </>
  );
}

export const home: Page = {
  path: "/",
  title: "Working: Google Chat desktop app for Mac, Linux, and Windows",
  description:
    "Working is a free, open-source native Google Chat client. It opens from a local cache, notifies through your system, and runs on macOS, Linux, and Windows.",
  schema: [
    {
      "@type": "FAQPage",
      mainEntity: questions.map(([name, text]) => ({
        "@type": "Question",
        name,
        acceptedAnswer: { "@type": "Answer", text },
      })),
    },
  ],
  body: Home,
};
