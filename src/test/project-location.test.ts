import { describe, expect, it } from "vitest";
import { selectedProjectLocation } from "../lib/project-location";

describe("native main-file selection", () => {
  it.each([
    [String.raw`D:\Papers\Article\main.tex`, String.raw`D:\Papers\Article`, "main.tex"],
    [String.raw`C:\main.tex`, "C:\\", "main.tex"],
    ["D:/main.tex", "D:/", "main.tex"],
    ["D:/Papers/My paper/main.tex", "D:/Papers/My paper", "main.tex"],
    [String.raw`\\server\research\paper\main.tex`, String.raw`\\server\research\paper`, "main.tex"],
    [String.raw`\\server\research\main.tex`, String.raw`\\server\research`, "main.tex"],
    [String.raw`\\?\C:\main.tex`, "\\\\?\\C:\\", "main.tex"],
    ["/Users/ada/Papers/main.tex", "/Users/ada/Papers", "main.tex"],
    ["/main.tex", "/", "main.tex"],
    ["~/Papers/demo/main.tex", "~/Papers/demo", "main.tex"],
    ["/papers/α study/café.tex", "/papers/α study", "café.tex"],
    [String.raw`/papers/a\b.tex`, "/papers", String.raw`a\b.tex`],
  ])("converts %s into its parent directory and exact file name", (path, rootPath, mainFile) => {
    expect(selectedProjectLocation(path)).toEqual({ rootPath, mainFile });
  });

  it.each(["main.tex", "C:\\", "/papers/", "/papers/.", "/papers/.."])("rejects %s without a usable selected file", (path) => {
    expect(() => selectedProjectLocation(path)).toThrow(/main .tex file/);
  });
});
