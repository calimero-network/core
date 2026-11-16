import { defineConfig } from 'vitest/config';

// Runs against a local merod 1.0 cluster (dev mode): set MEROD_BINARY and
// KV_STORE_WASM, or build both in a sibling `core` checkout.
export default defineConfig({
  test: {
    include: ['tests/cluster/**/*.test.ts'],
    testTimeout: 120000,
    hookTimeout: 120000,
    environment: 'node',
    fileParallelism: false,
  },
});
