/**
 * Reconvert a source (UI M4a, ruling A4): `redigest_item` with the settings its conversion
 * recorded (`SourceAppearance.extraction.converter.settings`, the only place the wire reports
 * them), of the whole file, or of only the pages not converted when the conversion names some
 * (`ConversionOutcome::Partial.coverage.not_converted`). The 202 job goes into the cache
 * (`rememberJob`), is followed in the section until it ends, and the version it made can then be
 * shown. Refusals are said in words, and a reader without `write` is told why.
 */
import { cleanup, configure, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { getJobOptions } from '../../src/api/generated/@tanstack/react-query.gen';
import type { Job } from '../../src/api/generated/types.gen';
import { fail, type Json, mockApi } from './support/api';
import { JOB } from './support/imports';
import {
  CONVERTER,
  correctionsService,
  extractionOf,
  PARTIAL,
  READ,
  REDIGEST_JOB,
  redigestJob,
  SETTINGS,
  type ServiceOptions,
  UUID,
} from './support/m4a-corrections';
import { REDIGEST_ITEM } from './support/reader';
import { HEAD, renderAt, SOURCE, WS } from './support/workspace';

configure({ asyncUtilTimeout: 10_000 });
// A test waits for several answers in turn, each up to that long.
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const short = (revision: string) => revision.slice(0, 10);
const RECONVERTED = '5'.repeat(40);

async function open(options: ServiceOptions = {}) {
  const service = correctionsService(options);
  const api = mockApi(service.handlers);
  const rendered = renderAt(`/w/${WS}/i/${SOURCE}`);
  const section = await screen.findByRole('region', { name: 'Corrections' });
  const group = await within(section).findByRole('group', { name: 'Reconvert' });
  return { api, service, section, group, ...rendered };
}

const asked = (api: ReturnType<typeof mockApi>) => api.callsTo(REDIGEST_ITEM);
const describedBy = (element: HTMLElement) =>
  (element.getAttribute('aria-describedby') ?? '')
    .split(' ')
    .map((id) => document.getElementById(id)?.textContent ?? '')
    .join(' ');

describe('Reconvert', () => {
  it('says the settings it converts with: those the conversion recorded', async () => {
    const { group } = await open();
    const whole = within(group).getByRole('button', { name: 'Reconvert the whole file' });
    expect(describedBy(whole)).toContain(
      'With the settings of its current conversion (docling 2.3.0): text recognition only where a page has no text layer, in en; page images kept, at 144 dpi; table structure reconstructed.',
    );
  });

  it('starts a whole reconversion, remembers its job, follows it, and shows the version it made', async () => {
    let reads = 0;
    const service = correctionsService();
    const api = mockApi({
      ...service.handlers,
      // The job has finished: the service holds the revision it made, and that is its head.
      [JOB]: () => {
        reads += 1;
        if (service.head() !== RECONVERTED) service.moveHead(RECONVERTED);
        return {
          status: 200,
          body: redigestJob('succeeded', {
            revision: RECONVERTED,
            finished_at: '2026-01-02T10:00:05.000Z',
          }),
        };
      },
    });
    const { queryClient } = renderAt(`/w/${WS}/i/${SOURCE}`);
    const group = await screen.findByRole('group', { name: 'Reconvert' });
    fireEvent.click(within(group).getByRole('button', { name: 'Reconvert the whole file' }));
    await within(group).findByText(
      'The reconversion was accepted as Conversion of a file again, job 77777777: “waiting to start” when the service answered. It is followed below, and listed on the Jobs page.',
    );
    expect(asked(api)).toEqual([
      {
        workspace_id: WS,
        item_id: SOURCE,
        base_revision: HEAD,
        settings: SETTINGS,
        unconverted_only: false,
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    // The job is in the cache, as the Jobs page reads it.
    const key = getJobOptions({ body: { workspace_id: WS, job_id: REDIGEST_JOB } }).queryKey;
    expect(queryClient.getQueryData<Job>(key)?.id).toBe(REDIGEST_JOB);
    // Followed here until it ends.
    const followed = within(group).getByRole('list', { name: 'The reconversion started here' });
    await within(followed).findByText('Finished.');
    expect(reads).toBeGreaterThan(0);
    expect(followed.textContent).toContain(
      `It made revision ${short(RECONVERTED)} of the workspace.`,
    );

    const show = within(group).getByRole('button', { name: 'Show the new conversion' });
    show.focus();
    fireEvent.click(show);
    await within(group).findByText(
      `The workspace now shows revision ${short(RECONVERTED)}, which the conversion made.`,
    );
    await waitFor(() =>
      expect(
        api
          .callsTo(READ)
          .some(
            (body) =>
              JSON.stringify(body.at) ===
              JSON.stringify({ kind: 'revision', revision: RECONVERTED }),
          ),
      ).toBe(true),
    );
    // The button stays, with keyboard focus, and says there is nothing more to show.
    expect(document.activeElement).toBe(show);
    expect(show.getAttribute('aria-disabled')).toBe('true');
    expect(api.violations).toEqual([]);
  });

  it('offers to convert only the pages not converted, with the same settings', async () => {
    const { api, group } = await open({ extraction: extractionOf({ outcome: PARTIAL }) });
    const only = within(group).getByRole('button', {
      name: 'Convert only the pages not converted',
    });
    expect(describedBy(only)).toContain('Pages 3–4 were not converted.');
    fireEvent.click(only);
    await within(group).findByText(/^The reconversion was accepted as/);
    expect(asked(api)).toEqual([
      {
        workspace_id: WS,
        item_id: SOURCE,
        base_revision: HEAD,
        settings: SETTINGS,
        unconverted_only: true,
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    expect(api.violations).toEqual([]);
  });

  it('does not offer the pages not converted when the conversion names none', async () => {
    for (const outcome of [
      { status: 'completed' as const },
      { status: 'partial' as const, issues: [] },
      {
        status: 'partial' as const,
        issues: [],
        coverage: {
          page_count: 2,
          converted: [{ start: 1, end: 2 }],
          not_converted: [],
          partly_extracted: [],
        },
      },
    ]) {
      const { group } = await open({ extraction: extractionOf({ outcome }) });
      expect(within(group).getByRole('button', { name: 'Reconvert the whole file' })).toBeTruthy();
      expect(
        within(group).queryByRole('button', { name: 'Convert only the pages not converted' }),
      ).toBeNull();
      cleanup();
      vi.unstubAllGlobals();
    }
  });

  it('says why it is not offered when no conversion recorded its settings', async () => {
    for (const outcome of [{ status: 'pending' as const }, { status: 'unsupported' as const }]) {
      const { api, group } = await open({
        extraction: { outcome, text_origin: 'none', corrected: false, warnings: [] },
        body: '',
      });
      expect(group.textContent).toContain(
        'Reconvert is not offered: no conversion of this file recorded the settings it was converted with, and the service does not say which settings a new conversion would take.',
      );
      expect(within(group).queryByRole('button')).toBeNull();
      expect(api.violations).toEqual([]);
      cleanup();
      vi.unstubAllGlobals();
    }
  });

  it('reconverts a failed conversion with the settings it failed with', async () => {
    const { api, group } = await open({
      extraction: {
        outcome: {
          status: 'failed',
          issues: [],
          reason: { kind: 'time_limit', limit_seconds: 120 },
        },
        converter: { ...CONVERTER, settings: { ...SETTINGS, ocr: 'force_full_page' } },
        text_origin: 'none',
        corrected: false,
        warnings: [],
      },
      body: '',
    });
    fireEvent.click(within(group).getByRole('button', { name: 'Reconvert the whole file' }));
    await within(group).findByText(/^The reconversion was accepted as/);
    expect(asked(api).at(0)?.settings).toEqual({ ...SETTINGS, ocr: 'force_full_page' });
    expect(api.violations).toEqual([]);
  });

  it('keeps the key while a refused start is tried again, and takes a new one after an answer', async () => {
    let starts = 0;
    const service = correctionsService();
    const api = mockApi({
      ...service.handlers,
      [REDIGEST_ITEM]: (_body: Json) => {
        starts += 1;
        return starts === 1
          ? fail(503, 'unavailable', 'the worker is busy')
          : { status: 202, body: redigestJob('queued') };
      },
    });
    renderAt(`/w/${WS}/i/${SOURCE}`);
    const group = await screen.findByRole('group', { name: 'Reconvert' });
    const whole = within(group).getByRole('button', { name: 'Reconvert the whole file' });
    fireEvent.click(whole);
    await within(group).findByText(
      /Reconverting this file: The service is temporarily unavailable\./,
    );
    fireEvent.click(within(group).getByRole('button', { name: 'Try again' }));
    await within(group).findByText(/^The reconversion was accepted as/);
    fireEvent.click(whole);
    await waitFor(() => expect(asked(api)).toHaveLength(3));
    const [first, second, third] = asked(api);
    expect(second?.idempotency_key).toBe(first?.idempotency_key);
    expect(third?.idempotency_key).not.toBe(first?.idempotency_key);
    expect(api.violations).toEqual([]);
  });

  it('says a reconversion is not available in this build, refused or failed as a job', async () => {
    const refused = await open({
      overrides: {
        [REDIGEST_ITEM]: () => fail(501, 'not_implemented', 'redigest_item is not implemented'),
      },
    });
    fireEvent.click(
      within(refused.group).getByRole('button', { name: 'Reconvert the whole file' }),
    );
    await within(refused.group).findByText(/Reconverting this file: Not available in this build\./);
    cleanup();
    vi.unstubAllGlobals();

    const failed = await open({
      extraction: extractionOf({ outcome: PARTIAL }),
      overrides: {
        [JOB]: () => ({
          status: 200,
          body: redigestJob('failed', {
            retryable: false,
            error: { code: 'not_implemented', message: 'merging converted pages is not built' },
          }),
        }),
      },
    });
    fireEvent.click(
      within(failed.group).getByRole('button', { name: 'Convert only the pages not converted' }),
    );
    const followed = await within(failed.group).findByRole('list', {
      name: 'The reconversion started here',
    });
    await within(followed).findByText(/Why: Not available in this build\./);
    expect(
      within(failed.group).queryByRole('button', { name: 'Show the new conversion' }),
    ).toBeNull();
    expect(failed.api.violations).toEqual([]);
  });

  it('keeps the button in an archived workspace, says why, and sends nothing', async () => {
    const { api, group } = await open({ archivedAt: '2026-01-03T08:00:00.000Z' });
    const whole = within(group).getByRole('button', { name: 'Reconvert the whole file' });
    expect(whole.getAttribute('aria-disabled')).toBe('true');
    expect(describedBy(whole)).toContain('This workspace was archived on');
    fireEvent.click(whole);
    expect(asked(api)).toEqual([]);
    expect(api.violations).toEqual([]);
  });

  it('is not offered to a reader without `write`, who is told why', async () => {
    const service = correctionsService({ permissions: ['read'] });
    mockApi(service.handlers);
    renderAt(`/w/${WS}/i/${SOURCE}`);
    const section = await screen.findByRole('region', { name: 'Corrections' });
    await waitFor(() =>
      expect(section.textContent).toContain(
        'Reconverting this file needs permission to change this workspace, which you do not have.',
      ),
    );
    expect(within(section).queryByRole('button', { name: /Reconvert|Convert only/ })).toBeNull();
  });
});
