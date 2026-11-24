import type { VNode } from "preact";
import { Code } from "../components";
import { Article, type Page } from "../layout";
import { href } from "../site";

type Group = {
  id: string;
  title: string;
  lede: string;
  tools: (readonly [string, string])[];
};

const groups: Group[] = [
  {
    id: "chat",
    title: "Chat",
    lede: "Google Chat. A conversation can be named the way a person would: a name, part of one, or an id. Reading changes nothing: messages stay unread.",
    tools: [
      ["inbox", "The unread conversations, newest first, each with its unread messages."],
      ["recent_activity", "Every conversation waiting on a reply from you in the last 24 hours."],
      [
        "find_conversations",
        "A direct message, group chat, space, or app by name, part of a name, or email.",
      ],
      [
        "read_conversation",
        "The messages of one conversation, oldest first, with thread replies under the message they answer.",
      ],
      ["search_messages", "The text of messages in every conversation, however old."],
      [
        "send_message",
        "Text, files, or both to one conversation the agent has read. Listed only with Let agents send messages.",
      ],
    ],
  },
  {
    id: "mail",
    title: "Mail",
    lede: "Gmail. Reading leaves threads unread. The nine tools that send and change mail are listed only with Let agents send and change mail; without it they answer that writing mail is turned off.",
    tools: [
      [
        "mail_inbox",
        "The newest threads of the Inbox, Starred, Sent, All mail, or one of your labels, 50 a page.",
      ],
      ["mail_search", "The threads that match Gmail's own search words, as in its search box."],
      [
        "mail_read_thread",
        "The emails of one thread, oldest first, as plain text: each email's id, who wrote it, to, copied, and hidden copies on your own sent emails, when, its files numbered, and its body without the earlier emails it quotes. A draft not sent is marked as one.",
      ],
      [
        "mail_html",
        "Saves the emails of one thread as HTML pages in Working's own folder, one page an email, as the email looks in Gmail, with the pictures inside it embedded. An agent opens a page in a browser or prints it to a PDF.",
      ],
      [
        "mail_eml",
        "Saves the emails of one thread as .eml files in Working's own folder, each the email exactly as it was sent, with every header, part, and attached file, for archiving it or opening it in another mail program. A draft has none, and files over 25 MB are refused.",
      ],
      ["mail_labels", "Your own labels, nested ones as Parent/Child, with their unread counts."],
      [
        "mail_attachment",
        "Saves one file of an email in Working's own folder, which Working empties when it starts, and gives a text file's text too. Files over 25 MB are refused.",
      ],
      [
        "mail_send",
        "Sends a new email from your address to the people in to, cc, and bcc, with a subject and text. Your Gmail signature is added unless the agent asks not to.",
      ],
      [
        "mail_reply",
        "Answers in a thread, quoting the email as Gmail does: the newest one from someone else, or the one message names. reply_all answers everyone. The recipients are the ones Gmail's Reply and Reply all pick; the agent does not choose them.",
      ],
      [
        "mail_forward",
        "Sends an email on as a new email on a thread of its own, with Fwd: before the subject and the email quoted under what the agent writes. Its attached files are not sent with it.",
      ],
      [
        "mail_draft",
        "Saves a new email, with to, cc, and bcc, or a reply in your Gmail Drafts without sending it, for you to look over. Each call makes a new draft.",
      ],
      [
        "mail_send_draft",
        "Sends a draft the agent saved, exactly as saved. After Working restarts, you send it from Gmail's Drafts.",
      ],
      ["mail_archive", "Moves threads out of the inbox, or back into it."],
      ["mail_mark", "Marks threads read or unread."],
      ["mail_star", "Stars threads, or takes the star off."],
      [
        "mail_label",
        "Puts your own labels on threads, or takes them off. Making a label is left to Gmail.",
      ],
    ],
  },
  {
    id: "calendar",
    title: "Calendar",
    lede: "Google Calendar. The four tools that change events are listed only with Let agents change events. Guests are emailed only when the agent asks for it.",
    tools: [
      ["calendar_list", "Your calendars and what you may do in each."],
      ["calendar_events", "Your events in a date range: today and the next 7 days unless asked."],
      [
        "calendar_get_event",
        "One event in full, with its description, organizer, and whether it repeats.",
      ],
      ["calendar_create_event", "A new event in your calendar."],
      ["calendar_update_event", "Changes to an event: only what the agent passes."],
      ["calendar_delete_event", "Deletes an event."],
      ["calendar_respond", "Answers an invitation: accept, decline, or maybe."],
    ],
  },
  {
    id: "meet",
    title: "Meet",
    lede: "Google Meet.",
    tools: [
      [
        "meet_create_meeting",
        "A new meeting's link, to share with whoever should join. It is in no chat and on no calendar.",
      ],
      [
        "meet_upcoming",
        "Your calendar events in the next 7 days that have a Meet link, soonest first.",
      ],
    ],
  },
];

