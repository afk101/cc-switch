import path from "node:path";
import { configDefaults, defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./tests/setupGlobals.ts", "./tests/setupTests.ts"],
    globals: true,
    // scripts 目录中的测试使用 Node 原生测试运行器，避免 Vitest 重复收集后误报“没有测试套件”。
    exclude: [...configDefaults.exclude, "scripts/**/*.test.js"],
    coverage: {
      reporter: ["text", "lcov"],
    },
  },
});
