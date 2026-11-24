import { defineConfig } from '@playwright/test';

// One cluster for the whole run (global-setup.mjs), so tests share a worker and run in order.
export default defineConfig({
  testDir: './tests',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 180_000,
  expect: { timeout: 20_000 },
  reporter: [['list']],
  globalSetup: './global-setup.mjs',
  use: { headless: true, viewport: { width: 520, height: 900 }, deviceScaleFactor: 2 },
});
