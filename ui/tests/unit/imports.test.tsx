/**
 * Bringing files in: choosing files and folders with their paths, and uploading each one as a
 * slot, its bytes and a finish against its length and SHA-256.
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
import { afterEach, describe, expect, it, vi } from 'vitest';
import { CSRF_COOKIE, CSRF_HEADER } from '../../src/app/api-client';
import { MAX_FILE_BYTES, useUploads } from '../../src/features/imports/upload-list';
import { resetUploads, UPLOADS_AT_ONCE } from '../../src/features/imports/uploading';
import { fail, type Handler, mockApi, ok } from './support/api';
import {
  COMPLETE,
  CREATE,
  dropOf,
  droppedFile,
  droppedFolder,
  fileOf,
  fileOfSize,
  SHA_ABC,
  SHA_EMPTY,
  type UploadService,
  uploadService,
} from './support/imports';
import { mockTransfers } from './support/transfers';
import {
  explorerHandlers,
  NOTE,
  renderAt,
  WS,
  workspace,
  writableWorkspace,
} from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const TOKEN = '0123456789abcdef0123456789abcdef';

// On a loaded machine an answer has taken longer than the default second to reach the page.
configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  act(() => resetUploads());
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/** The workspace, writable, with the upload service; a test overrides what it studies. */
function setup(service: UploadService = uploadService(), overrides: Record<string, Handler> = {}) {
  const api = mockApi(
    explorerHandlers({
      [OPEN]: () => ok(writableWorkspace),
      ...service.handlers,
      ...overrides,
    }),
  );
  const sent = mockTransfers(service.transfer);
  return { api, sent, service };
}

async function openImport() {
  const app = renderAt(`/w/${WS}/import`);
  await screen.findByRole('heading', { name: 'Import files' });
  return app;
}

/**
 * Choose files as a person does: press the button, which opens the browser's picker, and pick.
 * The picker itself is the browser's; here it is the input the button opened.
 */
function choose(button: string, files: File[]) {
  const opened: HTMLInputElement[] = [];
  const click = vi.spyOn(HTMLInputElement.prototype, 'click').mockImplementation(function open(
    this: HTMLInputElement,
  ) {
    opened.push(this);
  });
  fireEvent.click(screen.getByRole('button', { name: button }));
  click.mockRestore();
  const picker = opened.at(0);
  if (picker === undefined) throw new Error(`"${button}" opened no picker`);
  Object.defineProperty(picker, 'files', { configurable: true, value: files });
  fireEvent.change(picker);
  return picker;
}

/**
 * Drop files on the import page as a browser delivers a drop: the drag enters, moves over, and is
 * dropped. The folder tree is on the page with its own drag-and-drop listening on the window, as
 * in the real page, so the drop is one the two must both live with.
 */
async function dropOn(drop: ReturnType<typeof dropOf>) {
  await screen.findByRole('treeitem', { name: 'notes' });
  const target = screen.getByRole('group', { name: 'Choose what to import' });
  fireEvent.dragEnter(target, { dataTransfer: drop });
  fireEvent.dragOver(target, { dataTransfer: drop });
  fireEvent.drop(target, { dataTransfer: drop });
}

const rows = () =>
  within(screen.getByRole('list', { name: /^Files to import/ })).getAllByRole('listitem');
const rowOf = (path: string) => {
  const row = rows().find((candidate) => candidate.textContent?.startsWith(path));
  if (row === undefined) throw new Error(`no row for ${path}`);
  return row;
};
const upload = () => fireEvent.click(screen.getByRole('button', { name: /^Upload \d+ files?$/ }));
const uploaded = (path: string) =>
  waitFor(() => expect(rowOf(path).textContent).toContain('Uploaded: the service checked'));
const notUploaded = (path: string) =>
  waitFor(() => expect(rowOf(path).textContent).toContain('Not uploaded.'));

