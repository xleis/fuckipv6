import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import { resolve } from "node:path";

// The release gate asks the build artifact to be traceable to a commit. The value
// comes from the environment so a local rebuild stays byte-identical between
// commits: the checked-in bundle must not change just because HEAD moved.
function buildCommit(): string {
  const fromEnv = process.env.FUCKIPV6_BUILD_COMMIT?.trim();
  if (fromEnv) return fromEnv.slice(0, 12);
  return "unknown";
}

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
  define: {
    __FUCKIPV6_BUILD_COMMIT__: JSON.stringify(buildCommit()),
  },
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
