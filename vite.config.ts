import { defineConfig } from "vite";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // The whole web part (the pages and what they load) is in `src-ui`, which is the root of the site: the pages
  // are at `/index.html`, `/device.html`... for the windows that open them.
  root: "src-ui",
  // The app has five pages: the main window, the floating toolbar, the debug console, a device's window
  // and the small window of a request from a file manager.
  build: {
    // Next to `package.json`, where `tauri.conf.json` expects it (the output is outside the root, so it is emptied).
    outDir: "../dist",
    emptyOutDir: true,
    rollupOptions: {
      input: {
        main: "index.html",
        toolbar: "toolbar.html",
        console: "console.html",
        device: "device.html",
        request: "request.html",
      },
    },
  },
  // 2. tauri expects a fixed port, fail if that port is not available
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
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
