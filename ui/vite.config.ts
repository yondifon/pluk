import { defineConfig } from "vite";
import path from "path";

export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  preview: {
    port: 1420,
    strictPort: true,
  },
  build: {
    rollupOptions: {
      input: {
        main: path.resolve(__dirname, "index.html"),
        // The confirm window is its own document: it opens on its own, and
        // must not wait for the app shell to boot.
        confirm: path.resolve(__dirname, "confirm.html"),
      },
    },
  },
  resolve: {
    alias: {
      "bun:test": path.resolve(__dirname, "src/test-shim.ts"),
    },
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts"],
  },
});
