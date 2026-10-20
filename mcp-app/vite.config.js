import { fileURLToPath, URL } from "node:url";
import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";
import { viteSingleFile } from "vite-plugin-singlefile";

// MCP App 的 UI 以单个自包含 HTML 产出，方便 htknow 作为 ui:// 资源内嵌返回。
export default defineConfig({
  plugins: [vue(), tailwindcss(), viteSingleFile()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
      // KnowledgeSpace 保留了 vue-router 用法；MCP App 内没有多页路由，用空实现替代。
      "vue-router": fileURLToPath(new URL("./src/platform/routerShim.js", import.meta.url)),
    },
  },
  build: {
    outDir: "dist",
    // singlefile 要求全部内联。
    assetsInlineLimit: 100 * 1024 * 1024,
  },
});
