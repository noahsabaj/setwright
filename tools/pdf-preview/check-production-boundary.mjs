import { readFileSync, readdirSync, statSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join, resolve } from "node:path";

const forbidden = [
  "@wdio/tauri-plugin",
  "pdf-preview-e2e-harness",
  "tauri-plugin-wdio",
  "tauri-plugin-wdio-webdriver",
  "TAURI_WEBDRIVER_PORT",
  "wdio-webdriver:default",
  "wdio:default",
  "wdioTauri",
];

function filesBelow(root) {
  const files = [];
  for (const entry of readdirSync(root)) {
    const path = join(root, entry);
    if (statSync(path).isDirectory()) files.push(...filesBelow(path));
    else files.push(path);
  }
  return files;
}

function assertBytesExclude(path, bytes) {
  for (const marker of forbidden) {
    if (bytes.includes(Buffer.from(marker))) {
      throw new Error(`${path} contains forbidden E2E marker ${marker}.`);
    }
  }
}

const productionConfigPath = resolve("src-tauri/tauri.conf.json");
const productionConfig = JSON.parse(readFileSync(productionConfigPath, "utf8"));
const capabilities = productionConfig.app?.security?.capabilities;
if (productionConfig.app?.withGlobalTauri !== false) {
  throw new Error("Production must keep app.withGlobalTauri disabled.");
}
if (JSON.stringify(capabilities) !== JSON.stringify(["main-window"])) {
  throw new Error("Production must explicitly include only the main-window capability.");
}

const distRoot = resolve("dist");
for (const file of filesBelow(distRoot)) assertBytesExclude(file, readFileSync(file));

const cargo = spawnSync(
  "cargo",
  ["tree", "--manifest-path", "src-tauri/Cargo.toml", "--locked", "--no-default-features", "--features", "custom-protocol"],
  { encoding: "utf8" },
);
if (cargo.status !== 0) {
  throw new Error(`cargo tree failed:\n${cargo.stderr}`);
}
assertBytesExclude("normal Cargo dependency graph", Buffer.from(cargo.stdout));

if (process.argv.includes("--require-binary")) {
  const binaryPath = resolve(
    "src-tauri",
    "target",
    "release",
    process.platform === "win32" ? "setwright-desktop.exe" : "setwright-desktop",
  );
  assertBytesExclude(binaryPath, readFileSync(binaryPath));
}

process.stdout.write("Production PDF boundary contains no E2E plugin, capability, port, or harness markers.\n");
