/**
 * A finished job's download (B4): a job the service answered `succeeded` with an artifact offers it
 * as a plain link (`<a href download>`) to the artifact's own download path, with its kind, size and
 * SHA-256 beside it. It is offered by the artifact's own rule (`ArtifactKind::download_permission`
 * and `download_routes` in `crates/contract/src/workspace.rs`): an export or a View export needs
 * `read`; a backup needs `admin`, in a human session. Where it is not offered, the row says why.
 */
import { act, cleanup, configure, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type {
  AccessRoute,
  ArtifactKind,
  Job,
  Permission,
  SessionResponse,
} from '../../src/api/generated/types.gen';
import { sessionOptions } from '../../src/features/imports/limits';
import { downloadOf } from '../../src/features/jobs/download';
import { fromNewerService, mockApi, ok } from './support/api';
import { ARCHIVE_SHA, ARTIFACT, artifactOf, EXPORT_JOB, exportJobOf } from './support/exports';
import { JOBS } from './support/imports';
import { explorerHandlers, renderAt, WS, writableWorkspace } from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const WORKSPACE_PATH = `/api/workspaces/${WS}/artifacts/${ARTIFACT}`;

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/** The Jobs page listing `jobs`, for a person holding `permissions`. */
async function openJobs(jobs: Job[], permissions: Permission[] = ['read']) {
  const api = mockApi(
    explorerHandlers({
      [OPEN]: () => ok({ ...writableWorkspace, permissions }),
      [JOBS]: () => ok({ items: jobs, next_cursor: null }),
    }),
  );
  const app = renderAt(`/w/${WS}/jobs`);
  await screen.findByRole('heading', { name: 'Jobs' });
  return { api, ...app };
}

const rowOf = (id = EXPORT_JOB) => {
  const row = document.querySelector<HTMLElement>(`li[data-job="${id}"]`);
  if (row === null) throw new Error(`no row for job ${id}`);
  return row;
};
const listed = (id = EXPORT_JOB) => vi.waitFor(() => rowOf(id), { timeout: 10_000 });
const linkIn = (row: HTMLElement) => within(row).queryByRole('link', { name: /^Download / });

const sessionOf = (route: AccessRoute): SessionResponse => ({
  csrf_required: false,
  principal: { subject: 'ada', tenant_id: 'local', route },
  limits: { upload_bytes: '1024', request_body_bytes: 1_048_576, item_text_bytes: 8 },
});

/** A finished job of the kind that makes `kind`, with its artifact. */
function finished(kind: ArtifactKind, extra: Partial<Job> = {}): Job {
  const jobKind = {
    export: 'export_workspace',
    view_export: 'export_view',
    workspace_backup: 'backup_workspace',
    installation_backup: 'backup_installation',
  } as const;
  return exportJobOf('succeeded', { kind: jobKind[kind], artifact: artifactOf(kind), ...extra });
}

describe('the download of a finished export', () => {
  it('is a plain link to its download path, with its kind, size and SHA-256', async () => {
    const { api } = await openJobs([finished('export')]);
    await listed();
    const link = await within(rowOf()).findByRole('link', {
      name: 'Download the export of the workspace',
    });
    expect(link.getAttribute('href')).toBe(WORKSPACE_PATH);
    expect(link.hasAttribute('download')).toBe(true);
    expect(rowOf().textContent).toContain(
      'It produced an export of the workspace: 2,048 bytes (2.0 KB).',
    );
    expect(rowOf().textContent).toContain(`SHA-256: ${ARCHIVE_SHA}`);
    // Nothing of the archive is asked through the transport.
    expect(api.calls.filter((call) => call.path.includes('/artifacts/'))).toEqual([]);
    expect(api.violations).toEqual([]);
  });

  it('is offered for an export of a View by the same rule', async () => {
    await openJobs([finished('view_export')]);
    await listed();
    const link = await within(rowOf()).findByRole('link', {
      name: 'Download the export of the View',
    });
    expect(link.getAttribute('href')).toBe(WORKSPACE_PATH);
    expect(rowOf().textContent).toContain('It produced an export of a View: 2,048 bytes (2.0 KB).');
  });

  it('says the exact size of a large archive, and about how large it is', async () => {
    await openJobs([
      finished('export', { artifact: artifactOf('export', { size: '123456789012' }) }),
    ]);
    await listed();
    expect(rowOf().textContent).toContain(
      'It produced an export of the workspace: 123,456,789,012 bytes (115 GB).',
    );
  });

  it('needs permission to read the workspace, and says so where it is not offered', async () => {
    await openJobs(
      [finished('export'), finished('view_export', { id: 'bbbbbbbb-7777-4777-8777-777777777777' })],
      [],
    );
    await listed();
    expect(linkIn(rowOf())).toBeNull();
    expect(rowOf().textContent).toContain(
      'Downloading an export of the workspace needs permission to read this workspace, which you do not have.',
    );
    expect(linkIn(rowOf('bbbbbbbb-7777-4777-8777-777777777777'))).toBeNull();
  });

  it('is offered to a service identity, and not to an agent connected through MCP', async () => {
    const { queryClient } = await openJobs([finished('export')]);
    await listed();
    act(() => {
      queryClient.setQueryData(sessionOptions().queryKey, sessionOf('mcp_delegation'));
    });
    await vi.waitFor(() => expect(linkIn(rowOf())).toBeNull());
    expect(rowOf().textContent).toContain(
      'Only a signed-in browser, the local owner of this installation or a service identity may download an export of the workspace; this session is an agent connected through MCP.',
    );
    act(() => {
      queryClient.setQueryData(sessionOptions().queryKey, sessionOf('service'));
    });
    await vi.waitFor(() => expect(linkIn(rowOf())).not.toBeNull());
  });
});

describe('the download of a finished backup', () => {
  it('is offered to an administrator, in a browser', async () => {
    const { queryClient } = await openJobs([finished('workspace_backup')], ['read', 'admin']);
    await listed();
    // The session not read yet: this page, a browser, is taken for a human session.
    const link = await within(rowOf()).findByRole('link', {
      name: 'Download the backup of the workspace',
    });
    expect(link.getAttribute('href')).toBe(WORKSPACE_PATH);
    expect(link.hasAttribute('download')).toBe(true);
    act(() => {
      queryClient.setQueryData(sessionOptions().queryKey, sessionOf('local_owner'));
    });
    expect(linkIn(rowOf())).not.toBeNull();
  });

  it('is not offered to a person who is no administrator, and says why', async () => {
    await openJobs([finished('workspace_backup')], ['read', 'write']);
    await listed();
    await vi.waitFor(() =>
      expect(rowOf().textContent).toContain('It produced a backup of the workspace'),
    );
    expect(linkIn(rowOf())).toBeNull();
    expect(rowOf().textContent).toContain(
      'Downloading a backup of the workspace needs administrator rights here, which you do not have.',
    );
  });

  it('is offered only in a human session: never to a service identity or an agent', async () => {
    const { queryClient } = await openJobs([finished('workspace_backup')], ['read', 'admin']);
    await listed();
    const said = (session: string) =>
      `Only a signed-in browser or the local owner of this installation may download a backup of the workspace; this session is ${session}.`;
    for (const [route, session] of [
      ['service', 'a service identity'],
      ['mcp_delegation', 'an agent connected through MCP'],
    ] as const) {
      act(() => {
        queryClient.setQueryData(sessionOptions().queryKey, sessionOf(route));
      });
      await vi.waitFor(() => expect(rowOf().textContent).toContain(said(session)));
      expect(linkIn(rowOf())).toBeNull();
    }
    act(() => {
      queryClient.setQueryData(sessionOptions().queryKey, sessionOf('browser_session'));
    });
    await vi.waitFor(() => expect(linkIn(rowOf())).not.toBeNull());
  });

  it('leads to the installation’s own path for a backup of the installation', async () => {
    await openJobs([finished('installation_backup')], ['read', 'admin']);
    await listed();
    const link = await within(rowOf()).findByRole('link', {
      name: 'Download the backup of the installation',
    });
    expect(link.getAttribute('href')).toBe(`/api/artifacts/${ARTIFACT}`);
  });
});

describe('what gets no link', () => {
  it('a job the service did not answer as succeeded, whatever it carries', async () => {
    const ids = ['a', 'b', 'c', 'd'].map(
      (letter) => `${letter.repeat(8)}-7777-4777-8777-777777777777`,
    );
    const states = ['queued', 'running', 'failed', 'cancelled'] as const;
    await openJobs(
      states.map((state, index) =>
        exportJobOf(state, { id: ids[index] ?? '', artifact: artifactOf('export') }),
      ),
    );
    for (const id of ids) await listed(id);
    for (const id of ids) {
      expect(linkIn(rowOf(id))).toBeNull();
      expect(rowOf(id).textContent).not.toContain('SHA-256');
    }
  });

  it('a finished job that names no artifact', async () => {
    await openJobs([exportJobOf('succeeded')]);
    await listed();
    expect(rowOf().textContent).toContain('Finished.');
    expect(linkIn(rowOf())).toBeNull();
  });

  it('an artifact of a kind this page does not know, and it says so', async () => {
    const { api } = await openJobs([
      finished('export', {
        artifact: artifactOf('export', { kind: fromNewerService('audit_trail') }),
      }),
    ]);
    await listed();
    expect(linkIn(rowOf())).toBeNull();
    expect(rowOf().textContent).toContain(
      'This page does not know who may download something of this kind, so it offers no link to it.',
    );
    expect(api.violations).toHaveLength(1);
  });

  it('a download path outside this application', async () => {
    const elsewhere = [
      'https://elsewhere.example/a.zip',
      '//elsewhere.example/a.zip',
      '/\\elsewhere.example/a.zip',
    ];
    const ids = ['a', 'b', 'c'].map((letter) => `${letter.repeat(8)}-7777-4777-8777-777777777777`);
    await openJobs(
      elsewhere.map((path, index) =>
        finished('export', {
          id: ids[index] ?? '',
          artifact: artifactOf('export', { download_path: path }),
        }),
      ),
    );
    for (const id of ids) await listed(id);
    for (const id of ids) {
      expect(linkIn(rowOf(id))).toBeNull();
      expect(rowOf(id).textContent).toContain(
        'The service named a download path outside this application',
      );
    }
  });
});

describe('where the link leads', () => {
  it('is the download path under the base the API is reached at', () => {
    const artifact = artifactOf('export');
    expect(downloadOf(artifact, ['read'], null, '')).toEqual({
      kind: 'offered',
      href: WORKSPACE_PATH,
    });
    expect(downloadOf(artifact, ['read'], null, '/okf')).toEqual({
      kind: 'offered',
      href: `/okf${WORKSPACE_PATH}`,
    });
    expect(
      downloadOf(artifact, ['read'], 'browser_session', `${window.location.origin}/okf`),
    ).toEqual({
      kind: 'offered',
      href: `${window.location.origin}/okf${WORKSPACE_PATH}`,
    });
  });
});
