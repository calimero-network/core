import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { resolve } from 'path';

export default defineConfig({
  plugins: [react()],
  base: '/public/',
  build: {
    // Generate a single HTML file with inlined assets where possible
    assetsInlineLimit: 100000, // 100kb
    rollupOptions: {
      output: {
        manualChunks: {
          vendor: [
            'react',
            'react-dom',
            'react-router-dom',
            '@near-wallet-selector/core',
            '@near-wallet-selector/my-near-wallet'
          ],
        },
      },
    },
    chunkSizeWarningLimit: 1000, // Increase warning limit to 1000kb
  },
  resolve: {
    alias: {
      buffer: 'buffer'
    },
  },
  define: {
    'process.env': {},
    global: 'globalThis',
  },
  optimizeDeps: {
    include: ['buffer'],
  }
});
