import { defineConfig, type Plugin } from "vite";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";
import { resolve } from "path";

const host = process.env.TAURI_DEV_HOST;

/**
 * Preload the two Latin font files the startup screen needs, so they are
 * already in flight when the CSS asks for them.
 *
 * Build only, and derived from the bundle: the tags then carry exactly the
 * hashed URLs the emitted CSS references. A hand-written tag in index.html
 * cannot do that in dev, where Vite rewrites the CSS `url()`s to the package's
 * real path under `node_modules/.pnpm` and the preloaded copy goes unused.
 */
function preloadStartupFonts(): Plugin {
  const startupFont = /(?:^|\/)(?:outfit|syne)-latin-wght-normal-[^/]+\.woff2$/;
  let base = "/";
  return {
    name: "preload-startup-fonts",
    apply: "build",
    configResolved(config) {
      base = config.base;
    },
    transformIndexHtml: {
      order: "post",
      handler(_html, ctx) {
        return Object.keys(ctx.bundle ?? {})
          .filter((fileName) => startupFont.test(fileName))
          .sort()
          .map((fileName) => ({
            tag: "link",
            attrs: {
              rel: "preload",
              href: base + fileName,
              as: "font",
              type: "font/woff2",
              crossorigin: true,
            },
            injectTo: "head" as const,
          }));
      },
    },
  };
}

export default defineConfig(async () => ({
  plugins: [vue(), tailwindcss(), preloadStartupFonts()],
  resolve: {
    alias: {
      "@": resolve(__dirname, "src"),
    },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
}));