const claude =
  'claude mcp add --scope user --transport http working http://127.0.0.1:3021/mcp --header "Authorization: Bearer <contents of mcp-token>"';

const client = `{
  "mcpServers": {
    "working": {
      "type": "http",
      "url": "http://127.0.0.1:3021/mcp",
      "headers": { "Authorization": "Bearer <contents of mcp-token>" }
    }
  }
}`;

function Tools({ group }: { group: Group }): VNode {
  return (
    <section>
      <h3 id={group.id}>{group.title}</h3>
      <p>{group.lede}</p>
      <table>
        <thead>
          <tr>
            <th scope="col">Tool</th>
            <th scope="col">What it returns or does</th>
          </tr>
        </thead>
        <tbody>
          {group.tools.map(([name, text]) => (
            <tr>
              <td>
                <code>{name}</code>
              </td>
              <td>{text}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

function Mcp(): VNode {
  return (
    <Article
      crumb="MCP server"
      title="An MCP server for Google Chat, Gmail, Calendar, and Meet"
      lede="Working serves one MCP server, so AI agents on your computer can catch up on your Google Chat, read and answer your Gmail, check your calendar, and make Meet links. Each group of tools has its own switch and is off until you turn it on."
    >
      <section>
        <h2>Turn it on</h2>
        <ol>
          <li>
            Install Working for <a href={href("/mac/")}>Mac</a>, <a href={href("/linux/")}>Linux</a>
            , or <a href={href("/windows/")}>Windows</a>, and sign in.
          </li>
          <li>
            Open Settings, then Agents, and turn on the tool groups you want: Chat, Mail, Calendar,
            and Meet. Sending chat messages, sending and changing mail, and changing events each
            have a second switch under their group. The Calendar, Meet, and Mail tools also need
            their feature on in Settings, Features.
          </li>
          <li>
            Choose Add to an agent in the same place, pick your agent, and copy its setup. Copy link
            and Copy token are there too.
          </li>
        </ol>
      </section>

      <section>
        <h2>Connect an agent</h2>
        <p>
          By default the server listens on your computer only, at{" "}
          <code>http://127.0.0.1:3021/mcp</code>, and takes one token. Change, on the Address row in
          Settings, Agents, sets where it listens: an address such as <code>0.0.0.0</code> shares it
          with your network. It speaks MCP over HTTP. With Claude Code:
        </p>
        <Code>{claude}</Code>
        <p>
          Add to an agent in Settings, Agents has the setup for Codex, Grok, Pi, omp, Hermes,
          OpenClaw, and Antigravity too, for every project. Many other clients take a block like
          this one:
        </p>
        <Code>{client}</Code>
        <p>
          The token can also be the last part of the path, as in{" "}
          <code>http://127.0.0.1:3021/mcp/&lt;token&gt;</code>, for an agent that only takes a URL.
          It is in <code>~/.config/working/mcp-token</code> on macOS and Linux, and in{" "}
          <code>%APPDATA%\working\mcp-token</code> on Windows. It is a secret: anyone who has it can
          use every tool you turned on. Settings replaces it with a new one.
        </p>
      </section>

      <section>
        <h2>What an agent can do</h2>
        <p>
          The server tells the agent its name, <code>working</code>, and lists the tools of the
          groups that are on. A tool of a group that is off is not listed.
        </p>
        {groups.map((group) => (
          <Tools group={group} />
        ))}
      </section>

      <section>
        <h2>What keeps it safe</h2>
        <ul>
          <li>
            Every tool group is off until you turn it on, and with every group off the server does
            not listen. Turning Calendar, Meet, or Mail off in Settings, Features turns its tools
            off too, whatever their own switch says.
          </li>
          <li>
            By default it listens on your computer only. Another address in Settings, Agents makes
            it reachable from your network.
          </li>
          <li>
            Every request needs the token, and a request from a web page on another address is
            refused.
          </li>
          <li>
            Sending a chat message has its own switch, and a message goes only to a conversation the
            agent has read since its last send.
          </li>
          <li>
            Sending and changing mail has its own switch. An email an agent sends is real: it
            reaches its recipients at once, from your address. An agent can save it in your Gmail
            Drafts instead, for you to look over before it goes.
          </li>
          <li>Changing events has its own switch.</li>
          <li>
            Run with <code>WORKING_READONLY=1</code> and every tool that sends or changes something
            refuses, whatever the switches say.
          </li>
        </ul>
      </section>
    </Article>
  );
}

export const mcp: Page = {
  path: "/mcp/",
  crumb: "MCP server",
  title: "MCP server for Google Chat, Gmail, and Calendar | Working",
  description:
    "Working serves a local MCP server for Google Chat, Gmail, Google Calendar, and Meet. Each tool group has its own switch, and sending and changing are separate.",
  body: Mcp,
};
