import type { VNode } from "preact";
import { Code, Screenshot } from "../components";
import { type Shot, shots } from "../images";
import type { Page } from "../layout";
import { href, site } from "../site";

const half = "(min-width: 60rem) 34rem, 100vw";

type Stop = { shot: Shot; alt: string; title: string; text: string };

/** One part of the app: a heading, a lede, screenshots with captions, then what it does. */
type Part = {
  id: string;
  title: string;
  lede: string;
  stops: Stop[];
  features: (readonly [string, string])[];
  fine?: string;
  /** A page that says more. */
  more?: { path: string; label: string };
};

const parts: Part[] = [
  {
    id: "chat",
    title: "Google Chat",
    lede: "Every DM, group, space, and app in one list, with Mentions and Starred across all of them. Pick the layout that fits how you work and switch any time in Settings.",
    stops: [
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
    ],
    features: [
      [
        "Four layouts",
        "Google Chat's sidebar, a Messages-style list with previews, a Rail of categories, or an IRC-style list of names with a monospaced message style to match.",
      ],
      [
        "Full conversations",
        "Formatted text, threads, quotes, edits, reactions, cards, attachments, and read receipts.",
      ],
      [
        "Format as you write",
        "Bold, italic, strikethrough, lists, and code, from a bar or from the keyboard.",
      ],
      [
        "Find anyone fast",
        "⌘K jumps to any conversation. Filter the list to people, groups, spaces, apps, or unread, and sort it by recent or A to Z.",
      ],
      ["Media viewer", "Images, video, audio, PDF, and text files open inside the app."],
      [
        "Rewrite a draft",
        "⌘⇧R rewrites your message through the Claude Code or Codex CLI you already have.",
      ],
    ],
    fine: "Shortcuts are written the macOS way. On Linux and Windows, Ctrl takes the place of Cmd.",
  },
  {
    id: "mail",
    title: "Gmail",
    lede: "⌘4 opens Mail: your Gmail in the same window, through the session you signed in with. There is no second sign-in and no API key to set up.",
    stops: [
      {
        shot: shots.mail,
        alt: "The Mail view in Working: the Inbox list beside the reading pane, where Mei Tanaka's email New beans in the kitchen shows its photo of a coffee grinder, a bag of beans, and a pour-over carafe",
        title: "Inbox and reading pane.",
        text: "The thread list beside the open email, which renders as it was sent.",
      },
      {
        shot: shots.mailLabels,
        alt: "The Labels browser open over the Inbox in Working: a filter field above the label tree, with Clients, Travel, and Vendors open over their nested labels, each label's color, and unread counts on Design, Launch, Newsletters, and Team",
        title: "Labels.",
        text: "Your own labels as a tree with their colors and unread counts, filtered as you type.",
      },
    ],
    features: [
      [
        "Mailboxes and labels",
        "Inbox, Starred, Sent, and All mail. The Labels button or ⌘L opens your own labels as a tree with their colors and unread counts, filtered as you type.",
      ],
      ["Gmail search", "The search field takes the same queries as Gmail's search box."],
      [
        "Write new emails",
        "C or the New email button opens a form with To and Cc as chips, suggested from the people directory, a subject, and a body. ⌘Enter sends.",
      ],
      [
        "Reply, archive, mark unread",
        "Reply or Reply all under the last email. Archive from the Inbox with an Undo bar, or mark a thread unread again. A thread you open is marked read.",
      ],
      [
        "Your signature and Rewrite",
        "New emails and replies carry the signature you set in Gmail, shown under the body with a button to leave it out. ⌘⇧R rewrites the body, as in chat.",
      ],
      [
        "Emails as they were sent",
        "Each email renders its HTML in the reading pane, in your theme's colors where it sets none. Its scripts, frames, and forms never run, and links open in your browser.",
      ],
      [
        "Light or dark emails",
        "A switch on an email shows every HTML email on the other background: light under a dark theme, dark under a light one. Working remembers it.",
      ],
      ["Attachments", "Files open in the same media viewer as chat attachments."],
      [
        "Opens from the cache",
        "The inbox's first page shows from disk before Gmail answers, and new mail is checked every minute.",
      ],
    ],
    fine: "The Mail view does not forward, star, label, or delete; an agent you allow can forward, star, and label through the mail tools. Turn Mail off in Settings, Features, and Working does not read Gmail at all.",
  },
  {
    id: "calendar",
    title: "Google Calendar and Meet",
    lede: "⌘2 opens Calendar and ⌘3 opens Meet, beside your chats and your mail.",
    stops: [
      {
        shot: shots.calendar,
        alt: "The Calendar week view in Working, with an event card offering Join with Google Meet and a Going? reply",
        title: "Calendar.",
        text: "Day to year views, and event cards that join with Google Meet and answer going or not.",
      },
      {
        shot: shots.meet,
        alt: "The Meet view in Working: a day's meetings beside the field that joins with a code or nickname",
        title: "Meet.",
        text: "The day's meetings, a field to join with a code or a link, and new meetings in a click.",
      },
    ],
    features: [
      [
        "Make and change events",
        "The editor sets the time, the guests, and the notifications of a new event or an existing one.",
      ],
      [
        "Calls in their own window",
        "Camera, microphone, screen sharing, and the call's chat, on macOS and Windows. On Linux, meetings open in your browser.",
      ],
      [
        "Call from a chat",
        "The call button of a conversation posts a meeting in it, then joins it.",
      ],
    ],
    fine: "Calendar and Meet each have their own switch in Settings, Features. With Meet off, meeting links open in your browser.",
  },
  {
    id: "notifications",
    title: "Notifications",
    lede: "Working posts through your system's notifications, and a click opens what each one is about.",
    stops: [],
    features: [
      [
        "Messages",
        "New messages and mentions, following each conversation's notification setting.",
      ],
      [
        "New mail",
        "One for each unread email that reaches the inbox, with its sender, subject, and a preview. A click opens the thread.",
      ],
      [
        "Event reminders",
        "At each event's own reminder time, or the calendar's default. A click opens the event's day and card.",
      ],
      [
        "Unread badge",
        "The Dock icon counts unread chats and unread inbox threads. On Linux the count goes on the launcher; Windows has no taskbar count.",
      ],
      [
        "Quiet when you want",
        "Do-not-disturb schedules and a custom status. New mail and event reminders each have their own switch in Settings.",
      ],
    ],
  },
  {
    id: "agents",
    title: "AI agents",
    lede: "Working serves an optional MCP server to AI agents, with a group of tools each for Chat, Mail, Calendar, and Meet. Each group is off until you turn it on in Settings. By default the server listens on your computer only, and Settings can give it another address. It always needs a token. Turning Calendar, Meet, or Mail off in Settings, Features turns its tools off too.",
    stops: [],
    features: [
      [
        "Chat",
        "Agents catch up on unread chats, find a conversation, read it, and search every message. Sending is a separate switch.",
      ],
      [
        "Mail",
        "Agents list, search, and read your Gmail threads and labels. Sending, replying, archiving, and marking read or unread are a separate switch.",
      ],
      [
        "Calendar",
        "Agents read your calendars and events. Creating, changing, deleting, and answering invitations is a separate switch.",
      ],
      ["Meet", "Agents make a meeting link and list the meetings of the next 7 days."],
    ],
    more: { path: "/mcp/", label: "Connect an agent" },
  },
  {
    id: "yours",
    title: "Make it yours",
    lede: "One theme for light and one for dark, your own fonts, and the layout, message style, and sidebar you want.",
    stops: [
      {
        shot: shots.themes,
        alt: "The theme picker in Working previewing Ayu Dark, with Working Dark marked as the current theme",
        title: "28 themes.",
        text: "Ayu, Catppuccin, Dracula, Tokyo Night, and more, previewed live with ⌘⇧T.",
      },
      {
        shot: shots.settings,
        alt: "Working's Settings open on the density and layout options",
        title: "Settings.",
        text: "Layout, message style, sidebar side and width, and auto-hide, each a click away.",
      },
    ],
    features: [
      [
        "Number keys",
        "⌘1 to ⌘9 open a view or a conversation. Right-click a button or a conversation to give it one.",
      ],
      [
        "Only what you use",
        "Settings, Features turns Calendar, Meet, or Mail off as a whole: its view, buttons, notifications, and agent tools go. The Mentions and Starred buttons can go too.",
      ],
      ["Updates itself", "A new release installs from inside the app."],
    ],
  },
];

