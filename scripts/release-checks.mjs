import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, copyFileSync, writeFileSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

function fixture(t, version = "0.2.0") {
  const root = mkdtempSync(join(tmpdir(), "setwright-release-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, "scripts")); mkdirSync(join(root, "src-tauri"));
  copyFileSync(new URL("./release.mjs", import.meta.url), join(root, "scripts/release.mjs"));
  for (const path of ["package.json", "src-tauri/tauri.conf.json"]) writeFileSync(join(root, path), JSON.stringify({ version }));
  writeFileSync(join(root, "src-tauri/Cargo.toml"), `[package]\nname = "setwright-desktop"\nversion = "${version}"\n`);
  writeFileSync(join(root, "src-tauri/Cargo.lock"), `[[package]]\nname = "setwright-desktop"\nversion = "${version}"\n`);
  writeFileSync(join(root, "CHANGELOG.md"), `# Changelog\n\n## ${version} — 2026-09-13\n\n- Actual release notes.\n`);
  return { root, run: (...args) => spawnSync(process.execPath, [join(root, "scripts/release.mjs"), ...args], { encoding: "utf8" }) };
}

test("version preparation rejects malformed, equal and older versions without changing files", (t) => {
  const { root, run } = fixture(t);
  for (const version of ["0.1.9", "0.2.0", "0.2.0-beta.1", "01.2.3", "0.3.0-01", "v0.3.0"]) assert.notEqual(run("prepare", version).status, 0);
  assert.equal(JSON.parse(readFileSync(join(root, "package.json"))).version, "0.2.0");
  assert.equal(run("prepare", "0.2.1").status, 0);
  assert.notEqual(run("check").status, 0, "new versions need their own changelog");
  writeFileSync(join(root, "CHANGELOG.md"), "## 0.2.1 — today\n- A fix.\n");
  assert.equal(run("check", "v0.2.1").status, 0);
  assert.notEqual(run("check", "v0.2.0").status, 0);
});

test("prerelease ordering follows numeric SemVer identifiers", (t) => {
  const { run } = fixture(t, "0.3.0-beta.9");
  assert.notEqual(run("prepare", "0.3.0-beta.8").status, 0);
  assert.equal(run("prepare", "0.3.0-beta.10").status, 0);
  assert.equal(run("prepare", "0.3.0").status, 0);
});

test("manifest requires matching installer and signature and includes this release's notes", (t) => {
  const { root, run } = fixture(t);
  const installer = join(root, "Setwright_0.2.0_x64-setup.exe"), manifest = join(root, "latest.json");
  writeFileSync(installer, "test installer bytes");
  assert.notEqual(run("manifest", installer, manifest).status, 0);
  writeFileSync(`${installer}.sig`, "fixture signature");
  assert.equal(run("manifest", installer, manifest).status, 0);
  const data = JSON.parse(readFileSync(manifest));
  assert.equal(data.version, "0.2.0");
  assert.equal(data.notes, "- Actual release notes.");
  assert.equal(data.platforms["windows-x86_64"].signature, "fixture signature");
  assert.match(data.platforms["windows-x86_64"].url, /releases\/download\/v0.2.0\/Setwright_0.2.0_x64-setup.exe$/);
  writeFileSync(join(root, "src-tauri/Cargo.lock"), 'name = "setwright-desktop"\nversion = "0.1.0"');
  assert.notEqual(run("check").status, 0);
});
