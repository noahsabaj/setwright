import { createHash } from "node:crypto";
import { createReadStream, readFileSync, statSync } from "node:fs";
import { resolve } from "node:path";

const manifest = JSON.parse(readFileSync(resolve("config/chrome-for-testing.json"), "utf8"));
const args = new Map();
const allowedArguments = new Set(["--online", "--chrome-archive", "--chromedriver-archive"]);
for (let index = 2; index < process.argv.length; index += 1) {
  const name = process.argv[index];
  if (!name.startsWith("--")) throw new Error(`Unexpected argument ${name}.`);
  if (!allowedArguments.has(name)) throw new Error(`Unknown argument ${name}.`);
  if (args.has(name)) throw new Error(`Argument ${name} was supplied more than once.`);
  if (name === "--online") {
    args.set(name, true);
    continue;
  }
  const value = process.argv[index + 1];
  if (value === undefined) throw new Error(`${name} requires a value.`);
  args.set(name, value);
  index += 1;
}

const assets = [
  ["chrome", manifest.chrome, args.get("--chrome-archive")],
  ["chromedriver", manifest.chromedriver, args.get("--chromedriver-archive")],
];
const archiveCount = assets.filter(([, , archive]) => typeof archive === "string").length;
if (archiveCount !== 0 && archiveCount !== assets.length) {
  throw new Error("Supply both --chrome-archive and --chromedriver-archive together.");
}

function gcsObjectName(url) {
  const parsed = new URL(url);
  if (parsed.protocol !== "https:" || parsed.hostname !== "storage.googleapis.com") {
    throw new Error(`${url} is not an HTTPS Google Cloud Storage URL.`);
  }
  const prefix = "/chrome-for-testing-public/";
  if (!parsed.pathname.startsWith(prefix)) {
    throw new Error(`${url} is outside the official chrome-for-testing-public bucket.`);
  }
  return decodeURIComponent(parsed.pathname.slice(prefix.length));
}

async function sha256(path) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}

async function verifyUpstreamMetadata(name, asset) {
  const objectName = gcsObjectName(asset.url);
  const endpoint = `https://storage.googleapis.com/storage/v1/b/chrome-for-testing-public/o/${encodeURIComponent(objectName)}?fields=bucket,name,generation,size,md5Hash,crc32c`;
  const response = await fetch(endpoint);
  if (!response.ok) throw new Error(`${name} metadata request failed with HTTP ${String(response.status)}.`);
  const metadata = await response.json();
  const expected = {
    bucket: "chrome-for-testing-public",
    name: objectName,
    generation: asset.gcs.generation,
    size: String(asset.gcs.size),
    md5Hash: asset.gcs.md5Base64,
    crc32c: asset.gcs.crc32cBase64,
  };
  for (const [field, value] of Object.entries(expected)) {
    if (metadata[field] !== value) {
      throw new Error(`${name} upstream ${field} changed: expected ${value}, received ${String(metadata[field])}.`);
    }
  }
}

async function verifyArchive(name, asset, archive) {
  const path = resolve(archive);
  const size = statSync(path).size;
  if (size !== asset.gcs.size) {
    throw new Error(`${name} archive size mismatch: expected ${String(asset.gcs.size)}, received ${String(size)}.`);
  }
  const digest = await sha256(path);
  if (digest !== asset.sha256) {
    throw new Error(`${name} SHA-256 mismatch: expected ${asset.sha256}, received ${digest}.`);
  }
}

for (const [name, asset, archive] of assets) {
  gcsObjectName(asset.url);
  if (args.has("--online")) await verifyUpstreamMetadata(name, asset);
  if (typeof archive === "string") await verifyArchive(name, asset, archive);
}

if (!args.has("--online") && archiveCount === 0) {
  throw new Error("Specify --online and/or both archive paths to perform a verification.");
}

process.stdout.write("Chrome for Testing provenance and archive verification passed.\n");
