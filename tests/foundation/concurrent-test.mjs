/**
 * `test` for foundation tests that each own their disposable repositories and share nothing:
 * the same `(name, body)` call as node:test, run alongside its siblings by Bun, with the
 * `t.after` hook fixture-repo.mjs registers its cleanup on. A body that throws fails only itself.
 *
 * At most `limit` bodies run at once. Bun starts a test's timeout when the test is registered to
 * run, so a test waiting for a free place would spend its budget waiting; Bun's own timeout is
 * therefore a guard far outside the budget, and the budget of `bun test --timeout 60000` (what
 * `check-offline` passes) is counted here, from the moment the body starts.
 */
import { availableParallelism } from 'node:os';
import { test as bunTest } from 'bun:test';

/** How long one body may run, as `check-offline` allows every test. */
export const BODY_BUDGET_MS = 60_000;
/** Bun's own timeout for a registered test: outside every budget, so only a stuck queue reaches it. */
const GUARD_MS = 30 * 60_000;
/** Bodies running at once: the cores less two, between 2 and 8; more only slows each of them, and a body that starves hits its budget. */
const limit = Math.max(2, Math.min(8, availableParallelism() - 2));

let running = 0;
const waiting = [];

async function place() {
  if (running >= limit) await new Promise(resolve => waiting.push(resolve));
  else running += 1;
}

function release() {
  const next = waiting.shift();
  if (next) next();
  else running -= 1;
}

function within(promise, name) {
  let timer;
  const expired = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`this test timed out after ${BODY_BUDGET_MS}ms of running: ${name}`)), BODY_BUDGET_MS);
  });
  return Promise.race([promise, expired]).finally(() => clearTimeout(timer));
}

export default function test(name, body) {
  bunTest.concurrent(name, async () => {
    const cleanups = [];
    await place();
    try {
      await within(Promise.resolve(body({ after: cleanup => { cleanups.push(cleanup); } })), name);
    } finally {
      // Last registered first, as node:test does; one failing cleanup must not skip the rest.
      for (const cleanup of cleanups.reverse()) await Promise.resolve().then(cleanup).catch(() => {});
      release();
    }
  }, GUARD_MS);
}
