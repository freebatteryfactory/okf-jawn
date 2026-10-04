/** Pure and DOM tests are separate from real-host qualification. */
import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
export default defineConfig({ plugins: [react()], test: { environment: 'jsdom', include: ['tests/unit/**/*.test.{ts,tsx}'] } });
