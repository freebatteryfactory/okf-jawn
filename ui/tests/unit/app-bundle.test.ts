// @vitest-environment node
/**
 * The MCP App bundle is one production build whatever the caller's environment or the files that
 * happen to sit under ui/, and the workspace build is narrowed the same way.
 *
 * Every claim is proven by a real build (about 10 s each), but each distinct configuration is built
 * once, in beforeAll, and the tests compare against it: six App builds and two workspace builds.
 * Builds are serialized, write to temporary directories that are removed afterwards (the App through
 * OKF_APP_OUT), and plant scratch files only for the build that needs them.
 */

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { beforeAll, describe, expect, it } from 'vitest';

const uiRoot = fileURLToPath(new URL('../..', import.meta.url));
const buildTimeoutMs = 240_000;
// Class words are assembled at run time so this file does not itself put them under ui/.
const outsideWords = [
  ['under', 'line'].join(''),
  ['line', 'through'].join('-'),
  ['ita', 'lic'].join(''),
].join(' ');
const insideWord = ['line', 'through'].join('-');
// Strings present only in React's development build (verified against a NODE_ENV=development bundle).
const developmentMarkers = ['Download the React DevTools', 'react-stack-top-frame'];

interface AppBuild {
  html: string;
  manifest: string;
  sha256: string;
}
interface WorkspaceBuild {
  scripts: Map<string, string>;
  css: string;
}

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

function readApp(dir: string): AppBuild {
  const html = readFileSync(join(dir, 'app.html'), 'utf8');
  const manifest = readFileSync(join(dir, 'manifest.json'), 'utf8');
  return { html, manifest, sha256: createHash('sha256').update(html).digest('hex') };
}

/** Build the App into a temporary OKF_APP_OUT directory that is removed afterwards. */
function buildApp(nodeEnv: string | undefined): AppBuild {
  const outDir = mkdtempSync(join(tmpdir(), 'okf-app-out-'));
  try {
    runBundle(uiRoot, { ...bundleEnv(nodeEnv), OKF_APP_OUT: outDir });
    return readApp(outDir);
  } finally {
    rmSync(outDir, { recursive: true, force: true });
  }
}

/** Build the App from a temporary working directory, without OKF_APP_OUT. */
function buildAppInTemporaryCwd(): AppBuild {
  const cwd = mkdtempSync(join(tmpdir(), 'okf-app-cwd-'));
  try {
    runBundle(cwd, bundleEnv('production'));
    return readApp(join(cwd, 'dist-apps'));
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
}

/** Build the workspace UI into a temporary directory. */
function buildWorkspace(nodeEnv: string): WorkspaceBuild {
  const outDir = mkdtempSync(join(tmpdir(), 'okf-ui-build-'));
  try {
    const run = spawnSync('bunx', ['vite', 'build', '--outDir', outDir, '--emptyOutDir'], {
      cwd: uiRoot,
      env: bundleEnv(nodeEnv),
      encoding: 'utf8',
      maxBuffer: 64 * 1024 * 1024,
    });
    if (run.status !== 0) throw new Error(`vite build failed: ${run.stderr}${run.error ?? ''}`);
    const assets = join(outDir, 'assets');
    const names = readdirSync(assets);
    const read = (name: string) => readFileSync(join(assets, name), 'utf8');
    return {
      scripts: new Map(names.filter((name) => name.endsWith('.js')).map((n) => [n, read(n)])),
      css: names
        .filter((name) => name.endsWith('.css'))
        .map(read)
        .join('\n'),
    };
  } finally {
    rmSync(outDir, { recursive: true, force: true });
  }
}

/** Run a build while a scratch file exists at `relative` under ui/; the file is always removed. */
function withScratch<T>(relative: string, text: string, build: () => T): T {
  const path = join(uiRoot, relative);
  mkdirSync(join(path, '..'), { recursive: true });
  writeFileSync(path, text);
  try {
    return build();
  } finally {
    rmSync(path, { force: true });
  }
}

const builds = {} as {
  production: AppBuild;
  development: AppBuild;
  unset: AppBuild;
  defaultOutput: AppBuild;
  outsideScratch: AppBuild;
  insideScratch: AppBuild;
  workspace: WorkspaceBuild;
  workspaceOutsideScratch: WorkspaceBuild;
};

beforeAll(() => {
  builds.production = buildApp('production');
  builds.development = buildApp('development');
  builds.unset = buildApp(undefined);
  builds.defaultOutput = buildAppInTemporaryCwd();
  builds.outsideScratch = withScratch('tests/zz-scratch-outside.txt', `${outsideWords}\n`, () =>
    buildApp('production'),
  );
  builds.insideScratch = withScratch('src/zz-scratch-inside.txt', `${insideWord}\n`, () =>
    buildApp('production'),
  );
  // The workspace build under NODE_ENV=development doubles as the stylesheet baseline: the
  // stylesheet does not depend on NODE_ENV, and the scratch build uses the same environment.
  builds.workspace = buildWorkspace('development');
  builds.workspaceOutsideScratch = withScratch(
    'tests/zz-scratch-workspace.txt',
    `${outsideWords}\n`,
    () => buildWorkspace('development'),
  );
}, buildTimeoutMs);

describe('MCP App bundle output directory', () => {
  it('writes app.html and manifest.json to OKF_APP_OUT, and to ./dist-apps of the working directory by default', () => {
    expect(JSON.parse(builds.production.manifest).resources[0].sha256).toBe(
      builds.production.sha256,
    );
    expect(builds.defaultOutput.sha256).toBe(builds.production.sha256);
    expect(builds.defaultOutput.manifest).toBe(builds.production.manifest);
  });
});

describe('MCP App bundle environment independence', () => {
  it('is byte-identical under NODE_ENV development and production, with no React development build', () => {
    for (const marker of developmentMarkers) {
      expect(builds.production.html, `production bundle contains "${marker}"`).not.toContain(
        marker,
      );
      expect(builds.development.html, `development-env bundle contains "${marker}"`).not.toContain(
        marker,
      );
    }
    expect(builds.development.sha256).toBe(builds.production.sha256);
  });

  it('is byte-identical when NODE_ENV is unset', () => {
    expect(builds.unset.sha256).toBe(builds.production.sha256);
  });
});

describe('workspace UI build environment independence', () => {
  it('ships no React development build when the caller exports NODE_ENV=development', () => {
    expect(builds.workspace.scripts.size).toBeGreaterThan(0);
    for (const [name, code] of builds.workspace.scripts)
      for (const marker of developmentMarkers)
        expect(code, `${name} contains "${marker}"`).not.toContain(marker);
  });
});

describe('stylesheet source scope', () => {
  it('ignores Tailwind class words in files outside ui/src and keeps the classes the App uses', () => {
    expect(builds.production.html).toContain('.view-stack');
    expect(builds.production.html).toContain('.view-columns');
    expect(builds.outsideScratch.sha256).toBe(builds.production.sha256);
  });

  it('leaves the workspace UI stylesheet unchanged by class words outside ui/src', () => {
    expect(builds.workspace.css).toContain('.view-stack');
    expect(builds.workspaceOutsideScratch.css).toBe(builds.workspace.css);
  });

  it('still generates utilities for class words under ui/src', () => {
    expect(builds.production.html).not.toContain(`.${insideWord}`);
    expect(builds.insideScratch.html).toContain(`.${insideWord}`);
  });
});
