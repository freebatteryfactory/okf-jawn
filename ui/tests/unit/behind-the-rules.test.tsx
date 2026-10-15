/**
 * What stands behind the naming rules while they are open, and whose press a link of the bar is.
 *
 * The rules stand in front of what the address names, which stays mounted and hidden: an upload
 * under way goes on and its row is the same row afterwards; an editor with text not saved yet is
 * the same text box afterwards. A press of a bar link that opens it elsewhere (a modifier key,
 * another mouse button) is the browser's: the rules neither close nor ask.
 */
import {
  act,
  cleanup,
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import namingSchema from '../../../api/forms/naming-rules.schema.json';
import { resetDraftSessions } from '../../src/features/drafts/saving';
import { resetUploads } from '../../src/features/imports/uploading';
import { type Handler, mockApi, ok, rulesAnswer } from './support/api';
import { findTextBox, textIn, typeInto } from './support/editor';
import { fileOf, uploadService } from './support/imports';
import { mockTransfers } from './support/transfers';
import { explorerHandlers, NOTE, renderAt, WS, writableWorkspace } from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const SCHEMA = '/api/schemas/naming-rules';
const RULES = '/api/conventions/get-rules';
const SAVE = '/api/items/save-draft';
const savedRules = {
  schema_version: 1,
  rules: [
    {
      glob: '**/*.md',
      case: 'kebab',
      separator: '-',
      prefix: '',
      date_order: 'ymd',
      date_format: '%Y-%m-%d',
      strip_version_suffix: true,
      extension_policy: 'preserve',
      alias_preservation: true,
      collision: 'reject',
    },
  ],
};

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  act(() => resetUploads());
  act(() => resetDraftSessions());
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function setup(overrides: Record<string, Handler> = {}) {
  const service = uploadService();
  const api = mockApi(
    explorerHandlers({
      [OPEN]: () => ok(writableWorkspace),
      ...service.handlers,
      [SCHEMA]: () => ok(namingSchema),
      [RULES]: (body) => ok(rulesAnswer(savedRules, body)),
      ...overrides,
    }),
  );
  // No transfer is answered by itself: an upload stays under way until the test says.
  const sent = mockTransfers(() => undefined);
  return { api, sent, service };
}

const button = (name: string | RegExp) => screen.getByRole('button', { name });
const link = (name: string | RegExp) => screen.getByRole('link', { name });
const panel = () => screen.getByRole('region', { name: 'Naming rules' });
const rulesPanel = () => screen.queryByRole('region', { name: 'Naming rules' });
const prefix = () => within(panel()).getByRole('textbox', { name: /^Prefix/ });
const question = () =>
  screen.queryByRole('group', { name: 'Leave the naming rules without saving?' });
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 50)));

async function openRules() {
  await userEvent.click(button('Naming rules'));
  await within(panel()).findByRole('textbox', { name: /^Prefix/ });
}

/** Choose `files` in the picker that the button `name` opens. */
function choose(name: string, files: File[]) {
  const opened: HTMLInputElement[] = [];
  const click = vi.spyOn(HTMLInputElement.prototype, 'click').mockImplementation(function open(
    this: HTMLInputElement,
  ) {
    opened.push(this);
  });
  fireEvent.click(screen.getByRole('button', { name }));
  click.mockRestore();
  const picker = opened.at(0);
  if (picker === undefined) throw new Error(`"${name}" opened no picker`);
  Object.defineProperty(picker, 'files', { configurable: true, value: files });
  fireEvent.change(picker);
}

describe('what stands behind the naming rules', () => {
  it('keeps an upload under way on the Import page: hidden from everyone, going on, and the same row when the rules close', async () => {
    const { sent } = setup();
    renderAt(`/w/${WS}/import`);
    await screen.findByRole('heading', { name: 'Import files' });
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    const list = await screen.findByRole('list', { name: /^Files to import/ });
    const row = within(list).getAllByRole('listitem').at(0);
    fireEvent.click(button(/^Upload \d+ files?$/));
    await waitFor(() => expect(sent.transfers.length).toBeGreaterThan(0));
    const transfer = sent.transfers.at(-1);
    await openRules();
    // Behind the rules: out of the accessibility tree, under an element that is `hidden`.
    expect(screen.queryByRole('heading', { name: 'Import files' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Choose files…' })).toBeNull();
    expect(screen.queryByRole('list', { name: /^Files to import/ })).toBeNull();
    const behind = screen.getByRole('button', { name: 'Choose files…', hidden: true });
    const hider = behind.closest('[hidden]');
    expect(hider instanceof HTMLElement && hider.hidden).toBe(true);
    // The upload goes on meanwhile, and what it has sent is said when the page is back.
    expect(transfer?.aborted).toBe(false);
    act(() => transfer?.progress(1));
    await userEvent.click(button('Close the naming rules'));
    expect(rulesPanel()).toBeNull();
    expect(document.activeElement).toBe(button('Naming rules'));
    const again = within(screen.getByRole('list', { name: /^Files to import/ }))
      .getAllByRole('listitem')
      .at(0);
    // The same row, never taken off the page, and its transfer neither stopped nor over.
    expect(again).toBe(row);
    expect(again?.textContent).toContain('Sending: 33%');
    expect(transfer?.aborted).toBe(false);
    expect(transfer?.over).toBe(false);
  }, 40_000);

  it('keeps an editor with text not saved yet: hidden behind the rules, and the same text box with its text when they close', async () => {
    setup({ [SAVE]: () => new Promise(() => {}) });
    renderAt(`/w/${WS}/i/${NOTE}`);
    await userEvent.click(await screen.findByRole('button', { name: 'Edit' }));
    const box = await findTextBox('Text of Meeting notes');
    typeInto(box, ' and more');
    await openRules();
    expect(screen.queryByRole('textbox', { name: 'Text of Meeting notes' })).toBeNull();
    expect(box.isConnected).toBe(true);
    expect(box.closest('[hidden]')).not.toBeNull();
    await userEvent.click(button('Close the naming rules'));
    const again = screen.getByRole('textbox', { name: 'Text of Meeting notes' });
    expect(again).toBe(box);
    expect(textIn(again)).toContain(' and more');
    expect(document.activeElement).toBe(button('Naming rules'));
  }, 40_000);
});

describe('a press of a bar link that opens it elsewhere, while the rules are open', () => {
  it.each([
    ['Ctrl', { ctrlKey: true }],
    ['Meta', { metaKey: true }],
    ['Shift', { shiftKey: true }],
    ['Alt', { altKey: true }],
    ['the middle button', { button: 1 }],
  ])(
    'with %s is the browser’s: the rules neither close nor ask',
    async (_name, how) => {
      setup();
      const { router } = renderAt(`/w/${WS}`);
      await screen.findByRole('button', { name: 'Naming rules' });
      await openRules();
      await userEvent.clear(prefix());
      await userEvent.type(prefix(), 'unsaved-');
      // The link to the place the address already names: the one press this page follows itself.
      fireEvent.click(link('Documents'), how);
      await settle();
      expect(question()).toBeNull();
      expect(rulesPanel()).not.toBeNull();
      expect(prefix()).toHaveProperty('value', 'unsaved-');
      expect(router.state.location.pathname).toBe(`/w/${WS}`);
      // The same press with nothing held is this page's: it asks.
      fireEvent.click(link('Documents'));
      expect(question()).not.toBeNull();
    },
    40_000,
  );
});
