/**
 * Which reference host a run used, stated from what was fetched and what is on disk.
 *
 * The ext-apps package does not ship its basic-host example, so the run fetches the example's
 * sources from the repository. A tag can be moved; a commit cannot. The tag is therefore
 * resolved to a commit once (git ls-remote), every file is fetched at that commit, and the
 * commit and each file's sha256 are written beside the sources. Every later run hashes the
 * files again: a cache that no longer matches what was fetched stops the run as a harness
 * error instead of being rendered under the old name. Pure: the I/O is in run.mjs.
 */

/** The example the harness renders the App in. `tag` is the request; the commit is observed. */
export const BASIC_HOST = Object.freeze({
  repository: 'https://github.com/modelcontextprotocol/ext-apps',
  tag: 'v2.0.3',
  directory: 'examples/basic-host',
  files: Object.freeze([
    'package.json',
    'index.html',
    'sandbox.html',
    'serve.ts',
    'tsconfig.json',
    'vite.config.ts',
    'src/index.tsx',
    'src/implementation.ts',
    'src/sandbox.ts',
    'src/theme.ts',
    'src/index.module.css',
    'src/global.css',
    'src/host-styles.ts',
    'src/vite-env.d.ts',
  ]),
});

/** The file, beside the sources, that says what was fetched. */
export const SOURCE_RECORD = 'okf-source.json';
/** The served copy of serve.ts with the one patch this machine needs; the fetched file stays as fetched. */
export const PATCHED_SERVE = 'okf-serve.ts';

/** The arguments of the `git ls-remote` that resolves the tag: the tag itself and, for an annotated tag, what it points at. */
export const lsRemoteArgs = (host = BASIC_HOST) => ['ls-remote', host.repository, `refs/tags/${host.tag}`, `refs/tags/${host.tag}^{}`];

/**
 * The commit a tag names, from `git ls-remote` output. An annotated tag lists the tag object
 * and then the commit under `<ref>^{}`; a lightweight tag lists the commit only.
 */
export function tagCommit(lsRemoteOutput, tag) {
  const refs = new Map();
  for (const line of String(lsRemoteOutput).split(/\r?\n/)) {
    const match = /^([0-9a-f]{40})\s+(\S+)$/.exec(line.trim());
    if (match) refs.set(match[2], match[1]);
  }
  const commit = refs.get(`refs/tags/${tag}^{}`) ?? refs.get(`refs/tags/${tag}`);
  if (!commit) throw new Error(`git ls-remote did not list the tag ${tag}`);
  return commit;
}

/** Where one file of the example is fetched from, at a commit. */
export function rawUrl(commit, file, host = BASIC_HOST) {
  const repository = host.repository.replace('https://github.com/', '');
  return `https://raw.githubusercontent.com/${repository}/${commit}/${host.directory}/${file}`;
}

/**
 * Why the sources on disk (`hashes`: path -> sha256) are not what `record` says was fetched
 * for `host`; empty when they are. A record for another tag or repository, or without a
 * commit, is not a record of this host.
 */
export function sourceProblems(record, hashes, host = BASIC_HOST) {
  if (record === null || typeof record !== 'object') return ['no record of what was fetched'];
  const problems = [];
  if (record.repository !== host.repository) problems.push(`fetched from ${record.repository}, not ${host.repository}`);
  if (record.tag !== host.tag) problems.push(`fetched at tag ${record.tag}, not ${host.tag}`);
  if (typeof record.commit !== 'string' || !/^[0-9a-f]{40}$/.test(record.commit)) problems.push('no commit recorded');
  for (const file of host.files) {
    const fetched = record.files?.[file];
    if (typeof fetched !== 'string') problems.push(`${file} has no recorded sha256`);
    else if (hashes[file] !== fetched) problems.push(`${file} is ${hashes[file] ?? 'missing'}, fetched as ${fetched}`);
  }
  return problems;
}

const SEND_FILE = 'res.sendFile(join(DIRECTORY, "sandbox.html"));';

/**
 * serve.ts with sandbox.html sent relative to a root: Express sendFile with an absolute joined
 * path answers 404 under Bun on Windows. Throws when the line is not there, so an upstream
 * change is noticed instead of silently served unpatched.
 */
export function patchServe(source) {
  if (!source.includes(SEND_FILE)) throw new Error(`basic-host serve.ts no longer contains ${SEND_FILE}`);
  return source.replace(SEND_FILE, 'res.sendFile("sandbox.html", { root: DIRECTORY });');
}

/** What the receipt says about the host: the request, the observed commit and the proof the files are those. */
export function sourceRecord({ status, record, hashes, packageJson, lockfileSha256 }, host = BASIC_HOST) {
  return {
    status,
    repository: record.repository,
    requested_tag: host.tag,
    commit: record.commit,
    commit_resolved_at: record.fetched_at,
    files_verified: host.files.filter((file) => hashes[file] === record.files[file]).length,
    files_expected: host.files.length,
    package: { name: packageJson.name, version: packageJson.version },
    lockfile_sha256: lockfileSha256,
  };
}
