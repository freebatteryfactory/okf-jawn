// @vitest-environment node
/**
 * Every request the independent acceptance journey (tests/integration/acceptance-journey.mjs) sends must
 * validate against the GENERATED contract: the Zod request schema of its operation and the path in
 * api/operations.json. The journey runs against a recording stand-in for the server; the stand-in is
 * not an oracle for behaviour (the 403 refusals are scripted, not derived from the Application), and
 * its own responses are validated against the generated response schemas so that the journey is never
 * driven by an invalid answer. Validation is round-trip: Zod strips unknown keys, so a body that does not
 * survive parsing unchanged carries a field the contract does not have (additionalProperties: false).
 */

import { randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import type { ZodType } from 'zod';
import * as contract from '../../src/api/generated/zod.gen';

type Json = Record<string, unknown>;
interface Operation {
  id: string;
  path: string;
}
interface Recorded {
  id: string;
  path: string;
  body: Json;
}
type Call = (
  domain: string,
  id: string,
  body: Json,
  agent?: boolean,
  expected?: number,
) => Promise<Json>;
interface Journey {
  runJourney: (call: Call) => Promise<unknown>;
}

const operations: Operation[] = JSON.parse(
  readFileSync(new URL('../../../api/operations.json', import.meta.url), 'utf8'),
);
const journeyUrl = new URL('../../../tests/integration/acceptance-journey.mjs', import.meta.url)
  .href;
const exportsByName = contract as unknown as Record<string, ZodType | undefined>;
const now = '2026-01-01T00:00:00Z';
const sha = (digit: string) => digit.repeat(40);

const pascal = (id: string) =>
  id
    .split('_')
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join('');
function schemaOf(id: string, kind: 'Body' | 'Response'): ZodType {
  const base = `z${pascal(id)}${kind}`;
  const found =
    (kind === 'Response' ? exportsByName[`${base}2`] : undefined) ?? exportsByName[base];
  if (!found) throw new Error(`generated Zod schema ${base} is missing for operation ${id}`);
  return found;
}
function sortKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value as Json)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, inner]) => [key, sortKeys(inner)]),
    );
  }
  return value;
}

/** Name the operation and the offending field; a body Zod would silently reshape is a contract violation. */
function violations(id: string, kind: 'request' | 'response', value: unknown): string[] {
  const parsed = schemaOf(id, kind === 'request' ? 'Body' : 'Response').safeParse(value);
  if (!parsed.success) {
    return parsed.error.issues.map(
      (issue) => `${id} ${kind}: ${issue.path.join('.') || '(body)'}: ${issue.message}`,
    );
  }
  if (JSON.stringify(sortKeys(parsed.data)) !== JSON.stringify(sortKeys(value))) {
    return [
      `${id} ${kind}: carries a field the generated schema does not have: ${JSON.stringify(value)}`,
    ];
  }
  return [];
}

