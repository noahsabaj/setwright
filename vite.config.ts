import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import platformSupport from "./config/platform-support.json";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: [
      `chrome${String(platformSupport.browser.chromiumMinimumMajor)}`,
      `safari${String(platformSupport.browser.safariMinimumMajor)}`,
    ],
    minify: process.env.TAURI_ENV_DEBUG ? false : "oxc",
    sourcemap: Boolean(process.env.TAURI_ENV_DEBUG),
  },
});