const questions = [
  [
    "Is there a Google Chat desktop app?",
    "Google Chat itself lives in a browser tab or a wrapped web page. Working is a native app for macOS, Linux, and Windows that puts Chat beside Gmail, Calendar, and Meet: it opens from a local cache, keeps up through Google's realtime stream, and uses your system's notifications and unread badge.",
  ],
  [
    "Can I read and write Gmail in Working?",
    "Yes. Mail shows the Inbox, Starred, Sent, All mail, and your labels, and searches with Gmail's own queries. It writes new emails and replies with your Gmail signature, archives, and marks threads unread. It uses the session you signed in with. The Mail view does not forward, star, label, or delete; an agent you allow can forward, star, and label through the mail tools.",
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
    "Yes. Every release has an AppImage for x86_64 and aarch64, with Chat, Mail, and Calendar. It needs GTK 3, WebKitGTK 4.1, libsoup 3, and an X11 or XWayland session. Meet calls open in your browser.",
  ],
  [
    "Where does Working keep my session?",
    "On your computer. Settings, the Google session, and the cache of messages and mail live in folders on your own disk.",
  ],
  [
    "Can AI agents read my Google Chat and Gmail through Working?",
    "Only if you turn it on. Working has an optional MCP server, with a switch for each group of tools: Chat, Mail, Calendar, and Meet. By default it listens on your computer only, and Settings can give it another address. It always needs a token. Sending messages, sending and changing mail, and changing events are each a separate switch.",
  ],
] as const;

