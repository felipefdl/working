import type { VNode } from "preact";
import type { Page } from "../layout";
import { href } from "../site";

function NotFound(): VNode {
  return (
    <article class="doc">
      <h1>This page does not exist</h1>
      <p class="lede">
        The link may be old or mistyped. The <a href={href("/")}>home page</a> has the downloads and
        the guides for each system.
      </p>
    </article>
  );
}

export const notFound: Page = {
  path: "/404.html",
  hidden: true,
  title: "Page not found | Working",
  description:
    "This page does not exist. The home page of Working, the native Google Chat desktop app, has the downloads and the guides for each system.",
  body: NotFound,
};
