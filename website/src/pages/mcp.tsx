import type { VNode } from "preact";
import { Code } from "../components";
import { Article, type Page } from "../layout";
import { href } from "../site";

const tools = [
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
    "Text, files, or both to one conversation the agent has read. Listed only when sending is on.",
  ],
] as const;

const client = `{
  "mcpServers": {
    "working": {
      "type": "http",
      "url": "http://127.0.0.1:3021/mcp",
      "headers": { "Authorization": "Bearer <contents of mcp-token>" }
    }
  }
}`;

function Mcp(): VNode {
  return (
    <Article
      crumb="MCP server"
      title="Google Chat MCP server"
      lede="Working includes an optional MCP server, so AI agents on your computer can catch up on your Google Chat, find a conversation, and search every message. Sending is a separate switch that starts off."
    >
      <section>
        <h2>Turn it on</h2>
        <ol>
          <li>
            Install Working for <a href={href("/mac/")}>Mac</a>, <a href={href("/linux/")}>Linux</a>
            , or <a href={href("/windows/")}>Windows</a>, and sign in.
          </li>
          <li>Open Settings and turn on the MCP server.</li>
          <li>
            Point your agent at <code>http://127.0.0.1:3021/mcp</code> with the token from the{" "}
            <code>mcp-token</code> file beside the settings.
          </li>
        </ol>
      </section>

      <section>
        <h2>Connect an agent</h2>
        <p>
          The server speaks MCP over HTTP and expects the token as a bearer header. Many clients
          take a block like this one:
        </p>
        <Code>{client}</Code>
        <p>
          The token is in <code>~/.config/working/mcp-token</code> on macOS and Linux, and in{" "}
          <code>%APPDATA%\working\mcp-token</code> on Windows. It is a secret: anyone who has it can
          read your chats through the server.
        </p>
      </section>

      <section>
        <h2>What an agent can do</h2>
        <table>
          <thead>
            <tr>
              <th scope="col">Tool</th>
              <th scope="col">What it returns or does</th>
            </tr>
          </thead>
          <tbody>
            {tools.map(([name, text]) => (
              <tr>
                <td>
                  <code>{name}</code>
                </td>
                <td>{text}</td>
              </tr>
            ))}
          </tbody>
        </table>
        <p>
          A conversation can be named the way a person would: a name, part of one, or an id. Reading
          changes nothing: messages stay unread.
        </p>
      </section>

      <section>
        <h2>What keeps it safe</h2>
        <ul>
          <li>The server is off until you turn it on.</li>
          <li>It listens on your computer only, never on the network.</li>
          <li>Every request needs the token.</li>
          <li>
            Sending has its own switch, and a message goes only to a conversation the agent has just
            read.
          </li>
        </ul>
      </section>
    </Article>
  );
}

export const mcp: Page = {
  path: "/mcp/",
  crumb: "MCP server",
  title: "Google Chat MCP server for AI agents | Working",
  description:
    "Working includes a local MCP server so AI agents on your computer can read and search your Google Chat, and send messages only when you allow it.",
  body: Mcp,
};
