import type { Page } from "../layout";
import { home } from "./home";
import { linux } from "./linux";
import { mac } from "./mac";
import { mcp } from "./mcp";
import { notFound } from "./not-found";
import { windows } from "./windows";

export const pages: Page[] = [home, mac, linux, windows, mcp, notFound];
