import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(() => cleanup());

class TestResizeObserver implements ResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

Object.defineProperty(window, "ResizeObserver", { value: TestResizeObserver, writable: true });
Object.defineProperty(globalThis, "ResizeObserver", { value: TestResizeObserver, writable: true });

Object.defineProperty(window, "matchMedia", {
  value: (query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => undefined,
    removeListener: () => undefined,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    dispatchEvent: () => false,
  }),
  writable: true,
});

if (document.elementFromPoint === undefined) {
  document.elementFromPoint = () => null;
}

HTMLElement.prototype.scrollIntoView = () => undefined;

Range.prototype.getClientRects = () => Object.assign([], { item: () => null });
Range.prototype.getBoundingClientRect = () => new DOMRect(0, 0, 0, 0);
