/** Generate clients and validators from the Rust-emitted contract; never hand-edit output. */
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { defineConfig } from '@hey-api/openapi-ts';

const input = process.env['OKF_OPENAPI'] ?? '../api/openapi.json';
const metadata: unknown = JSON.parse(readFileSync(join(dirname(input), 'operations.json'), 'utf8'));
if (!Array.isArray(metadata)) throw new Error('Generated operation metadata must be an array');
const readPaths = new Set<string>();
for (const value of metadata) {
  if (typeof value !== 'object' || value === null || !('path' in value) || !('permission' in value)) {
    throw new Error('Invalid generated operation metadata');
  }
  if (typeof value.path !== 'string') throw new Error('Operation path must be a string');
  if (value.permission === 'read') readPaths.add(value.path);
}

export default defineConfig({
  input,
  output: process.env['OKF_CLIENT_OUT'] ?? 'src/api/generated',
  parser: {
    hooks: {
      operations: {
        getKind: (operation) => readPaths.has(operation.path) ? ['query'] : undefined,
      },
    },
  },
  plugins: [
    '@hey-api/typescript',
    '@hey-api/client-fetch',
    '@hey-api/sdk',
    { name: 'zod', requests: true, responses: true },
    { name: '@tanstack/react-query', queryOptions: true, mutationOptions: true },
  ],
});
