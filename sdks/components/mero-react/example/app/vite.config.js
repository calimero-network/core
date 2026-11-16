import { defineConfig } from 'vite';
import { nodePolyfills } from 'vite-plugin-node-polyfills';
import react from '@vitejs/plugin-react';
import { resolve } from 'path';

// Resolve `@calimero-network/mero-react` to the sibling source folder so
// Vite imports `src/index.ts` directly (the same path Storybook uses).
// Benefits:
//   - No tsup rebuild step in the loop — edits to mero-react/src/* hot-reload
//     instantly, same as edits inside example/app/src/*.
//   - CSS imports are processed by Vite's CSS plugin and injected as <style>
//     tags in dev. No dependency on tsup's `injectStyle` runtime IIFE, so
//     styles can never go missing because of a stale dist.
//   - Storybook and the example app render the components from the same
//     source path, eliminating dist-vs-source asymmetry.
const meroReactSrc = resolve(__dirname, '../../src/index.ts');

// https://vitejs.dev/config/
export default defineConfig({
  base: '/',
  build: {
    outDir: 'dist',
    rollupOptions: {
      input: {
        main: resolve(__dirname, 'index.html'),
        404: resolve(__dirname, 'public/404.html'),
      },
    },
  },
  resolve: {
    alias: {
      '@calimero-network/mero-react': meroReactSrc,
    },
  },
  optimizeDeps: {
    // Don't pre-bundle the aliased local package (the alias source uses
    // ESM + CSS imports that Vite handles natively per file).
    exclude: ['@calimero-network/mero-react'],
  },
  plugins: [nodePolyfills(), react()],
});
