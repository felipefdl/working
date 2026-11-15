import { join } from "node:path";

const built = await Bun.build({ entrypoints: [join(import.meta.dir, "styles.css")], minify: true });
const [sheet] = built.outputs;
if (!sheet) throw new Error("styles.css did not build");

/** The whole stylesheet, minified. Every page carries it inline: one request paints the page. */
export const css = await sheet.text();
