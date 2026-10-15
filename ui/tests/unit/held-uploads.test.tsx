/**
 * The uploads the service holds (ruling 3; nothing expires): listed on the import page, included
 * in an import, and discarded. And an import of more uploads than one request names, started in
 * parts (ruling 15).
 */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import {
  act,
  cleanup,
  configure,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createQueryClient } from '../../src/app/query-client';
import { heldUploadsChanged } from '../../src/features/imports/held-uploads';
import { ImportStart } from '../../src/features/imports/ImportStart';
import {
  beginImport,
  importAttempt,
  MAX_IMPORT_UPLOADS,
} from '../../src/features/imports/import-start';
import { resetUploads } from '../../src/features/imports/uploading';
import { fail, type Handler, mockApi, ok } from './support/api';
import {
  accepted,
  DISCARD_UPLOAD,
  fileOf,
  JOB,
  JOB_ID,
  jobOf,
  LIST_UPLOADS,
  START,
  slotOf,
  uploadService,
} from './support/imports';
import { mockTransfers } from './support/transfers';
import { explorerHandlers, HEAD, renderAt, WS, writableWorkspace } from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  act(() => resetUploads());
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/** The import page over a service that holds `held` already. */
async function openWith(
  held: ReturnType<typeof slotOf>[],
  overrides: Record<string, Handler> = {},
) {
  const service = uploadService();
  // Copies: a test changes the slots its service holds, never another test's.
  service.slots.push(...held.map((slot) => ({ ...slot })));
  const api = mockApi(
    explorerHandlers({ [OPEN]: () => ok(writableWorkspace), ...service.handlers, ...overrides }),
  );
  mockTransfers(service.transfer);
  const { queryClient } = renderAt(`/w/${WS}/import`);
  await screen.findByRole('heading', { name: 'Import files' });
  return { api, service, queryClient };
}

const section = () => screen.getByRole('region', { name: 'Uploads the service holds' });
const listNamed = (name: RegExp) => within(section()).getByRole('list', { name });
const waitingList = () => listNamed(/^Uploaded and not imported/);
const unfinishedList = () => listNamed(/^Not finished/);
const importButton = () =>
  screen.getByRole('button', { name: /^Import \d[\d,]* uploaded files?$/ });

const report = slotOf(1, 'report.pdf', 'abc', { relativePath: 'papers' });
const older = slotOf(2, 'older.txt', 'abcd');
const half = slotOf(3, 'half.csv', 'ab', { complete: false, expected: 10 });
const taken = slotOf(4, 'taken.txt', 'x', { consumedBy: JOB_ID });

