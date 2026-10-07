/**
 * Facts about the build the harness ran as, and what kind of refusal a converter error is.
 * Pure: no I/O and no imports. run.mjs supplies the manifest texts and `cargo tree` output.
 *
 * The must-fail fixture is refused with "pdfium support is not compiled in": docling-pdf's
 * pure-Rust parser could not read the file and the optional pdfium fallback is not part of
 * this build (docling-pdf pdfium_backend.rs:1287). That is a different statement from "the
 * converter inspected the file and rejected it", so the receipt names which one happened and
 * records, from cargo's own feature resolution, whether pdfium was compiled in.
 */

/** The sentence docling-pdf returns when its primary parser failed and no pdfium is built in. */
export const NO_PDFIUM_FALLBACK = 'pdfium support is not compiled in';

export const TREE_ARGS = ['tree', '--locked', '-p', 'okf-qualify-docling', '-e', 'normal', '--prefix', 'none', '-f', '{p}|{f}'];

/** Classify a converter refusal from its error text alone. */
export function classifyRefusal(errorText) {
  return String(errorText ?? '').includes(NO_PDFIUM_FALLBACK)
    ? 'primary_parser_failed_no_fallback_in_build'
    : 'converter_rejected';
}

function inlineTable(tomlText, key) {
  const match = new RegExp(`^${key}\\s*=\\s*\\{([^}]*)\\}`, 'm').exec(tomlText);
  return match ? match[1] : null;
}

function stringArray(body, key) {
  const match = new RegExp(`\\b${key}\\s*=\\s*\\[([^\\]]*)\\]`).exec(body ?? '');
  return match ? [...match[1].matchAll(/"([^"]+)"/g)].map((item) => item[1]) : [];
}

/**
 * The docling features the harness asks for: its own Cargo.toml entry, which may inherit
 * the workspace entry (`workspace = true`) and add features to it.
 */
export function declaredDoclingFeatures(harnessToml, workspaceToml) {
  const own = inlineTable(harnessToml, 'docling');
  if (own === null) throw new Error('qualification/docling/Cargo.toml has no inline docling dependency');
  const inherits = /\bworkspace\s*=\s*true\b/.test(own);
  const shared = inherits ? inlineTable(workspaceToml, 'docling') : null;
  if (inherits && shared === null) throw new Error('Cargo.toml [workspace.dependencies] has no inline docling entry');
  const declaring = inherits ? shared : own;
  return {
    features: [...new Set([...stringArray(shared, 'features'), ...stringArray(own, 'features')])].sort(),
    default_features: !/\bdefault-features\s*=\s*false\b/.test(declaring),
    declared_in: inherits
      ? 'Cargo.toml [workspace.dependencies] docling, inherited by qualification/docling/Cargo.toml (workspace = true)'
      : 'qualification/docling/Cargo.toml',
  };
}

/** Features cargo resolved per package, from `cargo tree ... -f "{p}|{f}"` output. */
export function resolvedFeatures(treeText) {
  const resolved = {};
  for (const line of String(treeText ?? '').split(/\r?\n/)) {
    const match = /^(\S+) v\S+(?: \([^)]*\))*\|([^ ]*)/.exec(line.trim());
    if (!match) continue;
    const features = match[2].split(',').filter(Boolean);
    resolved[match[1]] = [...new Set([...(resolved[match[1]] ?? []), ...features])].sort();
  }
  return resolved;
}

/**
 * Every docling crate Cargo.lock pins (`docling` and `docling-*`), by name, with the source Cargo
 * builds it from. The converter is these crates together, and they need not share one source: a
 * `registry+` source is the crates.io release (with its checksum), a `git+` source names the
 * repository and the commit the crate is built from, as `[patch.crates-io]` in Cargo.toml
 * directs (verification.json gate converter-docling-pdf-font-run-patch). The receipt records
 * this instead of a crates.io version for a crate that is not built from crates.io.
 */
export function doclingPackages(lockText) {
  const field = (block, key) => new RegExp(`^${key} = "([^"]+)"\\r?$`, 'm').exec(block)?.[1] ?? null;
  return String(lockText ?? '')
    .split(/\r?\n\[\[package\]\]\r?\n/)
    .map((block) => ({ name: field(block, 'name'), version: field(block, 'version'), source: field(block, 'source'), checksum: field(block, 'checksum') }))
    .filter((entry) => entry.name !== null && /^docling(?:-|$)/.test(entry.name))
    .sort((left, right) => (left.name < right.name ? -1 : left.name > right.name ? 1 : 0));
}

/** The `build` block of the receipt. Throws when cargo's output does not show the docling crates. */
export function buildFacts({ harnessToml, workspaceToml, treeText, command }) {
  const declared = declaredDoclingFeatures(harnessToml, workspaceToml);
  const resolved = resolvedFeatures(treeText);
  for (const name of ['docling', 'docling-pdf']) {
    if (!resolved[name]) throw new Error(`cargo ${TREE_ARGS.join(' ')} did not list ${name}; the build features cannot be determined`);
  }
  return {
    command,
    command_note: 'written beside the buildRelease call in run.mjs: qualification/lib/cargo.mjs runs this and does not return its argv',
    build_features: declared.features,
    default_features: declared.default_features,
    declared_in: declared.declared_in,
    resolved_features: { docling: resolved.docling, 'docling-pdf': resolved['docling-pdf'] },
    pdfium_compiled_in:
      resolved.docling.includes('pdfium') || resolved['docling-pdf'].includes('pdfium') || 'pdfium-render' in resolved,
    resolved_by: `cargo ${TREE_ARGS.join(' ')}`,
  };
}
