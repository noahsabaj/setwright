import { build, preview } from "vite";
import { resolve } from "node:path";
// Build the real PreviewPane and PDF.js worker. The test harness is a separate
// entrypoint and is never included in the shipping application.
const outDir = resolve("work/pdf-browser-build");
await build({ build: { outDir, emptyOutDir: true, rollupOptions: { input: resolve("test/browser/index.html") } } });
await preview({ build: { outDir }, preview: { host: "127.0.0.1", port: 1425, strictPort: true, headers: { "Content-Security-Policy": "default-src 'self'; connect-src 'self'; img-src 'self' blob: data:; font-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'" } } });
