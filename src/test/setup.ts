import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { Buffer } from "node:buffer";
import { webcrypto } from "node:crypto";
import { afterEach } from "vitest";

afterEach(() => cleanup());

// Node 22.13 exposes WebCrypto from the host realm while Vitest's jsdom
// environment supplies BufferSource objects from an isolated realm. Copy the
// input into a Node-owned Buffer in tests so the host WebCrypto brand check sees
// the same bytes the browser implementation receives in production.
const browserCrypto = globalThis.crypto;
const testSubtleCrypto = {
  digest(algorithm: AlgorithmIdentifier, data: BufferSource): Promise<ArrayBuffer> {
    const input = ArrayBuffer.isView(data)
      ? Buffer.from(data.buffer, data.byteOffset, data.byteLength)
      : Buffer.from(data);
    return webcrypto.subtle.digest(algorithm, input);
  },
} as SubtleCrypto;
const testCrypto = {
  subtle: testSubtleCrypto,
  getRandomValues: browserCrypto.getRandomValues.bind(browserCrypto),
  randomUUID: browserCrypto.randomUUID.bind(browserCrypto),
} satisfies Crypto;

Object.defineProperty(globalThis, "crypto", { value: testCrypto, configurable: true });

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
