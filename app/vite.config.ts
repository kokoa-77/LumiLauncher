import { fileURLToPath, URL } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  // 让 Rust 的编译报错不被清屏冲掉
  clearScreen: false,
  server: {
    // Tauri 要求固定端口，被占用时直接报错
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    // WebView2 是常青的 Chromium，可以直接用新语法
    target: "es2023",
    // 3D 皮肤预览（skinview3d，约 520 kB）只在打开皮肤页时按需加载，不算首屏；
    // 首屏体积由 scripts/check-bundle.mjs 单独把关
    chunkSizeWarningLimit: 600,
  },
});
