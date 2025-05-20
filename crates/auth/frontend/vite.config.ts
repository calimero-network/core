import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { resolve } from 'path';

export default defineConfig({
  plugins: [react()],
  base: '/auth/login/',
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
      external: ['vm'],
    },
    chunkSizeWarningLimit: 1000, // Increase warning limit to 1000kb
  },
  resolve: {
    alias: {
      buffer: 'buffer',
      crypto: 'crypto-browserify',
      stream: 'stream-browserify',
      util: 'util',
      process: 'process/browser',
      vm: 'vm-browserify',
      'js-sha256': resolve(__dirname, 'src/utils/sha256.js'), // We'll create a safe replacement
    },
  },
  define: {
    'process.env': {},
    global: 'globalThis',
  },
  optimizeDeps: {
    include: ['buffer'],
    esbuildOptions: {
      define: {
        global: 'globalThis'
      }
    }
  }
});