describe('the uploads the service holds', () => {
  it('lists those not imported and those not finished, newest first, and says nothing expires', async () => {
    const { api } = await openWith([report, older, half, taken]);
    await within(section()).findByRole('list', { name: /^Uploaded and not imported/ });
    expect(section().textContent).toContain(
      'The service keeps an upload until it is imported or discarded: nothing is removed by itself.',
    );
    const waiting = within(waitingList())
      .getAllByRole('listitem')
      .map((row) => row.textContent ?? '');
    // Newest first, as the service lists them; the one a job took is not among them.
    expect(
      within(waitingList())
        .getAllByRole('listitem')
        .map((row) => row.querySelector('bdi')?.textContent),
    ).toEqual(['older.txt', 'papers/report.pdf']);
    expect(waiting.at(1)).toContain('3 bytes');
    const unfinished = within(unfinishedList()).getByRole('listitem').textContent ?? '';
    expect(unfinished).toContain('half.csv');
    expect(unfinished).toContain('2 bytes of 10 bytes sent');
    expect(api.callsTo(LIST_UPLOADS).map((body) => body.state)).toEqual(['waiting', 'incomplete']);
    expect(api.violations).toEqual([]);
  });

  it('does not list again an upload this page’s own list holds', async () => {
    const { api, service, queryClient } = await openWith([older]);
    const opened = vi.spyOn(HTMLInputElement.prototype, 'click').mockImplementation(function open(
      this: HTMLInputElement,
    ) {
      Object.defineProperty(this, 'files', { configurable: true, value: [fileOf('a.txt', 'abc')] });
      fireEvent.change(this);
    });
    fireEvent.click(screen.getByRole('button', { name: 'Choose files…' }));
    opened.mockRestore();
    fireEvent.click(screen.getByRole('button', { name: 'Upload 1 file' }));
    await waitFor(() => expect(importButton().textContent).toBe('Import 1 uploaded file'));
    // Listed again once the upload finished: the service holds it, and so does the page's list.
    const asked = api.callsTo(LIST_UPLOADS).length;
    act(() => heldUploadsChanged(queryClient, WS));
    expect(service.slots.filter((slot) => slot.complete)).toHaveLength(2);
    // Both lists asked again, and the answer drawn: it holds the page's own upload too.
    await waitFor(() => expect(api.callsTo(LIST_UPLOADS).length).toBe(asked + 2));
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    await waitFor(() =>
      expect(
        within(waitingList())
          .getAllByRole('listitem')
          .map((row) => row.textContent),
      ).toEqual([expect.stringContaining('older.txt')]),
    );
  });

  it('includes one in the import when the person ticks it, and lists it no more once a job took it', async () => {
    const { api, service } = await openWith([report, older], {
      [START]: (body) => {
        for (const id of body.upload_ids as string[]) {
          const slot = service.slots.find((candidate) => candidate.id === id);
          if (slot !== undefined) slot.consumedBy = JOB_ID;
        }
        return accepted(jobOf('queued'));
      },
      [JOB]: () => ok(jobOf('queued')),
    });
    await within(section()).findByRole('list', { name: /^Uploaded and not imported/ });
    expect(importButton().textContent).toBe('Import 0 uploaded files');
    const tick = within(section()).getByRole('checkbox', {
      name: 'Include “papers/report.pdf” in the import',
    });
    expect(tick).toHaveProperty('checked', false);
    fireEvent.click(tick);
    expect(importButton().textContent).toBe('Import 1 uploaded file');
    fireEvent.click(importButton());
    await waitFor(() => expect(api.callsTo(START)).toHaveLength(1));
    expect(api.callsTo(START).at(0)?.upload_ids).toEqual([report.id]);
    await waitFor(() =>
      expect(
        within(waitingList())
          .getAllByRole('listitem')
          .map((row) => row.textContent),
      ).toEqual([expect.stringContaining('older.txt')]),
    );
    expect(importButton().textContent).toBe('Import 0 uploaded files');
    expect(api.violations).toEqual([]);
  });

  it('discards one after asking, under one key, and focus goes to the row now in its place', async () => {
    const { api, service } = await openWith([report, older]);
    await within(section()).findByRole('list', { name: /^Uploaded and not imported/ });
    const discard = within(section()).getByRole('button', { name: 'Discard “older.txt”' });
    act(() => discard.focus());
    fireEvent.click(discard);
    const question = within(section()).getByRole('group', { name: 'Discard “older.txt”?' });
    const keep = within(question).getByRole('button', { name: 'Keep the upload' });
    expect(document.activeElement).toBe(keep);
    fireEvent.click(keep);
    expect(document.activeElement).toBe(
      within(section()).getByRole('button', { name: 'Discard “older.txt”' }),
    );
    expect(api.callsTo(DISCARD_UPLOAD)).toEqual([]);
    fireEvent.click(within(section()).getByRole('button', { name: 'Discard “older.txt”' }));
    const confirm = within(section()).getByRole('button', { name: 'Discard it' });
    act(() => confirm.focus());
    fireEvent.click(confirm);
    await waitFor(() =>
      expect(within(section()).getByRole('status').textContent).toBe(
        'The upload of “older.txt” was discarded: the service no longer holds it.',
      ),
    );
    expect(api.callsTo(DISCARD_UPLOAD)).toEqual([
      { workspace_id: WS, upload_id: older.id, idempotency_key: expect.stringMatching(UUID) },
    ]);
    expect(service.slots.map((slot) => slot.id)).toEqual([report.id]);
    // Its row left with focus in it: focus is on the row now in its place.
    expect(document.activeElement).toBe(
      within(section()).getByRole('checkbox', {
        name: 'Include “papers/report.pdf” in the import',
      }),
    );
    expect(api.violations).toEqual([]);
  });

  it('keeps the row when the discard fails, and repeats it under the same key', async () => {
    let failing = true;
    const { api } = await openWith([half], {
      [DISCARD_UPLOAD]: (body) => {
        if (failing) return fail(503, 'unavailable', 'busy');
        return ok({
          id: half.id,
          workspace_id: WS,
          upload_path: `/api/workspaces/${WS}/uploads/${half.id}/content`,
          received_bytes: '2',
          complete: false,
          ...(body.upload_id === half.id ? {} : { filename: 'unexpected' }),
        });
      },
    });
    await within(section()).findByRole('list', { name: /^Not finished/ });
    fireEvent.click(within(section()).getByRole('button', { name: 'Discard “half.csv”' }));
    fireEvent.click(within(section()).getByRole('button', { name: 'Discard it' }));
    expect(
      await within(section()).findByText(
        'Discarding “half.csv”: The service is temporarily unavailable.',
      ),
    ).toBeDefined();
    expect(within(unfinishedList()).getByRole('listitem').textContent).toContain('half.csv');
    failing = false;
    fireEvent.click(within(section()).getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(api.callsTo(DISCARD_UPLOAD)).toHaveLength(2));
    const [first, second] = api.callsTo(DISCARD_UPLOAD);
    expect(second).toEqual(first);
  });

  it('reads "not available in this build" where the listing is not built, never "none"', async () => {
    await openWith([], { [LIST_UPLOADS]: () => fail(501, 'not_implemented', 'not built') });
    expect(
      await within(section()).findByText(
        'Listing the uploads that are not imported: Not available in this build.',
      ),
    ).toBeDefined();
    expect(
      within(section()).getByText(
        'Listing the uploads that were not finished: Not available in this build.',
      ),
    ).toBeDefined();
    expect(within(section()).queryByText('None.')).toBeNull();
  });
});

