/**
 * Record the ONNX Runtime native library this build links, from where the build leaves it.
 *
 * SPEC section 5 asks for the real native library requirements "with verified download
 * locations, versions, hashes and configured local paths". The library is not in the asset
 * manifest: the `ort-sys` build script downloads a prebuilt ONNX Runtime, compares the
 * archive with a SHA-256 compiled into the crate (ort-sys build/main.rs), extracts it into a
 * cache directory named by that hash, and tells cargo to link from there. So:
 *   - cargo's own `build-script-executed` message for ort-sys gives the directory and the
 *     linked libraries of this build (`cargo build --message-format=json`, a no-op after the
 *     build has run);
 *   - the last segment of that directory is the hash ort-sys verified;
 *   - the row of ort-sys's `build/download/dist.tsv` with that hash gives the download URL,
 *     which names the ONNX Runtime version, the target and the feature set.
 * The files in the directory are hashed again in this run. Anything that cannot be read this
 * way is recorded as `not_recorded` with the reason; no value is assumed.
 */

import { basename, dirname, join } from 'node:path';

const HASH = /^[0-9a-f]{64}$/;
const CRATE = 'ort-sys';

export const BUILD_MESSAGES_ARGS = (pkg) => ['build', '--locked', '--release', '-p', pkg, '--message-format=json'];
export const METADATA_ARGS = ['metadata', '--format-version', '1', '--locked'];

const notRecorded = (reason) => ({ name: 'onnxruntime', status: 'not_recorded', reason });

/** The `build-script-executed` message of ort-sys in cargo's JSON message stream, or null. */
export function ortBuildMessage(messagesText) {
  for (const line of String(messagesText ?? '').split(/\r?\n/)) {
    if (!line.startsWith('{') || !line.includes('"build-script-executed"')) continue;
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      continue;
    }
    if (message.reason === 'build-script-executed' && /[#/]ort-sys@/.test(String(message.package_id))) return message;
  }
  return null;
}

/** Rows of ort-sys's dist.tsv: target, feature set, download URL, SHA-256 of the archive. */
export function distRows(tsvText) {
  return String(tsvText ?? '')
    .split(/\r?\n/)
    .slice(1)
    .filter((line) => line.length > 0 && !line.startsWith('#'))
    .map((line) => line.split('\t'))
    .filter((cells) => cells.length >= 4)
    .map(([target, feature_set, url, sha256]) => ({ target, feature_set, url, sha256: sha256.trim().toLowerCase() }));
}

/**
 * The record for the receipt's asset section.
 * @param {object} input
 * @param {string} input.messagesText stdout of `cargo build --message-format=json`
 * @param {string|null} input.distTsv text of ort-sys build/download/dist.tsv
 * @param {string|null} input.distPath where that text was read
 * @param {{ file: string, bytes: number, sha256: string }[]} [input.files] the linked directory's files, hashed in this run
 */
export function onnxRuntimeRecord({ messagesText, distTsv, distPath, files = [] }) {
  const message = ortBuildMessage(messagesText);
  if (!message) return notRecorded('cargo printed no build-script-executed message for ort-sys in this build');
  const directories = (message.linked_paths ?? []).map((entry) => String(entry).replace(/^native=/, ''));
  const directory = directories.find((entry) => HASH.test(basename(entry.replaceAll('\\', '/')).toLowerCase()));
  if (!directory) {
    return notRecorded(`ort-sys links from ${JSON.stringify(directories)}, none of which is a download directory named by a SHA-256; the library was not taken from a verified download`);
  }
  const sha256 = basename(directory.replaceAll('\\', '/')).toLowerCase();
  const row = distRows(distTsv).find((entry) => entry.sha256 === sha256);
  if (!row) return notRecorded(`${distPath ?? 'ort-sys dist.tsv'} has no row with the hash ${sha256} of the directory this build links from`);
  return {
    name: 'onnxruntime',
    status: 'recorded',
    version: /@(\d+\.\d+\.\d+)\//.exec(row.url)?.[1] ?? null,
    url: row.url,
    sha256,
    sha256_is: 'the SHA-256 ort-sys compares the downloaded archive with before it keeps the extraction; the directory this build links from is named by it',
    target: row.target,
    feature_set: row.feature_set,
    crate: CRATE,
    crate_version: /ort-sys@([^\s"]+)$/.exec(String(message.package_id))?.[1] ?? null,
    linked_libs: message.linked_libs ?? [],
    directory,
    files,
    read_from: ['cargo build --message-format=json: the build-script-executed message of ort-sys (linked_paths, linked_libs)', distPath],
  };
}

/**
 * Read the record with the given I/O. Never throws: a step that fails is the reason.
 * @param {object} input
 * @param {string} input.root repository root
 * @param {string} input.pkg the harness package
 * @param {(command: string, args: string[], options: object) => Promise<{ code: number, stdout: string, stderr: string }>} input.exec
 * @param {(path: string, encoding: string) => Promise<string>} input.readFile
 * @param {(path: string) => Promise<string[]>} input.readdir
 * @param {(path: string) => Promise<{ bytes: number, sha256: string }>} input.hashFile
 */
export async function readOnnxRuntime({ root, pkg, exec, readFile, readdir, hashFile }) {
  try {
    const built = await exec('cargo', BUILD_MESSAGES_ARGS(pkg), { cwd: root });
    if (built.code !== 0) return notRecorded(`cargo ${BUILD_MESSAGES_ARGS(pkg).join(' ')} exited ${built.code}`);
    const metadata = await exec('cargo', METADATA_ARGS, { cwd: root });
    if (metadata.code !== 0) return notRecorded(`cargo ${METADATA_ARGS.join(' ')} exited ${metadata.code}`);
    const manifest = JSON.parse(metadata.stdout).packages?.find((entry) => entry.name === CRATE)?.manifest_path;
    if (typeof manifest !== 'string') return notRecorded('cargo metadata lists no ort-sys package');
    const distPath = join(dirname(manifest), 'build', 'download', 'dist.tsv');
    const distTsv = await readFile(distPath, 'utf8');
    const record = onnxRuntimeRecord({ messagesText: built.stdout, distTsv, distPath });
    if (record.status !== 'recorded') return record;
    const files = [];
    for (const file of (await readdir(record.directory)).sort()) files.push({ file, ...(await hashFile(join(record.directory, file))) });
    return { ...record, files };
  } catch (error) {
    return notRecorded(`reading the ONNX Runtime record failed: ${error.message}`);
  }
}