describe('choosing files', () => {
  it('lists several files with their paths and sizes, and sends nothing', async () => {
    const { api, sent } = setup();
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc'), fileOf('report.pdf', 'x'.repeat(2048))]);
    expect(rows().map((row) => row.textContent)).toEqual([
      expect.stringMatching(/^a\.txt 3 bytes Not sent yet\./),
      expect.stringMatching(/^report\.pdf 2\.0 KB Not sent yet\./),
    ]);
    expect(screen.getByRole('heading', { name: 'Files to import (2)' })).toBeDefined();
    expect(screen.getByText('2 files added.')).toBeDefined();
    expect(api.callsTo(CREATE)).toEqual([]);
    expect(sent.transfers).toEqual([]);
  });

  it('keeps the path of each file of a chosen folder, on screen and on the wire', async () => {
    const { api } = setup();
    await openImport();
    choose('Choose a folder…', [
      fileOf('a.txt', 'abc', 'reports/2025/a.txt'),
      fileOf('b.txt', 'abc', 'reports/b.txt'),
      fileOf('c.txt', 'abc', 'reports/2025/q1/c.txt'),
    ]);
    expect(rows().map((row) => row.textContent?.split(' ').at(0))).toEqual([
      'reports/2025/a.txt',
      'reports/b.txt',
      'reports/2025/q1/c.txt',
    ]);
    upload();
    await uploaded('reports/2025/q1/c.txt');
    expect(
      api.callsTo(CREATE).map((body) => [body.relative_path, body.filename, body.size]),
    ).toEqual([
      ['reports/2025', 'a.txt', '3'],
      ['reports', 'b.txt', '3'],
      ['reports/2025/q1', 'c.txt', '3'],
    ]);
    expect(api.violations).toEqual([]);
  });

  it('opens a folder picker for a folder, and a picker of several files for files', async () => {
    setup();
    await openImport();
    const folder = choose('Choose a folder…', []);
    expect(folder.hasAttribute('webkitdirectory')).toBe(true);
    const files = choose('Choose files…', []);
    expect(files.hasAttribute('webkitdirectory')).toBe(false);
    expect(files.multiple).toBe(true);
  });

  it('walks a dropped folder to its end and keeps each file’s path', async () => {
    setup();
    await openImport();
    const drop = dropOf([
      droppedFile(fileOf('x.txt', 'abc')),
      droppedFolder('docs', [
        droppedFile(fileOf('a.txt', 'abc')),
        droppedFolder('sub', [
          droppedFile(fileOf('b.txt', 'abc')),
          droppedFile(fileOf('c.txt', 'abc')),
        ]),
      ]),
    ]);
    await dropOn(drop);
    await screen.findByText('4 files added.');
    expect(rows().map((row) => row.textContent?.split(' ').at(0))).toEqual([
      'x.txt',
      'docs/a.txt',
      'docs/sub/b.txt',
      'docs/sub/c.txt',
    ]);
  });

  it('says how many dropped entries could not be read, and lists the rest', async () => {
    setup();
    await openImport();
    const drop = dropOf([
      droppedFile(fileOf('x.txt', 'abc')),
      droppedFolder('locked', [droppedFile(fileOf('a.txt', 'abc'))], true),
    ]);
    await dropOn(drop);
    await screen.findByText('1 file added. 1 dropped entry could not be read and is not listed.');
    expect(rows()).toHaveLength(1);
  });

  it('does not list the same file twice, and says so', async () => {
    setup();
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    choose('Choose files…', [fileOf('a.txt', 'abc'), fileOf('b.txt', 'abc')]);
    expect(screen.getByText('1 file added. 1 was already in the list.')).toBeDefined();
    expect(rows()).toHaveLength(2);
    // The same name in another folder, or with another length, is another file.
    choose('Choose a folder…', [fileOf('a.txt', 'abc', 'other/a.txt'), fileOf('a.txt', 'abcd')]);
    expect(rows()).toHaveLength(4);
  });

  it('removes an entry, and keeps keyboard focus in the list, then on "Choose files…"', async () => {
    const { api } = setup();
    await openImport();
    choose('Choose files…', [
      fileOf('a.txt', 'abc'),
      fileOf('b.txt', 'abc'),
      fileOf('c.txt', 'abc'),
    ]);
    const remove = (path: string) => screen.getByRole('button', { name: `Remove ${path}` });
    remove('b.txt').focus();
    fireEvent.click(remove('b.txt'));
    expect(rows().map((row) => row.textContent?.split(' ').at(0))).toEqual(['a.txt', 'c.txt']);
    // The row now in its place.
    expect(document.activeElement).toBe(remove('c.txt'));
    fireEvent.click(remove('c.txt'));
    // No row in its place: the last one.
    expect(document.activeElement).toBe(remove('a.txt'));
    fireEvent.click(remove('a.txt'));
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Choose files…' }));
    expect(screen.getByText('No files are listed yet.')).toBeDefined();
    expect(api.callsTo(CREATE)).toEqual([]);
  });

  it('refuses a file over the bound before anything of it is sent, and takes one at the bound', async () => {
    const { api } = setup();
    await openImport();
    choose('Choose files…', [
      fileOfSize('under.bin', MAX_FILE_BYTES - 1),
      fileOfSize('at.bin', MAX_FILE_BYTES),
      fileOfSize('over.bin', MAX_FILE_BYTES + 1),
    ]);
    expect(rowOf('over.bin').textContent).toContain(
      'Too large: this page uploads files of up to 512 MB. It will not be sent.',
    );
    expect(rowOf('at.bin').textContent).toContain('Not sent yet.');
    // Only the two that can be sent are counted.
    fireEvent.click(screen.getByRole('button', { name: 'Upload 2 files' }));
    await waitFor(() => expect(api.callsTo(CREATE)).toHaveLength(2));
    expect(api.callsTo(CREATE).map((body) => [body.filename, body.size])).toEqual([
      ['under.bin', String(MAX_FILE_BYTES - 1)],
      ['at.bin', String(MAX_FILE_BYTES)],
    ]);
    expect(rowOf('over.bin').textContent).toContain('Too large');
  });

  it('offers nothing to a person who may only read', async () => {
    mockApi(explorerHandlers({ [OPEN]: () => ok(workspace) }));
    await openImport();
    expect(screen.getByText(/You may read this workspace, but not add to it/)).toBeDefined();
    expect(screen.queryByRole('button', { name: 'Choose files…' })).toBeNull();
    expect(screen.queryByRole('button', { name: /^Upload/ })).toBeNull();
  });
});

