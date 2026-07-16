import { readFileSync, readdirSync, realpathSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, relative, resolve, sep } from "node:path";

const root = realpathSync(resolve("dist"));
const rootPrefix = `${root}${sep}`;
const port = Number(process.env.SETWRIGHT_PDF_PORT ?? "4173");
const tauriConfig = JSON.parse(readFileSync(resolve("src-tauri/tauri.conf.json"), "utf8"));
const csp = tauriConfig.app?.security?.csp;

if (typeof csp !== "string" || csp.length === 0) {
  throw new Error("The production Tauri CSP is missing.");
}

const contentTypes = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".mjs", "text/javascript; charset=utf-8"],
  [".pdf", "application/pdf"],
  [".svg", "image/svg+xml"],
]);

function indexAssets(directory, assets) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const candidate = join(directory, entry.name);
    if (entry.isDirectory()) {
      indexAssets(candidate, assets);
      continue;
    }
    if (!entry.isFile()) continue;

    const canonical = realpathSync(candidate);
    if (!canonical.startsWith(rootPrefix)) {
      throw new Error(`Build output escaped the static root: ${canonical}`);
    }
    const route = `/${relative(root, canonical).split(sep).join("/")}`;
    assets.set(route, {
      bytes: readFileSync(canonical),
      contentType: contentTypes.get(extname(canonical)) ?? "application/octet-stream",
    });
  }
}

const assets = new Map();
indexAssets(root, assets);
const indexAsset = assets.get("/index.html");
if (indexAsset === undefined) throw new Error("The PDF E2E build has no index.html.");

function resolveAsset(requestUrl) {
  try {
    const pathname = decodeURIComponent(new URL(requestUrl ?? "/", "http://127.0.0.1").pathname);
    if (pathname.includes("\0") || pathname.includes("\\") || pathname.split("/").some((part) => part === "." || part === "..")) {
      return null;
    }
    const route = pathname === "/" ? "/index.html" : pathname.replace(/\/{2,}/g, "/");
    // Filesystem access happens only while indexing trusted build output.
    // Request data selects an already-loaded record and never forms a path.
    return assets.get(route) ?? indexAsset;
  } catch {
    return null;
  }
}

createServer((request, response) => {
  const asset = resolveAsset(request.url);
  if (asset === null) {
    response.writeHead(400, { "Content-Type": "text/plain; charset=utf-8" });
    response.end("Invalid path");
    return;
  }

  response.writeHead(200, {
    "Cache-Control": "no-store",
    "Content-Security-Policy": csp,
    "Content-Length": String(asset.bytes.byteLength),
    "Content-Type": asset.contentType,
    "X-Content-Type-Options": "nosniff",
  });
  response.end(asset.bytes);
}).listen(port, "127.0.0.1", () => {
  process.stdout.write(`Setwright PDF CSP server listening on http://127.0.0.1:${String(port)}\n`);
});
