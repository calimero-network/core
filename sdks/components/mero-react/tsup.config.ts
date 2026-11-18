import { defineConfig } from 'tsup';

export default defineConfig({
  // Split so both entries share one MeroContext (and its chunk) in ESM and CommonJS alike.
  entry: { index: 'src/index.ts', 'node-shield': 'src/node-shield/index.ts' },
  splitting: true,
  format: ['cjs', 'esm'],
  dts: true,
  sourcemap: true,
  clean: true,
  external: ['react', 'react-dom'],
  treeshake: true,
  minify: false,
  // Bundle styles.css into the JS output and inject it as a <style> tag at
  // runtime (on first import). Means consumers don't need a separate CSS
  // import — `import { ConnectButton } from '@calimero-network/mero-react'`
  // is enough for the styles to apply.
  injectStyle: true,
});
