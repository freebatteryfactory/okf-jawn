// @vitest-environment node
/**
 * The MCP App bundle is one production build whatever the caller's environment or the files that
 * happen to sit under ui/.
 *
 * Each case runs the real scripts/bundle-app.mjs (about 10 s per build on a developer machine), so
 * the cases carry an explicit timeout. Builds are serialized because the script writes
 * dist-apps/app.html relative to the ui directory.
 */

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it } from 'vitest';

const uiRoot = fileURLToPath(new URL('../..', import.meta.url));
const buildTimeoutMs = 180_000;
// Class words are assembled at run time so this file does not itself put them under ui/.
const outsideWords = [
  ['under', 'line'].join(''),
  ['line', 'through'].join('-'),
  ['ita', 'lic'].join(''),
].join(' ');
const insideWord = ['line', 'through'].join('-');
// Strings present only in React's development build (verified against a NODE_ENV=development bundle).
const developmentMarkers = ['Download the React DevTools', 'react-stack-top-frame'];

function bundleEnv(nodeEnv: string | undefined) {
  const env: Record<string, string | undefined> = { ...process.env };
  delete env.OKF_APP_OUT;
  if (nodeEnv === undefined) delete env.NODE_ENV;
  else env.NODE_ENV = nodeEnv;
  return env;
}

function runBundle(cwd: string, env: Record<string, string | undefined>) {
  const run = spawnSync('bun', [join(uiRoot, 'scripts', 'bundle-app.mjs')], {
    cwd,
    env,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  });
  if (run.status !== 0) throw new Error(`bundle-app failed: ${run.stderr}${run.error ?? ''}`);
}

/** Build the App into a temporary OKF_APP_OUT directory that is removed afterwards. */
function buildApp(nodeEnv: string | undefined) {
  const outDir = mkdtempSync(join(tmpdir(), 'okf-app-out-'));
  try {
    runBundle(uiRoot, { ...bundleEnv(nodeEnv), OKF_APP_OUT: outDir });
    const html = readFileSync(join(outDir, 'app.html'), 'utf8');
    const manifest = readFileSync(join(outDir, 'manifest.json'), 'utf8');
    return { html, manifest, sha256: createHash('sha256').update(html).digest('hex') };
  } finally {
    rmSync(outDir, { recursive: true, force: true });
  }
}

/** Build the workspace UI into a temporary directory and return its stylesheet text. */
function buildWorkspaceCss() {
  const outDir = mkdtempSync(join(tmpdir(), 'okf-ui-css-'));
  try {
    const run = spawnSync('bunx', ['vite', 'build', '--outDir', outDir, '--emptyOutDir'], {
      cwd: uiRoot,
      env: bundleEnv('production'),
      encoding: 'utf8',
      maxBuffer: 64 * 1024 * 1024,
    });
    if (run.status !== 0) throw new Error(`vite build failed: ${run.stderr}${run.error ?? ''}`);
    const assets = join(outDir, 'assets');
    const css = readdirSync(assets).filter((name) => name.endsWith('.css'));
    return css.map((name) => readFileSync(join(assets, name), 'utf8')).join('\n');
  } finally {
    rmSync(outDir, { recursive: true, force: true });
  }
}

describe('MCP App bundle output directory', () => {
  it(
    'writes app.html and manifest.json to OKF_APP_OUT, and to ./dist-apps of the working directory by default',
    () => {
      const override = buildApp('production');
      expect(JSON.parse(override.manifest).resources[0].sha256).toBe(override.sha256);
      const cwd = mkdtempSync(join(tmpdir(), 'okf-app-cwd-'));
      try {
        runBundle(cwd, bundleEnv('production'));
        const html = readFileSync(join(cwd, 'dist-apps', 'app.html'), 'utf8');
        expect(createHash('sha256').update(html).digest('hex')).toBe(override.sha256);
        expect(readFileSync(join(cwd, 'dist-apps', 'manifest.json'), 'utf8')).toBe(
          override.manifest,
        );
      } finally {
        rmSync(cwd, { recursive: true, force: true });
      }
    },
    buildTimeoutMs,
  );
});

describe('MCP App bundle environment independence', () => {
  it(
    'is byte-identical under NODE_ENV development and production, with no React development build',
    () => {
      const development = buildApp('development');
      const production = buildApp('production');
      for (const marker of developmentMarkers) {
        expect(production.html, `production bundle contains "${marker}"`).not.toContain(marker);
        expect(development.html, `development-env bundle contains "${marker}"`).not.toContain(
          marker,
        );
      }
      expect(development.sha256).toBe(production.sha256);
    },
    buildTimeoutMs,
  );

  it(
    'is byte-identical when NODE_ENV is unset',
    () => {
      expect(buildApp(undefined).sha256).toBe(buildApp('production').sha256);
    },
    buildTimeoutMs,
  );
});

describe('workspace UI build environment independence', () => {
  it(
    'ships no React development build when the caller exports NODE_ENV=development',
    () => {
      const outDir = mkdtempSync(join(tmpdir(), 'okf-ui-build-'));
      try {
        const run = spawnSync('bunx', ['vite', 'build', '--outDir', outDir, '--emptyOutDir'], {
          cwd: uiRoot,
          env: { ...process.env, NODE_ENV: 'development' },
          encoding: 'utf8',
          maxBuffer: 64 * 1024 * 1024,
        });
        if (run.status !== 0) throw new Error(`vite build failed: ${run.stderr}${run.error ?? ''}`);
        const assets = join(outDir, 'assets');
        const scripts = readdirSync(assets).filter((name) => name.endsWith('.js'));
        expect(scripts.length).toBeGreaterThan(0);
        for (const name of scripts) {
          const code = readFileSync(join(assets, name), 'utf8');
          for (const marker of developmentMarkers)
            expect(code, `${name} contains "${marker}"`).not.toContain(marker);
        }
      } finally {
        rmSync(outDir, { recursive: true, force: true });
      }
    },
    buildTimeoutMs,
  );
});

describe('stylesheet source scope', () => {
  const scratch: string[] = [];
  afterEach(() => {
    for (const path of scratch.splice(0)) rmSync(path, { force: true });
  });
  function plant(relative: string, text: string) {
    const path = join(uiRoot, relative);
    mkdirSync(join(path, '..'), { recursive: true });
    writeFileSync(path, text);
    scratch.push(path);
  }

  it(
    'ignores Tailwind class words in files outside ui/src and keeps the classes the App uses',
    () => {
      const baseline = buildApp('production');
      expect(baseline.html).toContain('.view-stack');
      expect(baseline.html).toContain('.view-columns');
      plant('tests/zz-scratch-outside.txt', `${outsideWords}\n`);
      expect(buildApp('production').sha256).toBe(baseline.sha256);
    },
    buildTimeoutMs,
  );

  it(
    'leaves the workspace UI stylesheet unchanged by class words outside ui/src',
    () => {
      const baseline = buildWorkspaceCss();
      expect(baseline).toContain('.view-stack');
      plant('tests/zz-scratch-workspace.txt', `${outsideWords}\n`);
      expect(buildWorkspaceCss()).toBe(baseline);
    },
    buildTimeoutMs,
  );

  it(
    'still generates utilities for class words under ui/src',
    () => {
      const baseline = buildApp('production');
      expect(baseline.html).not.toContain(`.${insideWord}`);
      plant('src/zz-scratch-inside.txt', `${insideWord}\n`);
      expect(buildApp('production').html).toContain(`.${insideWord}`);
    },
    buildTimeoutMs,
  );
});
