import { defineConfig } from "vite";
import { mergeConfig } from "vite";

export default defineConfig({
  optimizeDeps: {
    include: [
      "@tiptap/react",
      "@tiptap/starter-kit",
      "@tiptap/extension-text-style",
      "@tiptap/extension-color",
      "@tiptap/extension-text-align",
      "@tiptap/extension-underline",
      "@tiptap/extension-link",
    ],
  },
  build: {
    commonjsOptions: {
      include: [/node_modules/],
    },
  },
  resolve: {
    alias: {
      // Ensure proper module resolution for Tiptap
      "@tiptap/react": "@tiptap/react/dist/index.js",
      "@tiptap/starter-kit": "@tiptap/starter-kit/dist/index.js",
      "@tiptap/extension-text-style":
        "@tiptap/extension-text-style/dist/index.js",
      "@tiptap/extension-color": "@tiptap/extension-color/dist/index.js",
      "@tiptap/extension-text-align":
        "@tiptap/extension-text-align/dist/index.js",
      "@tiptap/extension-underline":
        "@tiptap/extension-underline/dist/index.js",
      "@tiptap/extension-link": "@tiptap/extension-link/dist/index.js",
    },
  },
});