function Part({ part }: { part: Part }): VNode {
  return (
    <section class="band" aria-labelledby={part.id}>
      <h2 id={part.id}>{part.title}</h2>
      <p class="lede">{part.lede}</p>
      {part.stops.length > 0 && (
        <div class="tour">
          {part.stops.map((stop) => (
            <figure>
              <Screenshot shot={stop.shot} alt={stop.alt} sizes={half} />
              <figcaption>
                <strong>{stop.title}</strong> {stop.text}
              </figcaption>
            </figure>
          ))}
        </div>
      )}
      <dl class="features">
        {part.features.map(([name, text]) => (
          <div>
            <dt>{name}</dt>
            <dd>{text}</dd>
          </div>
        ))}
      </dl>
      {part.fine && <p class="fine">{part.fine}</p>}
      {part.more && (
        <p>
          <a href={href(part.more.path)}>{part.more.label}</a>
        </p>
      )}
    </section>
  );
}

function Home(): VNode {
  return (
    <>
      <section class="hero">
        <h1>{site.tagline}</h1>
        <p class="lede">
          Working is a native app for the Google account you sign in with: Chat, Gmail, Calendar,
          and Meet side by side in one window, on macOS, Linux, and Windows. It is written in Rust,
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
          alt="Working, a native desktop app for Google Chat, Gmail, Calendar, and Meet, in the Google Chat layout: the sidebar with its sections beside the Product launch space, where a link preview of the new landing page has a reaction"
          sizes="(min-width: 72rem) 70rem, 100vw"
          eager
        />
      </section>

      {parts.map((part) => (
        <Part part={part} />
      ))}

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
            <a href={href("/mac/")}>Google Chat and Gmail for Mac</a>
          </section>
          <section>
            <h3>Linux</h3>
            <p>
              The <code>.AppImage</code> for x86_64 or aarch64. Make it executable, then run it:
            </p>
            <Code>chmod +x Working_*.AppImage</Code>
            <p>Needs GTK 3, WebKitGTK 4.1, libsoup 3, and X11 or XWayland.</p>
            <a href={href("/linux/")}>Google Chat and Gmail for Linux</a>
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
            <a href={href("/windows/")}>Google Chat and Gmail for Windows</a>
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
  title: "Working: Google Chat, Gmail, Calendar, and Meet desktop app",
  description:
    "Working is a free, open-source native app for Google Chat, Gmail, Google Calendar, and Meet in one window, on macOS, Linux, and Windows.",
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
