/** Browser acceptance targets an explicitly started service, never an automatic mock backend. */
import { defineConfig } from '@playwright/test';
export default defineConfig({ testDir: 'tests/e2e', use: { baseURL: process.env['OKF_TEST_URL'] ?? 'http://127.0.0.1:8787' }, retries: 0 });