describe('an import of more uploads than one request names', () => {
  const ids = (n: number) =>
    Array.from({ length: n }, (_, at) => `99999999-9999-4999-8999-${String(at).padStart(12, '0')}`);

  it('is started in parts, in order, each within the bound and under its own key', async () => {
    let started = 0;
    const api = mockApi({
      [START]: () => {
        started += 1;
        return accepted(jobOf('queued', { id: `88888888-8888-4888-8888-00000000000${started}` }));
      },
    });
    const all = ids(MAX_IMPORT_UPLOADS + 2);
    const attempt = importAttempt(null, all, 'papers', HEAD);
    const jobs = await beginImport(new QueryClient(), WS, attempt, HEAD);
    const asked = api.callsTo(START);
    expect(asked.map((body) => (body.upload_ids as string[]).length)).toEqual([
      MAX_IMPORT_UPLOADS,
      2,
    ]);
    expect(asked.flatMap((body) => body.upload_ids as string[])).toEqual(all);
    expect(new Set(asked.map((body) => body.idempotency_key)).size).toBe(2);
    expect(jobs).toHaveLength(2);
    expect(api.violations).toEqual([]);
  });

  it('starts again from the part that failed: the parts before it are not asked again', async () => {
    let answers = 0;
    const api = mockApi({
      [START]: () => {
        answers += 1;
        return answers === 2 ? fail(503, 'unavailable', 'busy') : accepted(jobOf('queued'));
      },
    });
    const attempt = importAttempt(null, ids(MAX_IMPORT_UPLOADS + 1), '', HEAD);
    const client = new QueryClient();
    await expect(beginImport(client, WS, attempt, HEAD)).rejects.toBeDefined();
    expect(api.callsTo(START)).toHaveLength(2);
    await beginImport(client, WS, attempt, HEAD);
    const asked = api.callsTo(START);
    expect(asked).toHaveLength(3);
    // The retry is the failed part again, under its key.
    expect(asked.at(2)).toEqual(asked.at(1));
    expect(asked.at(2)?.upload_ids).toHaveLength(1);
  });

  it('says before the press that the files are imported in parts, each a job', () => {
    mockApi({});
    const show = (included: string[]) =>
      render(
        <QueryClientProvider client={createQueryClient()}>
          <ImportStart workspaceId={WS} head={HEAD} entries={[]} included={included} />
        </QueryClientProvider>,
      );
    const view = show(ids(MAX_IMPORT_UPLOADS));
    expect(screen.queryByText(/imported in \d+ parts/)).toBeNull();
    view.unmount();
    show(ids(MAX_IMPORT_UPLOADS + 1));
    expect(
      screen.getByText(
        'One import takes at most 5,000 files, so these 5,001 are imported in 2 parts, one after another. Each part is a job of its own.',
      ),
    ).toBeDefined();
    expect(importButton().textContent).toBe('Import 5001 uploaded files');
  });
});