describe('uploading', () => {
  it('opens a slot, sends the bytes and finishes against the length and the hash', async () => {
    vi.spyOn(document, 'cookie', 'get').mockReturnValue(`${CSRF_COOKIE}=${TOKEN}`);
    const { api, sent, service } = setup();
    await openImport();
    choose('Choose a folder…', [fileOf('a.txt', 'abc', 'notes/a.txt')]);
    upload();
    await uploaded('notes/a.txt');

    const [create] = api.callsTo(CREATE);
    expect(create).toEqual({
      workspace_id: WS,
      filename: 'a.txt',
      relative_path: 'notes',
      size: '3',
      sha256: SHA_ABC,
      idempotency_key: expect.stringMatching(/^[0-9a-f-]{36}$/),
    });
    const slot = service.slots.at(0);
    expect(sent.transfers).toHaveLength(1);
    const transfer = sent.transfers.at(0);
    expect(transfer?.method).toBe('PUT');
    expect(transfer?.path).toBe(`/api/workspaces/${WS}/uploads/${slot?.id}/content`);
    expect(transfer?.headers[CSRF_HEADER]).toBe(TOKEN);
    expect(await transfer?.bytes.text()).toBe('abc');
    const [complete] = api.callsTo(COMPLETE);
    expect(complete).toEqual({
      workspace_id: WS,
      upload_id: slot?.id,
      sha256: SHA_ABC,
      idempotency_key: expect.stringMatching(/^[0-9a-f-]{36}$/),
    });
    expect(complete?.idempotency_key).not.toBe(create?.idempotency_key);
    expect(slot?.complete).toBe(true);
    expect(screen.getByText('The file is uploaded and checked.')).toBeDefined();
    expect(api.violations).toEqual([]);
    expect(sent.violations).toEqual([]);
  });

  it('finishes an empty file without sending bytes', async () => {
    const { api, sent, service } = setup();
    await openImport();
    choose('Choose files…', [fileOf('empty.txt', '')]);
    upload();
    await uploaded('empty.txt');
    expect(sent.transfers).toEqual([]);
    expect(api.callsTo(CREATE).at(0)?.size).toBe('0');
    expect(api.callsTo(COMPLETE).at(0)?.sha256).toBe(SHA_EMPTY);
    expect(service.slots.at(0)?.complete).toBe(true);
  });

  it('does not count a file as uploaded when the finish does not confirm it', async () => {
    const service = uploadService();
    // The service answers, but not that the file is complete.
    service.script.complete.push(async (_body, usual) => {
      const reply = await usual();
      return ok({ ...Object(reply.body), complete: false });
    });
    setup(service);
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    upload();
    await notUploaded('a.txt');
    const row = rowOf('a.txt');
    expect(row.textContent).not.toContain('Uploaded');
    expect(row.textContent).toContain('Uploading a.txt: The service did not confirm this upload.');
    expect(within(row).getByRole('button', { name: 'Send it again' })).toBeDefined();
    expect(screen.getByText(/^0 of 1 file uploaded; 1 not uploaded\./)).toBeDefined();
  });

  it('does not count a file as uploaded when the finish reports another length', async () => {
    const service = uploadService();
    service.script.complete.push(async (_body, usual) => {
      const reply = await usual();
      return ok({ ...Object(reply.body), received_bytes: '2' });
    });
    setup(service);
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    upload();
    await notUploaded('a.txt');
    expect(rowOf('a.txt').textContent).toContain('The service did not confirm this upload.');
  });

  it('fails a file whose bytes the service did not keep whole, and sends the rest on a retry', async () => {
    const service = uploadService();
    // The service keeps two of the three bytes, and says so.
    service.script.put.push((transfer) => {
      const slot = service.slots.at(0);
      slot?.parts.push(transfer.bytes.slice(0, 2));
      return ok({
        id: slot?.id,
        workspace_id: WS,
        upload_path: transfer.path,
        received_bytes: '2',
        complete: false,
      });
    });
    const { api, sent } = setup(service);
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    upload();
    await notUploaded('a.txt');
    const row = rowOf('a.txt');
    expect(row.textContent).toContain('The service did not keep the whole file.');
    expect(row.textContent).toContain('It holds 2 of 3 bytes.');
    expect(api.callsTo(COMPLETE)).toEqual([]);

    fireEvent.click(within(row).getByRole('button', { name: 'Try again' }));
    await uploaded('a.txt');
    // The slot was asked again under its key, and only the missing byte was sent.
    const creates = api.callsTo(CREATE);
    expect(creates).toHaveLength(2);
    expect(creates.at(1)).toEqual(creates.at(0));
    expect(service.slots).toHaveLength(1);
    expect(await sent.transfers.at(1)?.bytes.text()).toBe('c');
    expect(await service.held(service.slots.at(0))).toBe('abc');
  });

  it('keeps a failed file in its place with a retry, while the others go on', async () => {
    const service = uploadService();
    service.script.put.push((transfer) => {
      transfer.breakConnection();
      return undefined;
    });
    const { api } = setup(service);
    await openImport();
    choose('Choose files…', [
      fileOf('a.txt', 'abc'),
      fileOf('b.txt', 'abc'),
      fileOf('c.txt', 'abc'),
    ]);
    upload();
    await notUploaded('a.txt');
    await uploaded('b.txt');
    await uploaded('c.txt');
    expect(rows().map((row) => row.textContent?.split(' ').at(0))).toEqual([
      'a.txt',
      'b.txt',
      'c.txt',
    ]);
    const row = rowOf('a.txt');
    expect(row.textContent).not.toContain('Uploaded');
    expect(row.textContent).toContain(
      'Uploading a.txt: The workspace service could not be reached.',
    );
    expect(
      screen.getByText(
        '2 of 3 files uploaded; 1 not uploaded. Each of those has its own notice and retry.',
      ),
    ).toBeDefined();

    const again = within(row).getByRole('button', { name: 'Try again' });
    again.focus();
    fireEvent.click(again);
    await uploaded('a.txt');
    // The same slot under the same key; nothing was opened twice.
    const mine = api.callsTo(CREATE).filter((body) => body.filename === 'a.txt');
    expect(mine).toHaveLength(2);
    expect(mine.at(1)).toEqual(mine.at(0));
    expect(service.slots).toHaveLength(3);
    const finishes = api.callsTo(COMPLETE);
    expect(new Set(finishes.map((body) => body.upload_id)).size).toBe(3);
    // Focus was on the retry, which is gone: it is on the row's own control.
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Remove a.txt' })),
    );
    expect(screen.getByText('All 3 files are uploaded and checked.')).toBeDefined();
  });

  it('goes on from what the service holds when the answer to the bytes was lost', async () => {
    const service = uploadService();
    // The bytes arrive and are kept; the answer never comes back.
    service.script.put.push((transfer, usual) => {
      usual();
      transfer.breakConnection();
      return undefined;
    });
    const { sent } = setup(service);
    await openImport();
    const file = fileOf('a.txt', 'abc');
    const read = vi.spyOn(file, 'arrayBuffer');
    choose('Choose files…', [file]);
    upload();
    await notUploaded('a.txt');
    fireEvent.click(within(rowOf('a.txt')).getByRole('button', { name: 'Try again' }));
    await uploaded('a.txt');
    // The same slot: the file was read once, for its hash, and not again.
    expect(read).toHaveBeenCalledTimes(1);
    // The slot said it holds the whole file: no byte was sent a second time.
    expect(sent.transfers).toHaveLength(1);
    expect(service.slots).toHaveLength(1);
    expect(await service.held(service.slots.at(0))).toBe('abc');
  });

  it('opens a new slot, and reads the file again, after the service refused the finish', async () => {
    const service = uploadService();
    service.script.complete.push(() =>
      fail(409, 'conflict', 'the digest differs from the one announced'),
    );
    const { api } = setup(service);
    await openImport();
    const file = fileOf('a.txt', 'abc');
    const read = vi.spyOn(file, 'arrayBuffer');
    choose('Choose files…', [file]);
    upload();
    await notUploaded('a.txt');
    expect(read).toHaveBeenCalledTimes(1);
    const row = rowOf('a.txt');
    expect(row.textContent).toContain('Uploading a.txt: The service did not accept this file.');
    expect(row.textContent).toContain(
      'The service said: the digest differs from the one announced',
    );
    expect(row.textContent).not.toContain('This changed since it was loaded');
    fireEvent.click(within(row).getByRole('button', { name: 'Send it again' }));
    await uploaded('a.txt');
    const creates = api.callsTo(CREATE);
    expect(creates).toHaveLength(2);
    expect(creates.at(1)?.idempotency_key).not.toBe(creates.at(0)?.idempotency_key);
    expect(service.slots).toHaveLength(2);
    const finishes = api.callsTo(COMPLETE);
    expect(finishes.at(1)?.idempotency_key).not.toBe(finishes.at(0)?.idempotency_key);
    expect(finishes.at(1)?.upload_id).toBe(service.slots.at(1)?.id);
    // The file was read a second time for the new slot.
    expect(read).toHaveBeenCalledTimes(2);
  });

  it('reads a file the service finds too large as too large, with no retry', async () => {
    const service = uploadService();
    service.script.create.push(() => fail(413, 'too_large', 'at most 1 GiB per file'));
    setup(service);
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    upload();
    await notUploaded('a.txt');
    const row = rowOf('a.txt');
    expect(row.textContent).toContain(
      'Uploading a.txt: This file is larger than the service accepts.',
    );
    expect(row.textContent).toContain('The service said: at most 1 GiB per file');
    expect(row.textContent).not.toContain('too large to show');
    expect(within(row).queryByRole('button', { name: /again/i })).toBeNull();
  });

  it('reads "not available in this build" where uploads are not built, never as uploaded', async () => {
    mockApi(explorerHandlers({ [OPEN]: () => ok(writableWorkspace) }));
    mockTransfers();
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    upload();
    await notUploaded('a.txt');
    expect(rowOf('a.txt').textContent).toContain('Uploading a.txt: Not available in this build.');
    expect(rowOf('a.txt').textContent).not.toContain('Uploaded');
  });

  it(`has ${UPLOADS_AT_ONCE} files under way at once, and starts the next when one ends`, async () => {
    const service = uploadService();
    for (let file = 0; file < 5; file += 1) service.script.put.push(() => undefined);
    const { sent } = setup(service);
    await openImport();
    choose(
      'Choose files…',
      ['a', 'b', 'c', 'd', 'e'].map((name) => fileOf(`${name}.txt`, 'abc')),
    );
    upload();
    await waitFor(() => expect(sent.transfers).toHaveLength(UPLOADS_AT_ONCE));
    // Give a fourth every chance to start: none does.
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(sent.transfers).toHaveLength(UPLOADS_AT_ONCE);
    expect(rowOf('d.txt').textContent).toContain('Waiting for its turn.');
    expect(rowOf('e.txt').textContent).toContain('Waiting for its turn.');
    const first = sent.transfers.at(0);
    if (first === undefined) throw new Error('no transfer');
    act(() => {
      first.allSent();
      first.answer(service.accept(first));
    });
    await waitFor(() => expect(sent.transfers).toHaveLength(UPLOADS_AT_ONCE + 1));
    await uploaded('a.txt');
    expect(rowOf('e.txt').textContent).toContain('Waiting for its turn.');
  });

  it('shows each file’s percent, and says the whole upload’s standing at each tenth only', async () => {
    const service = uploadService();
    service.script.put.push(() => undefined);
    const { sent } = setup(service);
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'x'.repeat(200))]);
    upload();
    await waitFor(() => expect(sent.transfers).toHaveLength(1));
    const transfer = sent.transfers.at(0);
    if (transfer === undefined) throw new Error('no transfer');
    const status = () => screen.getByText(/^Uploading: /).textContent;
    expect(status()).toBe('Uploading: 0 of 1 file done; about 0% of the bytes sent.');
    const bar = () => screen.getByRole('progressbar', { name: 'a.txt: bytes sent' });
    // A report that does not move the whole percent changes nothing.
    act(() => transfer.progress(1));
    expect(useUploads.getState().entries.at(0)?.sent).toBe(0);
    act(() => transfer.progress(110));
    expect(rowOf('a.txt').textContent).toContain('Sending: 55%.');
    expect(bar().getAttribute('value')).toBe('110');
    expect(status()).toBe('Uploading: 0 of 1 file done; about 50% of the bytes sent.');
    act(() => transfer.progress(118));
    expect(rowOf('a.txt').textContent).toContain('Sending: 59%.');
    // Still the same tenth: the words of the status have not changed.
    expect(status()).toBe('Uploading: 0 of 1 file done; about 50% of the bytes sent.');
    expect(screen.getByText(/^Uploading: /).getAttribute('role')).toBe('status');
    act(() => {
      transfer.allSent();
      transfer.answer(service.accept(transfer));
    });
    await uploaded('a.txt');
  });

  it('aborts the transfer of a file that is removed, and asks nothing more for it', async () => {
    const service = uploadService();
    service.script.put.push(() => undefined);
    const { api, sent } = setup(service);
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    upload();
    await waitFor(() => expect(sent.transfers).toHaveLength(1));
    fireEvent.click(screen.getByRole('button', { name: 'Remove a.txt' }));
    expect(sent.transfers.at(0)?.aborted).toBe(true);
    expect(screen.getByText('No files are listed yet.')).toBeDefined();
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
    expect(api.callsTo(COMPLETE)).toEqual([]);
    expect(useUploads.getState().entries).toEqual([]);
  });

  it('opens no slot for a file removed before its turn', async () => {
    const service = uploadService();
    for (let file = 0; file < UPLOADS_AT_ONCE; file += 1) service.script.put.push(() => undefined);
    const { api, sent } = setup(service);
    await openImport();
    choose(
      'Choose files…',
      ['a', 'b', 'c', 'd'].map((name) => fileOf(`${name}.txt`, 'abc')),
    );
    upload();
    await waitFor(() => expect(sent.transfers).toHaveLength(UPLOADS_AT_ONCE));
    fireEvent.click(screen.getByRole('button', { name: 'Remove d.txt' }));
    for (const transfer of sent.transfers) {
      act(() => {
        transfer.allSent();
        transfer.answer(service.accept(transfer));
      });
    }
    await uploaded('c.txt');
    expect(api.callsTo(CREATE).map((body) => body.filename)).toEqual(['a.txt', 'b.txt', 'c.txt']);
  });
});

