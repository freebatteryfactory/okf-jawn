/// <reference types="vitest/globals" />
import { cleanup } from '@testing-library/react';
import { afterEach, beforeEach } from 'vitest';

// happy-dom has no worker: YAML is read by the reader itself, a turn later
// (`support/yaml-engine.ts`). Imported here, not at the top: a test file that replaces a module
// the reader uses (`vi.mock`) has done so by the time its first test starts.
beforeEach(async () => {
  const [{ setYamlEngine }, { inlineEngine }] = await Promise.all([
    import('../../src/lib/yaml-reading'),
    import('./support/yaml-engine'),
  ]);
  setYamlEngine(inlineEngine);
});

afterEach(() => {
  cleanup();
});
