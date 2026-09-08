import { defineConfig } from "vite";

// Tauri serves the built files itself, so nothing here needs a base path.
export default defineConfig({
  clearScreen: false,
  server: { port: 5183, strictPort: true },
  build: { target: "es2022", outDir: "dist", emptyOutDir: true },
});
