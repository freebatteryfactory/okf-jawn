/** Pure and DOM tests are separate from real-host qualification. */

import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';
export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    include: ['tests/unit/**/*.test.{ts,tsx}'],
    setupFiles: ['tests/unit/setup.ts'],
    // Bun's worker pools break jsdom EventTarget setup on Windows.
    // The package.json test script runs Vitest under Node.
    pool: 'forks',
  },
});
