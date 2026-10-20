/**
 * What stays offered in an archived workspace: the operations the service runs there
 * (`ALLOWED_WHILE_ARCHIVED`) are held to the generated metadata in `api/operations.json`, both
 * ways, and an action of one of them is not stopped by the archive. Every other action of an
 * archived workspace still says why it cannot run, as before.
 */
import { describe, expect, it } from 'vitest';
import operations from '../../../api/operations.json';
import {
  ALLOWED_WHILE_ARCHIVED,
  allowed,
  allowedOperation,
  HUMAN_SESSION_ONLY,
  humanSessionWords,
  LOCAL_OWNER_ONLY,
  LOCAL_OWNER_WORDS,
  type Rights,
} from '../../src/features/explorer/rights';

interface Operation {
  id: string;
  allowed_while_archived: boolean;
}

const all = (operations as { operations?: Operation[] }).operations ?? (operations as Operation[]);

const ARCHIVED_AT = '2026-01-02T08:00:00.000Z';
const writer: Rights = { permissions: ['read', 'write'], archivedAt: null };
const archivedWriter: Rights = { ...writer, archivedAt: ARCHIVED_AT };
const archivedReader: Rights = { permissions: ['read'], archivedAt: ARCHIVED_AT };

describe('the operations the service runs in an archived workspace', () => {
  it('are the generated metadata’s, and nothing else', () => {
    const fromMetadata = all
      .filter((operation) => operation.allowed_while_archived)
      .map((operation) => operation.id)
      .sort();
    expect(fromMetadata.length).toBeGreaterThan(0);
    expect([...ALLOWED_WHILE_ARCHIVED].sort()).toEqual(fromMetadata);
  });

  it('include both exports and the reading of a View, and not an import', () => {
    for (const name of ['export_workspace', 'export_view', 'get_view', 'present_view'] as const) {
      expect(ALLOWED_WHILE_ARCHIVED.has(name)).toBe(true);
    }
    expect(ALLOWED_WHILE_ARCHIVED.has('start_import')).toBe(false);
  });
});

describe('whether an operation can run here', () => {
  it('is not stopped by the archive for an operation the service runs there', () => {
    expect(allowedOperation(archivedWriter, 'export_workspace', 'write')).toBe('yes');
    expect(allowedOperation(archivedWriter, 'export_view', 'write')).toBe('yes');
  });

  it('is stopped by the archive for every other operation', () => {
    expect(allowedOperation(archivedWriter, 'start_import', 'write')).toBe('archived');
    expect(allowedOperation(archivedWriter, 'restore_items', 'write')).toBe('archived');
  });

  it('still needs the permission, archived or not', () => {
    expect(allowedOperation(archivedReader, 'export_workspace', 'write')).toBe('no-permission');
    expect(allowedOperation({ ...archivedReader, archivedAt: null }, 'export_view', 'write')).toBe(
      'no-permission',
    );
  });

  it('runs where nothing stops it', () => {
    expect(allowedOperation(writer, 'export_workspace', 'write')).toBe('yes');
    expect(allowedOperation(writer, 'start_import', 'write')).toBe('yes');
  });

  it('leaves `allowed` as it was: an archived workspace stops any action it is asked about', () => {
    expect(allowed(archivedWriter, 'write')).toBe('archived');
    expect(allowed(archivedReader, 'write')).toBe('no-permission');
    expect(allowed(writer, 'write')).toBe('yes');
  });
});

interface Guarded {
  id: string;
  human_session_only: boolean;
  local_owner_only: boolean;
}

const guarded = all as unknown as Guarded[];

/** The ids the generated metadata marks with `flag`, sorted. */
function marked(flag: 'human_session_only' | 'local_owner_only'): string[] {
  return guarded
    .filter((operation) => operation[flag])
    .map((operation) => operation.id)
    .sort();
}

describe('the operations only a person in their own browser session, or the local owner, may ask (C2)', () => {
  it('are the generated metadata’s, both ways', () => {
    expect(marked('human_session_only').length).toBeGreaterThan(0);
    expect([...HUMAN_SESSION_ONLY].sort()).toEqual(marked('human_session_only'));
    expect(marked('local_owner_only').length).toBeGreaterThan(0);
    expect([...LOCAL_OWNER_ONLY].sort()).toEqual(marked('local_owner_only'));
  });

  it('are said in plain words, with what is allowed instead', () => {
    expect(humanSessionWords('Purging this workspace')).toBe(
      'Purging this workspace is done only by an administrator in their own browser session: an agent, a connector or a service cannot ask for it.',
    );
    expect(LOCAL_OWNER_WORDS).toBe(
      'Connectors are issued only on a local installation. Hosted agents connect through WorkOS Connect.',
    );
  });
});
