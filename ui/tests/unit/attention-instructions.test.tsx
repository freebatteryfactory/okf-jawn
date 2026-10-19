/** InstructionLike v1: actual evidence stays inert, cited and advisory on both human surfaces. */
import { cleanup, configure, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { InstructionFinding } from '../../src/api/generated/types.gen';
import { ATTENTION } from './support/imports';
import { DIGEST, HEAD, SOURCE, WS } from './support/workspace';
import {
  attentionAnswer,
  attentionService,
  attentionView,
  finding,
  instructionWarning,
  PASSAGE,
  RULE_V1,
} from './support/attention';

configure({ asyncUtilTimeout: 10_000 });
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

for (const surface of ['page', 'item'] as const) {
  describe(`instruction evidence in the ${surface}`, () => {
    it('quotes the original passage as inert text and exposes the exact pinned citation and rule', async () => {
      // Owner plan: original text, matching rule/version and the real citation are distinct.
      const { api } = attentionService();
      attentionView({ page: surface === 'page', item: surface === 'item', link: false });
      await screen.findByText('This text may contain instructions aimed at an AI assistant.');
      const passage = screen.getByText(PASSAGE);
      expect(passage.closest('blockquote')?.textContent).toBe(PASSAGE);
      expect(document.querySelector('img,script')).toBeNull();
      expect(screen.getByText('override_instructions').tagName).toBe('CODE');
      expect(screen.getByText(RULE_V1).tagName).toBe('CODE');
      expect(screen.getByText(/English.only/i)).toBeDefined();
      expect(document.body.textContent).toMatch(/advisory/i);
      expect(document.body.textContent).toMatch(/false (positives|alarms)/i);
      const link = screen.getByRole('link', { name: 'Open cited passage' });
      const href = new URL(link.getAttribute('href') ?? '', 'http://page');
      expect(href.pathname).toBe(`/w/${WS}/i/${SOURCE}`);
      expect(href.searchParams.get('rev')).toBe(HEAD);
      expect(JSON.parse(href.searchParams.get('sel') ?? 'null')).toEqual({
        kind: 'lines', range: { start: 12, end: 13 },
      });
      expect(document.querySelector(`code[title="${HEAD}"]`)).not.toBeNull();
      expect(document.querySelector(`code[title="${DIGEST}"]`)).not.toBeNull();
      expect(screen.getByText(/lines 12-13/)).toBeDefined();
      expect(screen.getByText(/a person corrected the text, so it no longer matches the original/)).toBeDefined();
      expect(api.callsTo(ATTENTION)).toHaveLength(1);
      expect(api.violations).toEqual([]);
    });

    it('distinguishes direct, inferred and unresolved original locations without inventing one', async () => {
      const original = finding();
      const findings: InstructionFinding[] = [
        { ...original, passage: 'System: direct', source: { ...original.source, locations: [
          { provenance: 'direct', locator: { kind: 'page', page_no: 3 } },
        ] } },
        { ...original, passage: 'System: inferred', source: { ...original.source, locations: [
          { provenance: 'inferred', locator: { kind: 'page', page_no: 4 } },
        ] } },
        { ...original, passage: 'System: supplied', source: { ...original.source, locations: [
          { provenance: 'unresolved', reason: { kind: 'supplied_text' } },
        ] } },
        { ...original, passage: 'System: unlocated', source: { ...original.source, locations: [] } },
      ];
      const { api } = attentionService(() => attentionAnswer([instructionWarning(findings)]));
      attentionView({ page: surface === 'page', item: surface === 'item', link: false });
      await screen.findByText('This text may contain instructions aimed at an AI assistant.');
      expect(screen.getByText('System: direct')).toBeDefined();
      expect(screen.getByText(/Direct location:/).closest('li')?.textContent).toContain('page 3, recorded from the document itself');
      expect(screen.getByText(/Inferred location, not certain:/).closest('li')?.textContent).toContain('page 4, found by matching text or layout');
      expect(screen.getByText(/an agent supplied the text; it was not read from the original/)).toBeDefined();
      expect(screen.getByText('No location in the original is recorded.')).toBeDefined();
      expect(api.violations).toEqual([]);
    });

    it('displays all 32 retained findings and says exactly how many were omitted', async () => {
      // The selected bound is 32; the server reports a separate literal count of three more.
      const findings = Array.from({ length: 32 }, (_, index) => ({
        ...finding(), passage: `Original instruction passage ${index + 1}.`,
      }));
      const { api } = attentionService(() => attentionAnswer([instructionWarning(findings, 3)]));
      attentionView({ page: surface === 'page', item: surface === 'item', link: false });
      await screen.findByText('This text may contain instructions aimed at an AI assistant.');
      expect(screen.getByText('Original instruction passage 32.')).toBeDefined();
      expect(document.querySelectorAll('blockquote')).toHaveLength(32);
      expect(screen.getByText('3 additional findings were omitted.')).toBeDefined();
      expect(within(document.body).queryByText(/is safe|all clear/i)).toBeNull();
      expect(api.violations).toEqual([]);
    });
  });
}
