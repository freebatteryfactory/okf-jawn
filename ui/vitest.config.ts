/** Pure and DOM tests are separate from real-host qualification. */

import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';
export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'happy-dom',
    include: ['tests/unit/**/*.test.{ts,tsx}'],
    setupFiles: ['tests/unit/setup.ts'],
    pool: 'forks',
  },
  server: {
    fs: {
      allow: ['..'],
    },
  },
});
