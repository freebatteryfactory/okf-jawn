/**
 * The steps of one Docling qualification run, with all I/O passed in.
 *
 * run.mjs observes the clean tree (the receipt header) and then calls `qualify`. From that
 * point every run ends with a receipt for this run at `<outDir>/receipt.json`:
 *
 *   - a step that fails before any conversion (the asset manifest is missing, an asset is
 *     missing or changed, the build or `cargo tree` fails) stops the run. Its sentence becomes
 *     `harness_error`, every criterion not yet judged is not_judged, and the result is
 *     INCOMPLETE. The receipt is WRITTEN rather than the old one removed: a receipt that says
 *     which step stopped which commit's run is evidence, and a missing file cannot be told
 *     from a run that was never started. The previous receipt is overwritten either way, so
 *     it cannot be read as current;
 *   - the per-fixture evidence directory is emptied before the first step, so the files beside
 *     a receipt are always the ones of the same run.
 *
 * An argument, a dirty tree and an unknown fixture name are refused by run.mjs before the run
 * starts; nothing is written and nothing is removed.
 *
 * Order: assets are re-hashed before anything is built; the build precedes every conversion.
 */

import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { pathContext, scrubReceiptPaths } from '../../../scripts/lib/provenance.mjs';
import { lockedPackage, lockedPackages } from '../../lib/cargo.mjs';
import { TREE_ARGS, buildFacts } from './build.mjs';
import { readOnnxRuntime } from './native.mjs';
import { TIMEOUT_PROBE, buildDoclingReceipt } from './receipt.mjs';

export const PACKAGE = 'okf-qualify-docling';
export const FIXTURES = 'tests/fixtures/documents';
export const MANIFEST = '.artifacts/qualification/docling/assets.json';

const oneLine = (text) => String(text ?? '').trim().replace(/\s+/g, ' ');

/** Run one preparation step; a failure is reported as what stopped the run. */
async function step(what, action) {
  try {
    return await action();
  } catch (error) {
    throw new Error(`${what}: ${oneLine(error.message)}`, { cause: error });
  }
}

/**
 * @param {object} input
 * @param {string} input.root repository root
 * @param {{ git_sha: string, inputs: string[], produced_at: string }} input.header from receiptHeader
 * @param {string} input.outDir where receipt.json and partial/ are written
 * @param {string} [input.manifestPath] the asset manifest; defaults to MANIFEST under root
 * @param {string[]} input.selected fixture runs to execute, in order
 * @param {object} input.scope recorded as is
 * @param {Record<string, string|undefined>} input.env the caller's environment
 * @param {string} input.platform
 * @param {ReturnType<typeof pathContext>} [input.paths] the roots path strings are judged against (scripts/lib/provenance.mjs);
 *   defaults to this machine's repository, home and temp directories
 * @param {object} input.io readFile, writeFile, mkdir, rm, readdir, stat, sha256File, verifyAssets,
 *   buildRelease, exec, runFixtureProcess, loadEvidence, log(text), logError(text), now()
 * @returns {Promise<{ receipt: object, receiptPath: string, converted: boolean }>} `converted`
 *   is false when the run stopped before any fixture was converted
 */
