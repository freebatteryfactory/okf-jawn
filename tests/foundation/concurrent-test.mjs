/**
 * `test` for foundation tests that each own their disposable repositories and share nothing:
 * the same `(name, body)` call as node:test, run alongside its siblings by Bun, with the
 * `t.after` hook fixture-repo.mjs registers its cleanup on. A body that throws fails only itself.
 */
import { test as bunTest } from 'bun:test';

export default function test(name, body) {
  bunTest.concurrent(name, async () => {
    const cleanups = [];
    try {
      await body({ after: cleanup => { cleanups.push(cleanup); } });
    } finally {
      // Last registered first, as node:test does; one failing cleanup must not skip the rest.
      for (const cleanup of cleanups.reverse()) await Promise.resolve().then(cleanup).catch(() => {});
    }
  });
}
