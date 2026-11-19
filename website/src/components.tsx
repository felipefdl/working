import type { ComponentChildren, VNode } from "preact";
import { type Shot, shotUrl } from "./images";
import { href, site } from "./site";

/** A screenshot at the width the screen needs. `eager` is for the one image above the fold. */
export function Screenshot(props: {
  shot: Shot;
  alt: string;
  sizes: string;
  eager?: boolean;
}): VNode {
  const { shot } = props;
  return (
    <img
      class="shot"
      src={shotUrl(shot, shot.widths[1] ?? shot.width)}
      srcset={shot.widths.map((width) => `${shotUrl(shot, width)} ${width}w`).join(", ")}
      sizes={props.sizes}
      width={shot.width}
      height={shot.height}
      alt={props.alt}
      loading={props.eager ? "eager" : "lazy"}
      fetchpriority={props.eager ? "high" : undefined}
      decoding={props.eager ? undefined : "async"}
    />
  );
}

/** A command or file content to copy. */
export function Code({ children }: { children: string }): VNode {
  return (
    <pre>
      <code>{children}</code>
    </pre>
  );
}

export function Download({ children }: { children: ComponentChildren }): VNode {
  return (
    <p class="actions">
      <a class="button" href={site.releases}>
        {children}
      </a>
      <a href={href("/#install")}>Other systems</a>
    </p>
  );
}

const exceptions = [
  ["Next or previous conversation", "Alt+Down, Alt+Up"],
  ["Newest unread conversation", "Shift+Alt+Down"],
  ["Open or close the sidebar", "Ctrl+Shift+S"],
] as const;

/** The shortcuts Linux and Windows share. */
export function Shortcuts(): VNode {
  return (
    <section>
      <h2>Keyboard shortcuts</h2>
      <p>
        Ctrl takes the place of Cmd: <kbd>Ctrl+K</kbd> jumps to any conversation, and{" "}
        <kbd>Ctrl+1</kbd> to <kbd>Ctrl+4</kbd> open Home, Calendar, Meet, and Mail. In Mail,{" "}
        <kbd>Ctrl+L</kbd> opens the labels and <kbd>Ctrl+Enter</kbd> sends an email or a reply.
        These differ from the Mac:
      </p>
      <table>
        <thead>
          <tr>
            <th scope="col">Action</th>
            <th scope="col">Shortcut</th>
          </tr>
        </thead>
        <tbody>
          {exceptions.map(([action, keys]) => (
            <tr>
              <td>{action}</td>
              <td>
                <kbd>{keys}</kbd>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <p>Settings and the app's menus show each shortcut the way your system writes it.</p>
    </section>
  );
}

/** Where the app keeps its files on one system. */
export function Files(props: { settings: string; cache: string; note?: string }): VNode {
  return (
    <section>
      <h2>Where Working keeps its files</h2>
      <p>
        Settings, the Google session, and the MCP token live in <code>{props.settings}</code>. The
        cache lives in <code>{props.cache}</code>. {props.note}
      </p>
    </section>
  );
}
