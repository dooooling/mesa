import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      "/api": {
        target: "http://127.0.0.1:8132",
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: "dist",
    sourcemap: false,
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    // M7：重交互测试（user.click + 轮询 + SSE mock）在高并发 worker 下时序
    // flaky；threads 池 2 并发是稳定与速度的折中（全量约 1 分钟）。
    poolOptions: {
      threads: { minThreads: 1, maxThreads: 2 },
    },
  },
});
