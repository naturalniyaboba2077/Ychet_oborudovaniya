import { defineConfig } from "vitest/config";
import path from "path";

const templateRoot = path.resolve(import.meta.dirname);

export default defineConfig({
  root: templateRoot,
  resolve: {
    alias: {
      "@": path.resolve(templateRoot, "src"),
      "@contracts": path.resolve(templateRoot, "contracts"),
      "@assets": path.resolve(templateRoot, "attached_assets"),
    },
  },
  test: {
    // jsdom, а не node: интерфейс до сих пор не проверялся ничем, и ошибки в
    // нём ловились глазами. Часть тестов — про экраны, им нужен браузерный
    // разбор разметки.
    environment: "jsdom",
    globals: true,
    include: [
      "api/**/*.test.ts",
      "api/**/*.spec.ts",
      "src/**/*.test.ts",
      "src/**/*.spec.ts",
      "src/**/*.test.tsx",
    ],
  },
});
