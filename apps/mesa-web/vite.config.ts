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
    // Windows 上「编辑工具 + 文件监听」会互相打架：部分工具写文件时先在源码目录
    // 建 `.<文件名>.<pid>.<uuid>.tmpdir/<文件名>.tmp` 再改名，而 chokidar 会去
    // 监听这个瞬时存在、且可能被锁定的临时文件，命中 EBUSY 后**整个 dev server
    // 崩溃**（实测连续三次，分别撞 .tsx / .test.tsx / .ts 的临时文件）。
    //
    // 这里只忽略该类临时目录：不改变正常源码的监听（`.tsx` 改动仍触发 HMR），
    // 也不切换到轮询模式（轮询会明显增加 CPU/延迟，属过度修复）。
    watch: {
      ignored: ["**/.*.tmpdir", "**/.*.tmpdir/**"],
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
