import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import { resolve } from "node:path";

const cleanRemoveScrollBarCss = {
  name: "clean-remove-scroll-bar-css",
  enforce: "pre" as const,
  transform(code: string, id: string) {
    if (!id.includes("/react-remove-scroll-bar/")) {
      return null;
    }

    // The dependency embeds whitespace-only CSS lines in a template literal.
    // Keep generated assets clean without changing the rendered stylesheet.
    return { code: code.replaceAll("\\n  \\n", "\\n\\n"), map: null };
  },
};

export default defineConfig({
  plugins: [react(), cleanRemoveScrollBarCss],
  base: "/",
  build: {
    outDir: resolve(__dirname, "../crates/client/src/webui/dist"),
    emptyOutDir: true,
    sourcemap: false,
    cssCodeSplit: false,
    rollupOptions: {
      output: {
        entryFileNames: "assets/app.js",
        chunkFileNames: "assets/[name].js",
        assetFileNames: (assetInfo) =>
          assetInfo.name?.endsWith(".css") ? "assets/app.css" : "assets/[name][extname]",
      },
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    css: true,
  },
});
