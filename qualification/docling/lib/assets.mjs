/**
 * Re-hash the Docling model assets the manifest names, at run time, before anything converts.
 *
 * SPEC section 5: "never invent a hash or call a missing asset qualified". The manifest
 * (.artifacts/qualification/docling/assets.json, machine-local) lists each asset with path,
 * bytes and sha256. A listed hash proves nothing about the file a later run loads, so
 * verifyAssets reads every file again and throws, naming the file, on the first one that is
 * missing, has another length or another hash. Nothing here downloads or repairs.
 */

import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { stat } from 'node:fs/promises';
import { relative } from 'node:path';

const SHA256 = /^[0-9a-f]{64}$/;

/** SHA-256 of a file read as a stream (the largest asset is over 170 MB). */
export function sha256File(path) {
  return new Promise((resolveHash, reject) => {
    const hash = createHash('sha256');
    createReadStream(path)
      .on('error', reject)
      .on('data', (chunk) => hash.update(chunk))
      .on('end', () => resolveHash(hash.digest('hex')));
  });
}

/** Parse the manifest text (UTF-8, with or without a BOM) and check its shape. */
export function parseManifest(text) {
  const manifest = JSON.parse(String(text).replace(/^﻿/, ''));
  if (typeof manifest?.DOCLING_RS_MODELS_DIR !== 'string' || manifest.DOCLING_RS_MODELS_DIR.length === 0) {
    throw new Error('Docling assets manifest: DOCLING_RS_MODELS_DIR must name the models directory');
  }
  if (!Array.isArray(manifest.assets) || manifest.assets.length === 0) {
    throw new Error('Docling assets manifest: assets must be a non-empty array; a run without verified assets is not a qualification');
  }
  manifest.assets.forEach((asset, index) => {
    if (typeof asset?.path !== 'string' || asset.path.length === 0) {
      throw new Error(`Docling assets manifest: assets[${index}] has no path`);
    }
    if (!Number.isInteger(asset.bytes) || asset.bytes < 0) {
      throw new Error(`Docling assets manifest: ${asset.path} has no integer byte length`);
    }
    if (typeof asset.sha256 !== 'string' || !SHA256.test(asset.sha256.toLowerCase())) {
      throw new Error(`Docling assets manifest: ${asset.path} has no 64-hex sha256`);
    }
  });
  return manifest;
}

const samePath = (a, b) => String(a).replaceAll('\\', '/').toLowerCase() === String(b).replaceAll('\\', '/').toLowerCase();

/**
 * Model files the manifest's recommended_env points the converter at that are not in the
 * verified asset list. The converter would load them unhashed, so the run must refuse.
 */
export function unverifiedEnvPaths(manifest) {
  const dir = manifest.DOCLING_RS_MODELS_DIR;
  return Object.entries(manifest.recommended_env ?? {})
    .filter(([, value]) => typeof value === 'string' && !samePath(value, dir))
    .filter(([, value]) => /[\\/]/.test(value))
    .filter(([, value]) => !manifest.assets.some((asset) => samePath(asset.path, value)))
    .map(([name, value]) => `${name}=${value}`);
}

/**
 * Hash every listed asset now. Resolves with what the receipt records; rejects naming the
 * first file that is missing or differs.
 * @param {Buffer|string} manifestBytes the manifest file exactly as read; its hash is recorded
 */
export async function verifyAssets(manifestBytes) {
  const manifest = parseManifest(Buffer.from(manifestBytes).toString('utf8'));
  const unverified = unverifiedEnvPaths(manifest);
  if (unverified.length) {
    throw new Error(
      `Docling assets manifest: recommended_env points at files with no verified hash: ${unverified.join(', ')}`,
    );
  }
  const files = [];
  for (const asset of manifest.assets) {
    let size;
    try {
      size = (await stat(asset.path)).size;
    } catch (error) {
      throw new Error(`Docling model asset missing: ${asset.path} (${error.code ?? error.message}). A missing asset is never qualified.`);
    }
    if (size !== asset.bytes) {
      throw new Error(`Docling model asset changed: ${asset.path} is ${size} bytes, manifest says ${asset.bytes}`);
    }
    const sha256 = await sha256File(asset.path);
    if (sha256 !== asset.sha256.toLowerCase()) {
      throw new Error(`Docling model asset changed: ${asset.path} has sha256 ${sha256}, manifest says ${asset.sha256.toLowerCase()}`);
    }
    files.push({
      file: relative(manifest.DOCLING_RS_MODELS_DIR, asset.path).replaceAll('\\', '/'),
      path: asset.path,
      bytes: size,
      sha256,
    });
  }
  return {
    manifest,
    verified: {
      count: files.length,
      all_match: true,
      manifest_sha256: createHash('sha256').update(Buffer.from(manifestBytes)).digest('hex'),
      bytes_total: files.reduce((sum, file) => sum + file.bytes, 0),
      hashed_at_run_time: true,
      files,
    },
  };
}

/**
 * Compare the models the library says it would load (docling::model_inventory, reported by
 * the converter process) with the verified files. `pdfium` is the optional native renderer.
 */
export function inventoryCheck(inventory, verifiedFiles) {
  if (!Array.isArray(inventory) || inventory.length === 0) {
    return { status: 'FAIL', reason: 'no converter process reported docling::model_inventory()', entries: [] };
  }
  const entries = inventory.map((entry) => {
    const match = verifiedFiles.find((file) => samePath(file.path, entry.path));
    return {
      stage: entry.stage,
      path: entry.path,
      found: entry.found,
      bytes: entry.bytes,
      verified_sha256: match && match.bytes === entry.bytes ? match.sha256 : null,
    };
  });
  const unverified = entries.filter((entry) => entry.stage !== 'pdfium' && (!entry.found || entry.verified_sha256 === null));
  const pdfium = entries.find((entry) => entry.stage === 'pdfium') ?? null;
  return {
    status: unverified.length ? 'FAIL' : 'PASS',
    source: 'docling::model_inventory() called in a converter process under the same environment',
    pdfium_library_found: pdfium ? pdfium.found : null,
    unverified: unverified.map((entry) => entry.stage),
    entries,
  };
}
