import { defineConfig } from "vite";

// Tauri serves these files itself from inside the application, so nothing here
// needs a base path.
export default defineConfig({
  clearScreen: false,
  server: { port: 5183, strictPort: true },
  build: {
    target: "es2022",
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: {
      output: {
        // Stable names, not content hashes. There is no cache to bust: the
        // files ship inside the application. Hashed names are actively harmful
        // here, because the webview caches the page it was given, and after a
        // rebuild that cached page points at files that no longer exist. The
        // window then loads nothing at all.
        entryFileNames: "assets/studio.js",
        chunkFileNames: "assets/[name].js",
        assetFileNames: "assets/studio.[ext]",
      },
    },
  },
});
