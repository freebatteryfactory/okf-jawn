/** Shared MCP App result dispatch: Source / Changes / Timeline / Present / unknown text. */

import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import changesFixture from '../../../tests/fixtures/views/changes-diff.json';
import presentFixture from '../../../tests/fixtures/views/present-response.json';
import sourceFixture from '../../../tests/fixtures/views/source-read-item.json';
import timelineFixture from '../../../tests/fixtures/views/timeline-log.json';
import { AppResult } from '../../src/mcp-apps/main';

describe('MCP App AppResult dispatch', () => {
  it('renders Source from a ReadItemResponse', () => {
    render(<AppResult result={sourceFixture} callTool={vi.fn()} />);
    expect(screen.getByText(/Qualification source/i)).toBeTruthy();
  });

  it('renders Changes from a DiffResponse', () => {
    render(<AppResult result={changesFixture} callTool={vi.fn()} />);
    expect(screen.getByRole('heading', { name: 'Changes' })).toBeTruthy();
  });

  it('renders Timeline from a LogResponse', () => {
    render(<AppResult result={timelineFixture} callTool={vi.fn()} />);
    expect(screen.getByRole('heading', { name: 'Timeline' })).toBeTruthy();
    expect(screen.getByText('Qualification timeline fixture')).toBeTruthy();
  });

  it('renders Present from a PresentResponse', async () => {
    const present = {
      ...presentFixture,
      resolved_bindings: [],
    };
    render(
      <AppResult result={present} callTool={vi.fn(async () => ({ structuredContent: {} }))} />,
    );
    expect(await screen.findByRole('heading', { name: 'Six-component catalog' })).toBeTruthy();
  });

  it('falls back to text for an unknown structuredContent shape', () => {
    render(<AppResult result={{ unexpected: true }} callTool={vi.fn()} />);
    expect(screen.getByRole('status').textContent).toMatch(/unexpected/);
  });
});
