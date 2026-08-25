import { cp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const websiteRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const output = resolve(websiteRoot, "dist/static");
const workerUrl = new URL("../dist/server/index.js", import.meta.url);
workerUrl.searchParams.set("export", `${process.pid}-${Date.now()}`);
const { default: worker } = await import(workerUrl.href);

const response = await worker.fetch(
  new Request("https://open-agent-view.github.io/", {
    headers: { accept: "text/html" },
  }),
  { ASSETS: { fetch: async () => new Response("Not found", { status: 404 }) } },
  { waitUntil() {}, passThroughOnException() {} },
);

if (!response.ok) {
  throw new Error(`static render failed with HTTP ${response.status}`);
}

const html = await response.text();
if (!html.includes("See every agent") || html.includes("/_next/image?")) {
  throw new Error("static render is incomplete or depends on a server-side image optimizer");
}

await rm(output, { recursive: true, force: true });
await mkdir(output, { recursive: true });
await cp(resolve(websiteRoot, "dist/client"), output, { recursive: true });
await writeFile(resolve(output, "index.html"), html);
await writeFile(resolve(output, ".nojekyll"), "");

const exported = await readFile(resolve(output, "index.html"), "utf8");
if (!exported.includes("https://open-agent-view.github.io/install.sh")) {
  throw new Error("static export lost the canonical install command");
}

console.log(`exported static site to ${output}`);
