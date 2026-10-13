/**
 * Browser acceptance targets an explicitly started service, never an automatic mock backend.
 * The workspace browser tests run in both Chromium and WebKit (SPEC section 2): one config, two
 * projects. A cold browser start on a shared, loaded machine has taken longer than the default
 * 5 s for a first page, so web-first assertions wait up to 15 s; no assertion is weakened.
 */
import { defineConfig, devices } from '@playwright/test';
export default defineConfig({
  testDir: 'tests/e2e',
  use: { baseURL: process.env['OKF_TEST_URL'] ?? 'http://127.0.0.1:7711' },
  retries: 0,
  expect: { timeout: 15_000 },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
    { name: 'webkit', use: { ...devices['Desktop Safari'] } },
  ],
});
