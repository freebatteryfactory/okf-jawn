/** Execute the real Rust and Hey API generators; never synthesize their expected output. */
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { run } from './process.mjs';
import { exists, manifest, differences, replaceGenerated } from './files.mjs';

async function onePass(root, output) {
  await run('cargo', ['run', '--frozen', '--package', 'xtask', '--', 'generate', '--out', output], { cwd: root });
  await run('node', ['scripts/generate-catalog.mjs'], { cwd: join(root, 'ui'), env: { OKF_CATALOG_OUT: join(output, 'api', 'presentation') } });
  await mkdir(join(output, 'client'), { recursive: true });
  await run('pnpm', ['exec', 'openapi-ts', '--file', 'openapi-ts.config.ts'], {
    cwd: join(root, 'ui'), env: { OKF_OPENAPI: join(output, 'api', 'openapi.yaml'), OKF_CLIENT_OUT: join(output, 'client') }
  });
}

export async function generate(root, check = false) {
  for (const name of ['Cargo.lock', 'pnpm-lock.yaml']) {
    if (!await exists(join(root, name))) throw new Error(`Missing resolved ${name}. Run bootstrap; do not fabricate lockfiles.`);
  }
  const scratch = await mkdtemp(join(tmpdir(), 'okf-jawn-generate-'));
  try {
    const first = join(scratch, 'first'); const second = join(scratch, 'second');
    await onePass(root, first); await onePass(root, second);
    const delta = differences(await manifest(first), await manifest(second));
    if (delta.length) throw new Error(`Generators are not deterministic:\n${delta.join('\n')}`);
    for (const [from, to] of [['api', 'api'], ['client', 'ui/src/api/generated'], ['cli', 'generated/cli']]) {
      const destination = join(root, to);
      if (check) {
        if (!await exists(destination)) throw new Error(`Generated output missing: ${to}`);
        const changes = differences(await manifest(join(first, from)), await manifest(destination));
        if (changes.length) throw new Error(`Generated drift in ${to}:\n${changes.join('\n')}`);
      } else await replaceGenerated(join(first, from), destination, root);
    }
  } finally { await rm(scratch, { recursive: true, force: true }); }
}
