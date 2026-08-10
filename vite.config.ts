import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
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
  build: {
    target: "es2021",
    minify: "esbuild",
    sourcemap: false,
    rollupOptions: {
      output: {
        // 拆分第三方库，避免单个 chunk 过大并提升缓存命中率
        manualChunks: {
          react: ["react", "react-dom"],
          markdown: ["marked", "dompurify"],
          highlight: ["highlight.js"],
        },
      },
    },
  },
});
