export function isApplePlatform(): boolean {
  return /Mac|iPhone|iPad|iPod/i.test(navigator.platform || navigator.userAgent);
}

export function shortcutLabel(key: string): string {
  return `${isApplePlatform() ? "⌘" : "Ctrl+"}${key}`;
}

export function hasPrimaryModifier(event: KeyboardEvent): boolean {
  return isApplePlatform() ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
}
