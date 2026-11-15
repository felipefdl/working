import type { VNode } from "preact";
import { Code, Download, Files, Screenshot, Shortcuts } from "../components";
import { shots } from "../images";
import { Article, type Page } from "../layout";
import { site } from "../site";

function Linux(): VNode {
  return (
    <Article
      crumb="Linux"
      title="Google Chat for Linux"
      lede="Working is a native Google Chat client for Linux, shipped as one AppImage for x86_64 and one for aarch64. No browser tab, no Electron: a Rust app that opens from a local cache."
    >
      <Download>Download the AppImage</Download>
      <Screenshot
        shot={shots.linux}
        alt="Working, a native Google Chat client, on Linux with its own window buttons, in the Messages layout: the chat list beside the Coffee chat space, where a photo of coffee beans gets a reply"
        sizes="(min-width: 48rem) 46rem, 100vw"
        eager
      />

      <section>
        <h2>Install</h2>
        <ol>
          <li>
            Download the <code>.AppImage</code> for your architecture from the{" "}
            <a href={site.releases}>latest release</a>.
          </li>
          <li>Make it executable and run it:</li>
        </ol>
        <Code>{"chmod +x Working_*.AppImage\n./Working_*.AppImage"}</Code>
        <p>
          Then sign in with Google on the welcome screen. The AppImage updates itself; a copy
          installed any other way does not.
        </p>
      </section>

      <section>
        <h2>Requirements</h2>
        <p>The AppImage uses these system libraries rather than carrying its own:</p>
        <ul>
          <li>
            GTK 3, WebKitGTK 4.1, and libsoup 3. On Debian and Ubuntu: <code>libgtk-3-0</code>,{" "}
            <code>libwebkit2gtk-4.1-0</code>, <code>libsoup-3.0-0</code>. On Fedora:{" "}
            <code>gtk3</code>, <code>webkit2gtk4.1</code>, <code>libsoup3</code>.
          </li>
          <li>An X11 session, or XWayland under Wayland.</li>
          <li>A glibc at least as new as the one in Ubuntu 24.04, where the release is built.</li>
        </ul>
      </section>

      <section>
        <h2>Apps menu</h2>
        <p>
          The first time the AppImage runs, a card in the window asks to add Working to your apps
          menu. Add writes a desktop entry and an icon under <code>~/.local/share</code>, so the
          dock and the launcher show Working's icon and name. No thanks writes nothing, and the app
          does not ask again. Settings › Appearance › Show in apps menu changes the answer later.
        </p>
      </section>

      <section>
        <h2>Meet on Linux</h2>
        <p>
          Meet opens in your default browser. WebKitGTK as Fedora builds it has no WebRTC, so a call
          cannot run inside the app. The call button still posts the meeting in the chat.
        </p>
      </section>

      <section>
        <h2>Known limits</h2>
        <ul>
          <li>
            In the composer, an emoji the interface font also has shows as that font's black glyph.
            The sent message shows it in color.
          </li>
          <li>In the media viewer, a video can stay black on some setups while its sound plays.</li>
          <li>
            Emoji are drawn with a bundled Noto Color Emoji on every distribution, so the system
            emoji font is not used.
          </li>
        </ul>
        <p>
          The full list is in the{" "}
          <a href={`${site.repo}/blob/main/docs/platforms.md#linux`}>platform notes</a>.
        </p>
      </section>

      <Shortcuts />
      <Files
        settings="$XDG_CONFIG_HOME/working (~/.config/working)"
        cache="$XDG_CACHE_HOME/working (~/.cache/working)"
      />
    </Article>
  );
}

export const linux: Page = {
  path: "/linux/",
  crumb: "Linux",
  title: "Google Chat for Linux: native AppImage client | Working",
  description:
    "Working is a native Google Chat client for Linux, shipped as an AppImage for x86_64 and aarch64. Requirements, apps menu setup, Meet, and known limits.",
  body: Linux,
};