export async function qualify({ root, header, outDir, manifestPath = join(root, MANIFEST), selected, scope, env, platform, io, paths = pathContext({ root }) }) {
  const partialDir = join(outDir, 'partial');
  const receiptPath = join(outDir, 'receipt.json');
  const fixturesDir = join(root, FIXTURES);
  const facts = { sources: null, converter: null, build: null, assets: null, environment: null };
  const runs = [];
  let harnessError = null;

  await io.mkdir(outDir, { recursive: true });
  await io.rm(partialDir, { recursive: true, force: true });
  await io.mkdir(partialDir, { recursive: true });

  try {
    facts.sources = await step('the fixture declarations in SOURCES.json could not be read', async () =>
      JSON.parse(await io.readFile(join(fixturesDir, 'SOURCES.json'), 'utf8')),
    );
    const manifestBytes = await step(`the Docling model assets manifest is not readable at ${manifestPath} (qualification needs the verified model assets)`, () =>
      io.readFile(manifestPath),
    );
    io.log('Verifying Docling model assets against the manifest hashes...\n');
    const { manifest, verified } = await step('the Docling model assets could not be verified', () => io.verifyAssets(manifestBytes));
    io.log(`Verified ${verified.matched} of ${verified.count} assets (${verified.bytes_total} bytes) against the manifest hashes.\n`);
    facts.assets = { ...verified, manifest: manifestPath, models_dir: manifest.DOCLING_RS_MODELS_DIR, models_source: manifest.models_source ?? null };

    const docling = await step('Cargo.lock does not pin the docling crate', async () => {
      const lockText = await io.readFile(join(root, 'Cargo.lock'), 'utf8');
      const pinned = lockedPackage(lockText, 'docling');
      facts.converter = {
        crate: 'docling',
        version: pinned.version,
        checksum: pinned.checksum,
        docling_core_versions: lockedPackages(lockText, 'docling-core').map((entry) => entry.version),
        source: 'Cargo.lock',
      };
      return pinned;
    });

    // What the converter processes see beyond the caller's environment. Recorded as settings.
    facts.environment = {
      DOCLING_RS_MODELS_DIR: manifest.DOCLING_RS_MODELS_DIR,
      ...manifest.recommended_env,
      OKF_DOCLING_FIXTURES: fixturesDir,
      OKF_DOCLING_CRATE_VERSION: docling.version,
      OKF_DOCLING_HOLD: '1',
    };

    const binPath = await step('the harness build failed', () => io.buildRelease(root, PACKAGE));
    facts.build = await step('the docling features of this build could not be read from cargo', async () => {
      const tree = await io.exec('cargo', TREE_ARGS, { cwd: root });
      if (tree.code !== 0) throw new Error(`cargo ${TREE_ARGS.join(' ')} exited ${tree.code} ${tree.stderr}`);
      return buildFacts({
        harnessToml: await io.readFile(join(root, 'qualification/docling/Cargo.toml'), 'utf8'),
        workspaceToml: await io.readFile(join(root, 'Cargo.toml'), 'utf8'),
        treeText: tree.stdout,
        // buildRelease (qualification/lib/cargo.mjs) runs exactly this and does not return its argv.
        command: `cargo build --locked --release -p ${PACKAGE}`,
      });
    });
    // The ONNX Runtime library this build links is not in the asset manifest: record it from the build.
    const onnxRuntime = await readOnnxRuntime({
      root,
      pkg: PACKAGE,
      exec: io.exec,
      readFile: io.readFile,
      readdir: io.readdir,
      hashFile: async (path) => ({ bytes: (await io.stat(path)).size, sha256: await io.sha256File(path) }),
    });
    facts.assets = { ...facts.assets, native_libraries: [onnxRuntime] };

    for (const name of selected) {
      io.log(`\n=== Docling fixture: ${name} ===\n`);
      const fixtureOut = join(partialDir, name.replaceAll(/[\\/]/g, '__'));
      await io.mkdir(fixtureOut, { recursive: true });
      const run = await io.runFixtureProcess({
        command: binPath,
        cwd: root,
        env: { ...env, ...facts.environment, OKF_DOCLING_ONLY: name, OKF_DOCLING_OUT: fixtureOut },
        onStdout: (text) => io.log(text),
        onStderr: (text) => io.logError(text),
      });
      let report = null;
      try {
        report = JSON.parse(await io.readFile(join(fixtureOut, 'receipt.json'), 'utf8'));
      } catch {
        report = null; // lib/receipt.mjs records a process without a receipt as a harness error
      }
      const written = name === TIMEOUT_PROBE ? report?.timeout_case?.document : report?.receipts?.[0]?.document;
      runs.push({
        only: name,
        run: { ...run, stdoutSha256: createHash('sha256').update(run.stdout ?? '').digest('hex') },
        report,
        evidence: await io.loadEvidence(fixtureOut, written),
      });
    }
  } catch (error) {
    harnessError = oneLine(error.message);
  }

  // Every path in the receipt is written by one rule, here and nowhere else (scripts/lib/provenance.mjs).
  const receipt = scrubReceiptPaths(buildDoclingReceipt({
    header,
    converter: facts.converter,
    platform,
    build: facts.build,
    assets: facts.assets,
    environment: facts.environment,
    sources: facts.sources,
    runs,
    scope,
    paths: { fixtures_dir: FIXTURES, sources: `${FIXTURES}/SOURCES.json`, per_fixture_evidence: partialDir },
    finishedAt: io.now(),
    harnessError,
  }), paths);
  await io.writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
  return { receipt, receiptPath, converted: runs.length > 0 };
}
