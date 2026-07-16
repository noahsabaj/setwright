import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

interface RepresentativeManifest {
  sha256: string;
  byteLength: number;
}

describe("representative PDF fixture provenance", () => {
  it("binds the checked-in PDF and CI regeneration gate to one valid SHA-256", () => {
    const manifest = JSON.parse(
      readFileSync(resolve("test/fixtures/pdf/sample-project.manifest.json"), "utf8"),
    ) as RepresentativeManifest;
    const pdf = readFileSync(resolve("test/fixtures/pdf/sample-project.pdf"));
    const workflow = readFileSync(resolve(".github/workflows/ci.yml"), "utf8");

    expect(manifest.sha256).toMatch(/^[0-9a-f]{64}$/u);
    expect(pdf).toHaveLength(manifest.byteLength);
    expect(createHash("sha256").update(pdf).digest("hex")).toBe(manifest.sha256);
    expect(workflow).toContain(`${manifest.sha256}  main.pdf`);
  });
});
