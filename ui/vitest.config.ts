/** Pure and DOM tests are separate from real-host qualification. */

import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';
export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'happy-dom',
    include: ['tests/unit/**/*.test.{ts,tsx}', 'src/**/*.test.{ts,tsx}'],
    setupFiles: ['tests/unit/setup.ts'],
    pool: 'forks',
    // The same NODE_ENV as CI whatever the caller's shell exports: under any other value the
    // node build of TanStack Router decides it is a server and never mounts its client router.
    env: { NODE_ENV: 'test' },
  },
  server: {
    fs: {
      allow: ['..'],
    },
  },
});