describe('leaving with uploads', () => {
  const leaving = () => {
    const event = new Event('beforeunload', { cancelable: true });
    window.dispatchEvent(event);
    return event.defaultPrevented;
  };

  it('asks before the page is left while a file is under way, or uploaded and not imported', async () => {
    const service = uploadService();
    service.script.put.push(() => undefined);
    const { sent } = setup(service);
    await openImport();
    // Nothing listed, then files only listed: nothing is at risk.
    expect(leaving()).toBe(false);
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    expect(leaving()).toBe(false);
    upload();
    await waitFor(() => expect(sent.transfers).toHaveLength(1));
    expect(leaving()).toBe(true);
    const transfer = sent.transfers.at(0);
    if (transfer === undefined) throw new Error('no transfer');
    act(() => {
      transfer.allSent();
      transfer.answer(service.accept(transfer));
    });
    await uploaded('a.txt');
    // Uploaded, and only this page knows its slot.
    expect(leaving()).toBe(true);
    fireEvent.click(screen.getByRole('button', { name: 'Remove a.txt' }));
    expect(leaving()).toBe(false);
  });

  it('goes on uploading while the person looks at something else, and lists it on return', async () => {
    const service = uploadService();
    service.script.put.push(() => undefined);
    const { sent } = setup(service);
    const { router } = await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    upload();
    await waitFor(() => expect(sent.transfers).toHaveLength(1));
    await act(() =>
      router.navigate({
        to: '/w/$workspaceId/i/$itemId',
        params: { workspaceId: WS, itemId: NOTE },
      }),
    );
    await screen.findByRole('heading', { name: 'Meeting notes', level: 2 });
    expect(screen.queryByRole('heading', { name: 'Import files' })).toBeNull();
    const transfer = sent.transfers.at(0);
    if (transfer === undefined) throw new Error('no transfer');
    expect(transfer.aborted).toBe(false);
    act(() => {
      transfer.allSent();
      transfer.answer(service.accept(transfer));
    });
    await waitFor(() => expect(service.slots.at(0)?.complete).toBe(true));
    fireEvent.click(screen.getByRole('link', { name: 'Import' }));
    await screen.findByRole('heading', { name: 'Import files' });
    await uploaded('a.txt');
  });
});
