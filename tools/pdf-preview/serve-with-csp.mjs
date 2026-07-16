import { readFileSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, normalize, resolve, sep } from "node:path";

const root = resolve(process.env.SETWRIGHT_PDF_DIST ?? "dist");
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

function resolveAsset(requestUrl) {
  const pathname = decodeURIComponent(new URL(requestUrl ?? "/", "http://127.0.0.1").pathname);
  const relative = normalize(pathname === "/" ? "index.html" : pathname.replace(/^\/+/, ""));
  const candidate = resolve(join(root, relative));
  if (candidate !== root && !candidate.startsWith(`${root}${sep}`)) return null;
  try {
    return statSync(candidate).isFile() ? candidate : resolve(join(root, "index.html"));
  } catch {
    return resolve(join(root, "index.html"));
  }
}

createServer((request, response) => {
  const asset = resolveAsset(request.url);
  if (asset === null) {
    response.writeHead(400, { "Content-Type": "text/plain; charset=utf-8" });
    response.end("Invalid path");
    return;
  }

  const bytes = readFileSync(asset);
  response.writeHead(200, {
    "Cache-Control": "no-store",
    "Content-Security-Policy": csp,
    "Content-Length": String(bytes.byteLength),
    "Content-Type": contentTypes.get(extname(asset)) ?? "application/octet-stream",
    "X-Content-Type-Options": "nosniff",
  });
  response.end(bytes);
}).listen(port, "127.0.0.1", () => {
  process.stdout.write(`Setwright PDF CSP server listening on http://127.0.0.1:${String(port)}\n`);
});
