import type { VNode } from "preact";
import { Download, Files, Screenshot, Shortcuts } from "../components";
import { shots } from "../images";
import { Article, type Page } from "../layout";
import { site } from "../site";

function Windows(): VNode {
  return (
    <Article
      crumb="Windows"
      title="Google Chat for Windows"
      lede="Working is a native Google Chat app for Windows on x64 and Arm64: system notifications, keyboard shortcuts, Meet calls in their own window, and a start from a local cache instead of a browser tab."
    >
      <Download>Download for Windows</Download>
      <Screenshot
        shot={shots.windows}
        alt="Working, a native Google Chat app, on Windows with Windows' own window buttons, in the Ayu Dark theme and the Google Chat layout: the sidebar beside a chat with a photo of a bowl of ramen"
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
  title: "Google Chat for Windows: native desktop app | Working",
  description:
    "Install Working, a native Google Chat app for Windows on x64 and Arm64. A per-user installer with no administrator rights, notifications, and Meet calls.",
  body: Windows,
};
