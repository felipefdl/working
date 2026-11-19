import type { VNode } from "preact";
import { Download, Files, Screenshot, Shortcuts } from "../components";
import { shots } from "../images";
import { Article, type Page } from "../layout";
import { site } from "../site";

function Windows(): VNode {
  return (
    <Article
      crumb="Windows"
      title="Google Chat and Gmail for Windows"
      lede="Working is a native app for Google Chat, Gmail, Google Calendar, and Meet on Windows, on x64 and Arm64: system notifications, keyboard shortcuts, Meet calls in their own window, and a start from a local cache instead of a browser tab."
    >
      <Download>Download for Windows</Download>
      <Screenshot
        shot={shots.windows}
        alt="Working, a native Google Chat and Gmail app, on Windows with Windows' own window buttons, in the Ayu Dark theme and the Google Chat layout: the sidebar beside a chat with a photo of a bowl of ramen"
        sizes="(min-width: 48rem) 46rem, 100vw"
        eager
      />

      <section>
        <h2>Install</h2>
        <ol>
          <li>
            Download the <code>-setup.exe</code> for your architecture from the{" "}
            <a href={site.releases}>latest release</a>.
          </li>
          <li>
            Run it. SmartScreen warns because the installer is not code-signed: choose More info,
            then Run anyway.
          </li>
          <li>Open Working from the Start Menu and sign in with Google on the welcome screen.</li>
        </ol>
      </section>

      <section>
        <h2>What the installer does</h2>
        <p>
          It sets Working up for your user only, with no administrator rights, and installs the
          Microsoft WebView2 runtime if it is missing. Notifications show only for the installed
          app, whose Start Menu shortcut carries the app's identity.
        </p>
      </section>

      <section>
        <h2>What Windows has</h2>
        <ul>
          <li>
            Google Chat, Mail, Calendar, and Meet, as on macOS. Emails and calls render in WebView2.
          </li>
          <li>
            Calls in their own window, with camera, microphone, the call's chat, and WebView2's own
            screen sharing.
          </li>
          <li>Notifications for new messages, new mail, and event reminders.</li>
          <li>No unread count: Windows has none on a taskbar icon.</li>
        </ul>
      </section>

      <section>
        <h2>Known limits</h2>
        <p>
          In Mail, an email scrolled partly out of the reading pane keeps drawing past the pane,
          over the Mail toolbar and the thread list. On Windows the web view library cannot clip an
          email to a scrolling pane.
        </p>
      </section>

      <section>
        <h2>Updates</h2>
        <p>Working downloads a new release and runs its installer when you restart the app.</p>
      </section>

      <Shortcuts />
      <Files
        settings="%APPDATA%\working"
        cache="%LOCALAPPDATA%\dev.felipefdl.working\cache"
        note="The web views keep their data in %LOCALAPPDATA%\dev.felipefdl.working\WebView2."
      />
    </Article>
  );
}

export const windows: Page = {
  path: "/windows/",
  crumb: "Windows",
  title: "Google Chat and Gmail for Windows: native app | Working",
  description:
    "Working is a native Windows app for Google Chat, Gmail, Calendar, and Meet on x64 and Arm64. Per-user installer with no admin rights, notifications, and calls.",
  body: Windows,
};
