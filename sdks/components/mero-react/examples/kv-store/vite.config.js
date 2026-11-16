import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { resolve } from 'path';

// `@calimero-network/mero-react` resolves to this package's own source, so the
// example builds from the tree it sits in (as Storybook does); its
// `@calimero-network/mero-js` is the workspace's mero-js (build that first).
const meroReactSrc = resolve(__dirname, '../../src/index.ts');

export default defineConfig({
  base: './',
  root: __dirname,
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
  resolve: {
    alias: {
      '@calimero-network/mero-react': meroReactSrc,
    },
  },
  optimizeDeps: {
    exclude: ['@calimero-network/mero-react'],
  },
  plugins: [react()],
});
