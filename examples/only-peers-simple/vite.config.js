import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

export default defineConfig({
  plugins: [react()],
  resolve: {
    fallback: {
      fs: false,
      path: path.resolve(__dirname, 'node_modules/path-browserify'),
    },
  },
});