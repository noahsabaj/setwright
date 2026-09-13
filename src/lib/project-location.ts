/** Adapt a native main-file selection to the directory-based project commands. */
export function selectedProjectLocation(selectedPath: string): { rootPath: string; mainFile: string } {
  const windowsPath = /^[a-z]:[\\/]/i.test(selectedPath) || selectedPath.startsWith("\\\\");
  const separator = windowsPath
    ? Math.max(selectedPath.lastIndexOf("\\"), selectedPath.lastIndexOf("/"))
    : selectedPath.lastIndexOf("/");
  const mainFile = selectedPath.slice(separator + 1);
  if (separator < 0 || mainFile === "" || mainFile === "." || mainFile === "..") {
    throw new Error("Choose the paper’s main .tex file inside its project folder.");
  }

  const parentWithSeparator = selectedPath.slice(0, separator + 1);
  const isFilesystemRoot = separator === 0
    || /^[a-z]:[\\/]$/i.test(parentWithSeparator)
    || /^\\\\\?\\[a-z]:\\$/i.test(parentWithSeparator);
  return {
    rootPath: isFilesystemRoot ? parentWithSeparator : selectedPath.slice(0, separator),
    mainFile,
  };
}
