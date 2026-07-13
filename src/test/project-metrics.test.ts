import { describe, expect, it } from "vitest";
import { cloneDemoProject } from "../lib/mock-project";
import { deriveProjectMetrics } from "../lib/project-metrics";

describe("canonical project metrics", () => {
  it("derives outline order, bibliography entries, labels, and visual words from project source", () => {
    const metrics = deriveProjectMetrics(cloneDemoProject());

    expect(metrics.outline.map((item) => item.label)).toEqual(["Abstract", "Introduction", "Method", "Results"]);
    expect(metrics.outlineStatus).toBe("available");
    expect(metrics.referenceCount).toBe(1);
    expect(metrics.labelCount).toBe(2);
    expect(metrics.visualWordCount).not.toBeNull();
    expect(metrics.visualWordCount).toBeGreaterThan(20);
  });

  it("marks dynamic outlines partial and does not count commented labels or BibTeX metadata declarations", () => {
    const project = cloneDemoProject();
    project.files = project.files.map((file) => {
      if (file.id === project.mainFile) {
        return {
          ...file,
          content: "\\begin{document}\n\\section{Real heading}\n% \\section{Commented heading}\n\\label{real}\n% \\label{commented}\n\\input{\\dynamic}\nBody words.\n\\end{document}\n",
        };
      }
      if (file.kind === "bib") {
        return {
          ...file,
          content: "@string{name = {Setwright}}\n@comment{not an entry @article{nested, title={No}}}\n@article{real, title={Real}}\n",
        };
      }
      return file;
    });

    const metrics = deriveProjectMetrics(project);

    expect(metrics.outline.map((item) => item.label)).toEqual(["Real heading"]);
    expect(metrics.outlineStatus).toBe("partial");
    expect(metrics.labelCount).toBe(1);
    expect(metrics.referenceCount).toBe(1);
  });

  it("reports unavailable values instead of inventing counts for non-UTF-8 source", () => {
    const project = cloneDemoProject();
    project.files = project.files.map((file) => file.id === project.mainFile
      ? { ...file, encoding: "nonUtf8" as const, content: null }
      : file.kind === "bib" ? { ...file, encoding: "nonUtf8" as const, content: null } : file);

    const metrics = deriveProjectMetrics(project);

    expect(metrics.outlineStatus).toBe("unavailable");
    expect(metrics.visualWordCount).toBeNull();
    expect(metrics.labelCount).toBeNull();
    expect(metrics.referenceCount).toBeNull();
  });
});
