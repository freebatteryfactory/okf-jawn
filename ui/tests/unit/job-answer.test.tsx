/**
 * A job the service answers for (`start_import`) is asked for once after its answer, so events
 * that went out before the answer was read are not missed.
 */
import { act, cleanup, configure, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Job } from '../../src/api/generated/types.gen';
import { resetUploads } from '../../src/features/imports/uploading';
import { mockApi, ok } from './support/api';
import {
  accepted,
  eventOf,
  fileOf,
  IMPORTED,
  JOB,
  jobOf,
  mockEvents,
  START,
  uploadService,
} from './support/imports';
import { mockTransfers } from './support/transfers';
import { explorerHandlers, NOTE, renderAt, WS, writableWorkspace } from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  act(() => resetUploads());
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('a job started while the event stream is open', () => {
  it('is asked for once after its answer, not left at "Waiting to start" until the slow clock', async () => {
    const service = uploadService();
    const FIRST = '88888888-8888-4888-8888-888888888881';
    const SECOND = '88888888-8888-4888-8888-888888888882';
    const jobs = new Map<string, Job>();
    let starts = 0;
    const events = { current: null as ReturnType<typeof mockEvents> | null };
    const api = mockApi(
      explorerHandlers({
        [OPEN]: () => ok(writableWorkspace),
        ...service.handlers,
        [START]: async () => {
          starts += 1;
          const id = starts === 1 ? FIRST : SECOND;
          jobs.set(id, jobOf('succeeded', { id, item_ids: [NOTE], revision: IMPORTED }));
          if (starts === 2) {
            // Its events go out on the open stream before the answer is read.
            events.current?.streams
              .at(-1)
              ?.send('41', eventOf('job_updated', '41', { job_id: id }));
            events.current?.streams.at(-1)?.send('42', eventOf('imported', '42', { job_id: id }));
            await new Promise((resolve) => setTimeout(resolve, 150));
          }
          return accepted(jobOf('queued', { id }));
        },
        [JOB]: (body) => ok(jobs.get(String(body.job_id)) ?? jobOf('queued')),
      }),
    );
    events.current = mockEvents();
    mockTransfers(service.transfer);
    renderAt(`/w/${WS}/import`);
    await screen.findByRole('heading', { name: 'Import files' });
    const importButton = () => screen.getByRole('button', { name: /^Import \d+ uploaded files?$/ });
    const row = (id: string) => document.querySelector<HTMLElement>(`li[data-job="${id}"]`);
    const importOne = async (file: File) => {
      const opened: HTMLInputElement[] = [];
      const click = vi.spyOn(HTMLInputElement.prototype, 'click').mockImplementation(function open(
        this: HTMLInputElement,
      ) {
        opened.push(this);
      });
      fireEvent.click(screen.getByRole('button', { name: 'Choose files…' }));
      click.mockRestore();
      const picker = opened.at(0) as HTMLInputElement;
      Object.defineProperty(picker, 'files', { configurable: true, value: [file] });
      fireEvent.change(picker);
      fireEvent.click(screen.getByRole('button', { name: /^Upload \d+ files?$/ }));
      await waitFor(() => expect(importButton().textContent).toBe('Import 1 uploaded file'));
      fireEvent.click(importButton());
    };
    await importOne(fileOf('a.txt', 'abc'));
    await waitFor(() => expect(row(FIRST)?.textContent).toContain('Finished.'));
    expect(events.current.streams).toHaveLength(1);
    await importOne(fileOf('b.txt', 'abcd'));
    await waitFor(() => expect(row(SECOND)).not.toBeNull());
    // Far inside the slow clock (30 s): the answer itself has to cause the question.
    await new Promise((resolve) => setTimeout(resolve, 4_000));
    expect(row(SECOND)?.textContent).toContain('Finished.');
    expect(api.callsTo(JOB).filter((body) => body.job_id === SECOND)).toHaveLength(1);
  }, 30_000);
});
