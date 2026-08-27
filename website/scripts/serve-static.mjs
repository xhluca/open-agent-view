#!/usr/bin/env node

import { createServer } from "node:http";
import { readFile, realpath, stat } from "node:fs/promises";
import { extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const port = Number.parseInt(process.argv[2] ?? "", 10);
if (!Number.isInteger(port) || port < 1 || port > 65_535) {
  throw new Error("usage: node scripts/serve-static.mjs <port>");
}

const root = await realpath(fileURLToPath(new URL("../dist/static/", import.meta.url)));
const mime = {
  ".cast": "application/x-asciicast",
  ".css": "text/css; charset=utf-8",
  ".gif": "image/gif",
  ".html": "text/html; charset=utf-8",
  ".ico": "image/x-icon",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".mp4": "video/mp4",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".webmanifest": "application/manifest+json",
  ".woff2": "font/woff2",
};

function isInsideRoot(path) {
  return path === root || path.startsWith(`${root}${sep}`);
}

const server = createServer(async (request, response) => {
  if (request.method !== "GET" && request.method !== "HEAD") {
    response.statusCode = 405;
    response.end("Method not allowed");
    return;
  }

  try {
    const pathname = decodeURIComponent(
      new URL(request.url ?? "/", "http://127.0.0.1").pathname,
    );
    let path = resolve(root, pathname.replace(/^\/+/, "") || "index.html");
    if (!isInsideRoot(path)) {
      response.statusCode = 403;
      response.end("Forbidden");
      return;
    }
    if ((await stat(path)).isDirectory()) {
      path = resolve(path, "index.html");
    }
    path = await realpath(path);
    if (!isInsideRoot(path)) {
      response.statusCode = 403;
      response.end("Forbidden");
      return;
    }

    response.setHeader("content-type", mime[extname(path)] ?? "application/octet-stream");
    response.setHeader("cache-control", "no-store");
    if (request.method === "HEAD") {
      response.end();
    } else {
      response.end(await readFile(path));
    }
  } catch {
    response.statusCode = 404;
    response.end("Not found");
  }
});

server.listen(port, "127.0.0.1", () => {
  process.stdout.write(`serving static export at http://127.0.0.1:${port}\n`);
});
