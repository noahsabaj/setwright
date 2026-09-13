import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { resolve, basename } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const read = (path) => readFileSync(resolve(root, path), "utf8");
const json = (path) => JSON.parse(read(path));
const semver = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?$/;
const [command = "check", argument, destination] = process.argv.slice(2);

function localPath(path) {
  const candidate = resolve(root, path);
  if (!candidate.startsWith(root)) throw new Error("Release files must stay inside the repository.");
  return candidate;
}

function version() {
  const value = json("package.json").version;
  const cargo = read("src-tauri/Cargo.toml").match(/^version = "([^"]+)"/m)?.[1];
  const locked = read("src-tauri/Cargo.lock").match(/name = "setwright-desktop"\r?\nversion = "([^"]+)"/)?.[1];
  if (!semver.test(value) || cargo !== value || locked !== value || json("src-tauri/tauri.conf.json").version !== value) throw new Error("Package, Cargo, lockfile, and Tauri must share a valid SemVer version.");
  return value;
}
function notes(value) {
  const entry = read("CHANGELOG.md").split(/^## /m).slice(1).find((part) => part.startsWith(`${value} —`) || part.startsWith(`${value}\n`));
  if (!entry) throw new Error(`Add release notes for ${value} to CHANGELOG.md first.`);
  return entry.slice(entry.indexOf("\n") + 1).trim();
}
if (command === "prepare") {
  if (!semver.test(argument ?? "")) throw new Error("Usage: node scripts/release.mjs prepare MAJOR.MINOR.PATCH[-prerelease]");
  const current = version();
  if (compareVersions(argument, current) <= 0) throw new Error("The next version must be greater than the current version.");
  for (const path of ["package.json", "src-tauri/tauri.conf.json"]) {
    const data = json(path); data.version = argument;
    writeFileSync(resolve(root, path), JSON.stringify(data, null, 2) + "\n");
  }
  writeFileSync(resolve(root, "src-tauri/Cargo.toml"), read("src-tauri/Cargo.toml").replace(/^version = "[^"]+"/m, `version = "${argument}"`));
  writeFileSync(resolve(root, "src-tauri/Cargo.lock"), read("src-tauri/Cargo.lock").replace(/(name = "setwright-desktop"\r?\nversion = ")[^"]+/, `$1${argument}`));
  console.log(`Prepared ${argument}. Add its changelog entry, run checks, then commit and tag v${argument}.`);
} else if (command === "check") {
  const value = version(); notes(value);
  if (argument && argument !== `v${value}`) throw new Error(`Tag must be v${value}.`);
  console.log(`Version and changelog verified: ${value}`);
} else if (command === "notes") {
  const output = notes(version());
  if (argument) writeFileSync(localPath(argument), output + "\n"); else console.log(output);
} else if (command === "manifest") {
  const value = version();
  if (!argument || !destination) throw new Error("Usage: manifest SIGNED_INSTALLER OUTPUT_JSON");
  const installerPath = localPath(argument), signaturePath = localPath(`${argument}.sig`), outputPath = localPath(destination);
  if (!existsSync(installerPath) || !existsSync(signaturePath)) throw new Error("Installer and signature must exist.");
  if (basename(argument) !== `Setwright_${value}_x64-setup.exe`) throw new Error("The installer filename must match the current Windows x64 version.");
  const signature = readFileSync(signaturePath, "utf8").trim();
  if (!signature) throw new Error("Installer signature is missing.");
  const url = `https://github.com/noahsabaj/setwright/releases/download/v${value}/${encodeURIComponent(basename(argument))}`;
  writeFileSync(outputPath, JSON.stringify({ version: value, notes: notes(value), pub_date: new Date().toISOString(), platforms: { "windows-x86_64": { url, signature } } }, null, 2) + "\n");
} else { throw new Error(`Unknown command: ${command}`); }

function compareVersions(left, right) {
  const split = (value) => { const dash = value.indexOf("-"); return dash < 0 ? [value, null] : [value.slice(0, dash), value.slice(dash + 1)]; };
  const [leftCore, leftPre] = split(left);
  const [rightCore, rightPre] = split(right);
  const a = leftCore.split(".").map(BigInt), b = rightCore.split(".").map(BigInt);
  for (let i = 0; i < 3; i++) { if (a[i] !== b[i]) return a[i] > b[i] ? 1 : -1; }
  if (leftPre === rightPre) return 0;
  if (leftPre === null) return 1;
  if (rightPre === null) return -1;
  const x = leftPre.split("."), y = rightPre.split(".");
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    if (x[i] === undefined) return -1;
    if (y[i] === undefined) return 1;
    if (x[i] === y[i]) continue;
    const nx = /^\d+$/.test(x[i]), ny = /^\d+$/.test(y[i]);
    if (nx && ny) return BigInt(x[i]) > BigInt(y[i]) ? 1 : -1;
    if (nx !== ny) return nx ? -1 : 1;
    return x[i] > y[i] ? 1 : -1;
  }
  return 0;
}