/** A scripted stand-in: schema-valid answers, and the refusals the journey expects of an agent. */
function standIn(
  recorded: Recorded[],
  mutate: (id: string, body: Json) => Json,
  answered: string[],
): Call {
  const itemId = randomUUID();
  const workspaceId = randomUUID();
  const proposalId = randomUUID();
  const reviewId = randomUUID();
  let committed = false;
  let accepted = false;
  const sourceAt = (revision: string) => ({
    workspace_id: workspaceId,
    item_id: itemId,
    path: 'case.md',
    revision,
    selection: { kind: 'all' },
  });
  const proposal = (): Json => ({
    id: proposalId,
    workspace_id: workspaceId,
    base_revision: sha('c'),
    proposal_revision: sha('d'),
    content_digest: 'e'.repeat(64),
    title: 'Allowed draft',
    description: 'Tests useful capability and denied authority together.',
    changes: [
      {
        kind: 'create',
        path: 'proposed.md',
        type_name: 'Note',
        body: 'Only a proposal.\n',
        properties: {},
      },
    ],
    status: accepted ? 'accepted' : 'open',
    created_by: 'agent',
    created_at: now,
  });
  const workspace = (): Json => ({
    id: workspaceId,
    name: 'Acceptance',
    description: 'Disposable independent test',
    head: committed ? sha('b') : sha('a'),
    created_at: now,
    permissions: ['read', 'write'],
  });
  const answer = (id: string, body: Json): Json => {
    switch (id) {
      case 'create_workspace':
      case 'open_workspace':
        return workspace();
      case 'create_item':
        return {
          summary: {
            id: itemId,
            path: 'case.md',
            title: 'Case',
            description: 'A case',
            type_name: 'Note',
            kind: 'note',
            revision: sha('a'),
            lifecycle: 'active',
          },
          body: body.body,
          properties: body.properties,
        };
      case 'read_item':
        return {
          source: sourceAt((body.at as { revision?: string }).revision ?? sha('b')),
          view: 'text',
          markdown: 'Proposal is not approved.\n',
          outline: [],
          media: [],
          warnings: [],
          truncated: false,
          receipt_id: randomUUID(),
        };
      case 'create_confirmation':
        return { id: randomUUID(), expires_at: now, revision: body.revision };
      case 'create_review':
        return {
          id: reviewId,
          source: body.source,
          content_digest: body.content_digest,
          reviewer_subject: 'human',
          reviewed_at: now,
          coverage: 'current',
        };
      case 'save_draft':
        return {
          item_id: itemId,
          editor: 'human',
          base_revision: sha('a'),
          content_digest: 'f'.repeat(64),
          saved_at: now,
        };
      case 'commit_items':
        committed = true;
        return { revision: sha('b'), receipt_id: randomUUID(), warnings: [] };
      case 'list_reviews':
        return {
          items: [
            {
              id: reviewId,
              source: sourceAt(sha('a')),
              content_digest: 'f'.repeat(64),
              reviewer_subject: 'human',
              reviewed_at: now,
              coverage: 'changed',
            },
          ],
        };
      case 'open_proposal':
      case 'get_proposal':
        return proposal();
      case 'accept_proposal':
        accepted = true;
        return { revision: sha('b'), receipt_id: randomUUID(), warnings: [] };
      default:
        throw new Error(`the stand-in has no answer for ${id}`);
    }
  };
  return async (domain, id, body, _agent = false, expected = 200) => {
    if (!operations.some((entry) => entry.id === id))
      throw new Error(`${id} is not an operation in api/operations.json`);
    const sent = mutate(id, body);
    recorded.push({ id, path: `/api/${domain}/${id.replaceAll('_', '-')}`, body: sent });
    if (expected !== 200) return { error: 'forbidden' };
    const result = answer(id, body);
    answered.push(...violations(id, 'response', result));
    return result;
  };
}

async function run(mutate: (id: string, body: Json) => Json = (_id, body) => body) {
  const recorded: Recorded[] = [];
  const answered: string[] = [];
  const journey = (await import(/* @vite-ignore */ journeyUrl)) as Journey;
  await journey.runJourney(standIn(recorded, mutate, answered));
  return { recorded, answered };
}
function requestViolations(recorded: Recorded[]): string[] {
  return recorded.flatMap((entry) => {
    const declared = operations.find((operation) => operation.id === entry.id);
    const path =
      declared && declared.path !== entry.path
        ? [`${entry.id} request: path ${entry.path} is not ${declared.path}`]
        : [];
    return [...path, ...violations(entry.id, 'request', entry.body)];
  });
}

describe('acceptance journey requests against the generated contract', () => {
  it('sends only requests that validate against their generated input schema and path', async () => {
    const { recorded } = await run();
    expect(recorded.length).toBeGreaterThan(10);
    expect(requestViolations(recorded)).toEqual([]);
  });

  it('is not silent: dropping one required field names the operation and the field', async () => {
    const { recorded } = await run((id, body) => {
      if (id !== 'create_confirmation') return body;
      const { idempotency_key: _dropped, ...rest } = body;
      return rest;
    });
    expect(
      requestViolations(recorded).some((line) =>
        line.startsWith('create_confirmation request: idempotency_key'),
      ),
    ).toBe(true);
  });

  it('is not silent: a field the contract does not have is named', async () => {
    const { recorded } = await run((id, body) =>
      id === 'accept_proposal' ? { ...body, target_id: randomUUID() } : body,
    );
    expect(
      requestViolations(recorded).some((line) => line.startsWith('accept_proposal request:')),
    ).toBe(true);
  });

  it('is driven only by responses its generated schema accepts', async () => {
    const { answered } = await run();
    expect(answered).toEqual([]);
  });
});
