/**
 * Who may cancel or retry a job (ruling 8): the table of the operations that start jobs is held to
 * the generated metadata in `api/operations.json`, both ways, and the rule reads it.
 */
import { describe, expect, it } from 'vitest';
import operations from '../../../api/operations.json';
import type { Job, JobKind } from '../../src/api/generated/types.gen';
import { mayControl, STARTED_BY } from '../../src/features/jobs/job-control';
import { jobOf } from './support/imports';

interface Operation {
  id: string;
  permission: string;
  starts_job: string | null;
  human_session_only: boolean;
}

const all = (operations as { operations?: Operation[] }).operations ?? (operations as Operation[]);

describe('the operations that start jobs', () => {
  it('are the generated metadata’s, with their permission and session rule, and nothing else', () => {
    const fromMetadata = Object.fromEntries(
      all
        .filter((operation) => operation.starts_job !== null)
        .map((operation) => [
          operation.starts_job,
          {
            operation: operation.id,
            permission: operation.permission,
            humanSessionOnly: operation.human_session_only,
          },
        ]),
    );
    expect(Object.keys(fromMetadata).length).toBeGreaterThan(0);
    expect(STARTED_BY).toEqual(fromMetadata);
  });
});

describe('whether a person may control a job', () => {
  const of = (kind: JobKind): Job => jobOf('running', { kind });

  it('needs the permission of the operation that started it', () => {
    expect(mayControl(of('import'), ['read', 'write'], 'browser_session')).toBe(true);
    expect(mayControl(of('import'), ['read'], 'browser_session')).toBe(false);
    expect(mayControl(of('purge_workspace'), ['read', 'write'], 'browser_session')).toBe(false);
    expect(mayControl(of('purge_workspace'), ['read', 'admin'], 'browser_session')).toBe(true);
  });

  it('needs a human session where that operation needs one, and takes this page for one until told', () => {
    const admin = ['read', 'write', 'admin'] as const;
    expect(mayControl(of('backup_workspace'), admin, 'mcp_delegation')).toBe(false);
    expect(mayControl(of('backup_workspace'), admin, 'service')).toBe(false);
    expect(mayControl(of('backup_workspace'), admin, 'local_owner')).toBe(true);
    expect(mayControl(of('backup_workspace'), admin, null)).toBe(true);
    // An operation that needs no human session: any route.
    expect(mayControl(of('rebuild_index'), admin, 'service')).toBe(true);
  });

  it('is no one’s for a kind this build does not know', () => {
    const unknown = { ...of('import'), kind: 'compact' as JobKind };
    expect(mayControl(unknown, ['read', 'write', 'admin'], 'browser_session')).toBe(false);
  });
});
