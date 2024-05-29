import { defineConfig } from 'vite'
import { nodePolyfills } from 'vite-plugin-node-polyfills'
import EnvironmentPlugin from 'vite-plugin-environment';

// https://vitejs.dev/config/
export default defineConfig({
  base: '/admin/',
  plugins: [
    nodePolyfills(),
    EnvironmentPlugin('all')
  ],
})
