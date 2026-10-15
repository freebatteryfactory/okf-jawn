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
  unreadableFile,
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
const WORKSPACES = '/api/workspaces/list-workspaces';
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

  it('lists a file of a dropped folder that cannot be read as not readable, and every other file of the folder', async () => {
    const { api } = setup();
    await openImport();
    await dropOn(
      dropOf([
        droppedFolder('docs', [
          droppedFile(fileOf('a.txt', 'abc')),
          unreadableFile('dangling-link'),
          droppedFile(fileOf('c.txt', 'abc')),
          droppedFolder('sub', [droppedFile(fileOf('d.txt', 'abc'))]),
        ]),
      ]),
    );
    await screen.findByText(
      '3 files added. 1 file could not be read: it is listed as not readable and will not be sent.',
    );
    // The files after the one that could not be read are there, and the folder is not a file.
    expect(rows().map((row) => row.textContent?.split(' ').at(0))).toEqual([
      'docs/a.txt',
      'docs/c.txt',
      'docs/sub/d.txt',
      'docs/dangling-link',
    ]);
    // No size is claimed for it: nothing of it was read.
    expect(rowOf('docs/dangling-link').textContent?.replace(/\s+/g, ' ')).toBe(
      'docs/dangling-link Not readable: the browser could not open this file. It will not be sent. Remove',
    );
    // It is not counted among the files to upload, and nothing is asked for it.
    expect(screen.getByRole('button', { name: 'Upload 3 files' })).toBeDefined();
    upload();
    await uploaded('docs/sub/d.txt');
    await screen.findByText('All 3 files are uploaded and checked.');
    expect(api.callsTo(CREATE).map((body) => `${body.relative_path}/${body.filename}`)).toEqual([
      'docs/a.txt',
      'docs/c.txt',
      'docs/sub/d.txt',
    ]);
    expect(rowOf('docs/dangling-link').textContent).toContain('Not readable');
    // It is not listed twice when the folder is dropped again, and it can be taken off the list.
    await dropOn(dropOf([droppedFolder('docs', [unreadableFile('dangling-link')])]));
    await screen.findByText('No file was added to the list. 1 was already in the list.');
    fireEvent.click(screen.getByRole('button', { name: 'Remove docs/dangling-link' }));
    expect(rows()).toHaveLength(3);
  });

  it('lists a file dropped by itself that the browser offers although its entry cannot hand it over', async () => {
    setup();
    await openImport();
    // Chromium, for a link to a file: the entry fails, the item still answers a File.
    await dropOn(
      dropOf([unreadableFile('link.txt', fileOf('link.txt', 'abc')), unreadableFile('gone.txt')]),
    );
    await screen.findByText(
      '1 file added. 1 file could not be read: it is listed as not readable and will not be sent.',
    );
    expect(rows().map((row) => row.textContent?.split(' ').at(0))).toEqual([
      'link.txt',
      'gone.txt',
    ]);
    upload();
    await uploaded('link.txt');
  });

  it('lists a dropped thing the browser knows only as a File, and nothing for one that is no file', async () => {
    setup();
    await openImport();
    const drop = dropOf([]);
    drop.items.push(
      // No entry: a drag another page made, or a browser without the entry API for it.
      { kind: 'file', webkitGetAsEntry: () => null, getAsFile: () => fileOf('plain.txt', 'abc') },
      // Dragged text beside the files: neither an entry nor a File.
      { kind: 'string', webkitGetAsEntry: () => null, getAsFile: () => null },
    );
    await dropOn(drop);
    await screen.findByText('1 file added.');
    expect(rows().map((row) => row.textContent?.split(' ').at(0))).toEqual(['plain.txt']);
  });

  it('never lists a folder as a file: not one it could not list, and not an empty one', async () => {
    const { api } = setup();
    await openImport();
    // A browser offers each dropped folder as a File of its name too; that File is not a file.
    await dropOn(
      dropOf([
        droppedFile(fileOf('x.txt', 'abc')),
        droppedFolder('locked', [droppedFile(fileOf('a.txt', 'abc'))], true),
        droppedFolder('empty', []),
        droppedFolder('docs', [
          droppedFile(fileOf('b.txt', 'abc')),
          droppedFolder('old', []),
          droppedFolder('older', []),
        ]),
      ]),
    );
    await screen.findByText(
      '2 files added. The folders “empty”, “docs/old”, “docs/older” are empty: an empty folder is not kept. The folder “locked” could not be read in full: only what was read of it is listed.',
    );
    expect(rows().map((row) => row.textContent?.split(' ').at(0))).toEqual(['x.txt', 'docs/b.txt']);
    upload();
    await uploaded('docs/b.txt');
    expect(api.callsTo(CREATE).map((body) => body.filename)).toEqual(['x.txt', 'b.txt']);
  });

  it('says that a dropped folder is empty and not kept, and lists nothing for it', async () => {
    setup();
    await openImport();
    await dropOn(dropOf([droppedFolder('empty', [])]));
    await screen.findByText(
      'No file was added to the list. The folder “empty” is empty: an empty folder is not kept.',
    );
    expect(screen.getByText('No files are listed yet.')).toBeDefined();
    // More than a few are counted, not all named.
    await dropOn(dropOf(['a', 'b', 'c', 'd', 'e'].map((name) => droppedFolder(name, []))));
    await screen.findByText(
      'No file was added to the list. The folders “a”, “b”, “c” and 2 more are empty: an empty folder is not kept.',
    );
  });

  it('keeps what was read of a folder whose listing fails part-way, and says so', async () => {
    setup();
    await openImport();
    await dropOn(
      dropOf([
        droppedFolder(
          'docs',
          [droppedFile(fileOf('a.txt', 'abc')), droppedFile(fileOf('b.txt', 'abc'))],
          'part-way',
        ),
      ]),
    );
    await screen.findByText(
      '1 file added. The folder “docs” could not be read in full: only what was read of it is listed.',
    );
    expect(rows().map((row) => row.textContent?.split(' ').at(0))).toEqual(['docs/a.txt']);
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

  it('lists a file again when it changed since it was listed, although its path and length are the same', async () => {
    setup();
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    // Saved again since: the same name and length, a later modification time.
    choose('Choose files…', [fileOf('a.txt', 'xyz', '', 1_700_000_060_000)]);
    expect(screen.getByText('1 file added.')).toBeDefined();
    expect(screen.queryByText(/already in the list/)).toBeNull();
    expect(rows()).toHaveLength(2);
  });

  it('lists a file for another workspace although this one lists it', async () => {
    const elsewhere = '12121212-1212-4212-8212-121212121212';
    setup(uploadService(), {
      [OPEN]: (body) => ok({ ...writableWorkspace, id: body.workspace_id }),
    });
    const { router } = await openImport();
    const file = fileOf('a.txt', 'abc');
    choose('Choose files…', [file]);
    expect(rows()).toHaveLength(1);
    await act(() =>
      router.navigate({ to: '/w/$workspaceId/import', params: { workspaceId: elsewhere } }),
    );
    await screen.findByText('No files are listed yet.');
    choose('Choose files…', [file]);
    expect(screen.queryByText(/already in the list/)).toBeNull();
    expect(rows()).toHaveLength(1);
    // Nor is it held back for being at the same place as the other workspace's file.
    expect(screen.getByRole('button', { name: 'Upload 1 file' })).toBeDefined();
    expect(useUploads.getState().entries.map((entry) => entry.workspaceId)).toEqual([
      WS,
      elsewhere,
    ]);
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
    expect(rowOf('under.bin').textContent).toContain('Not sent yet.');
    // Only the two that can be sent are counted, and the one over the bound is given no turn.
    // (These stand-in files report a length they do not have, so nothing of them is uploaded:
    // each reads as changed since it was chosen. The bound is decided before that, from the
    // length the browser reports.)
    fireEvent.click(screen.getByRole('button', { name: 'Upload 2 files' }));
    await notUploaded('under.bin');
    await notUploaded('at.bin');
    expect(rowOf('over.bin').textContent).toContain('Too large');
    expect(rowOf('over.bin').textContent).not.toContain('Not uploaded.');
    expect(api.callsTo(CREATE)).toEqual([]);
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

  it('keeps the slot when the service cannot take the bytes now, and goes on with it on "Try again"', async () => {
    const service = uploadService();
    // What the real store answers while another transfer to the same slot is under way.
    service.script.put.push(() =>
      fail(409, 'conflict', 'another transfer to this upload is in progress'),
    );
    const { api } = setup(service);
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    upload();
    await notUploaded('a.txt');
    const row = rowOf('a.txt');
    expect(row.textContent).toContain(
      'Uploading a.txt: The service could not take this file’s bytes now.',
    );
    expect(row.textContent).toContain(
      'Try again: the upload goes on from what the service already holds.',
    );
    // It is not worded as a file the service refused, and no new slot is offered.
    expect(row.textContent).not.toContain('The service did not accept this file.');
    expect(within(row).queryByRole('button', { name: 'Send it again' })).toBeNull();
    fireEvent.click(within(row).getByRole('button', { name: 'Try again' }));
    await uploaded('a.txt');
    const creates = api.callsTo(CREATE);
    expect(creates).toHaveLength(2);
    expect(creates.at(1)).toEqual(creates.at(0));
    expect(service.slots).toHaveLength(1);
  });

  it('opens a new slot when the service no longer holds the upload', async () => {
    const service = uploadService();
    service.script.put.push(() => fail(404, 'not_found', 'no such upload'));
    const { api } = setup(service);
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    upload();
    await notUploaded('a.txt');
    const row = rowOf('a.txt');
    expect(row.textContent).toContain('Uploading a.txt: The service no longer holds this upload.');
    expect(row.textContent).toContain('Send the file again.');
    // No way out to the workspace list is offered here: the file is still the person's to send.
    expect(within(row).queryByRole('link')).toBeNull();
    fireEvent.click(within(row).getByRole('button', { name: 'Send it again' }));
    await uploaded('a.txt');
    const creates = api.callsTo(CREATE);
    expect(creates).toHaveLength(2);
    expect(creates.at(1)?.idempotency_key).not.toBe(creates.at(0)?.idempotency_key);
    expect(service.slots).toHaveLength(2);
    expect(service.slots.map((slot) => slot.complete)).toEqual([false, true]);
  });

  it('says the page cannot check a file where the browser gives it no hash, and sends nothing', async () => {
    const { api, sent } = setup();
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    // A page that is not secure: the browser gives it no `crypto.subtle`.
    const real = globalThis.crypto;
    vi.stubGlobal('crypto', { randomUUID: () => real.randomUUID() });
    upload();
    await notUploaded('a.txt');
    const row = rowOf('a.txt');
    expect(row.textContent).toContain(
      'Uploading a.txt: This page cannot check a file before sending it.',
    );
    expect(row.textContent).toContain('only on a secure page');
    expect(row.textContent).not.toContain('could not be reached');
    // Trying the same page again cannot help: no retry is offered.
    expect(within(row).queryByRole('button', { name: /again/i })).toBeNull();
    expect(api.callsTo(CREATE)).toEqual([]);
    expect(sent.transfers).toEqual([]);
  });

  it('sends nothing more to a slot that says it holds more than the file: the finish refuses it, and a new slot follows', async () => {
    const service = uploadService();
    service.script.create.push(async (_body, usual) => {
      const reply = await usual();
      return ok({ ...(reply.body as object), received_bytes: '5' });
    });
    const { api, sent } = setup(service);
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    upload();
    await notUploaded('a.txt');
    const row = rowOf('a.txt');
    // No byte was added to what the slot holds; the service's own check decided.
    expect(sent.transfers).toEqual([]);
    expect(api.callsTo(COMPLETE)).toHaveLength(1);
    expect(row.textContent).toContain('Uploading a.txt: The service did not accept this file.');
    fireEvent.click(within(row).getByRole('button', { name: 'Send it again' }));
    await uploaded('a.txt');
    expect(service.slots).toHaveLength(2);
    expect(api.callsTo(CREATE).at(1)?.idempotency_key).not.toBe(
      api.callsTo(CREATE).at(0)?.idempotency_key,
    );
  });

  it('opens a new slot when the service does not say how many bytes the slot holds', async () => {
    const service = uploadService();
    service.script.create.push(async (_body, usual) => {
      const reply = await usual();
      return ok({ ...(reply.body as object), received_bytes: 'some' });
    });
    const { api, sent } = setup(service);
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    upload();
    await notUploaded('a.txt');
    const row = rowOf('a.txt');
    expect(row.textContent).toContain(
      'Uploading a.txt: The service’s answer about this upload could not be read.',
    );
    expect(row.textContent).toContain('It did not say how many bytes of the file it holds.');
    // Nothing was sent on a guess, and nothing was finished.
    expect(sent.transfers).toEqual([]);
    expect(api.callsTo(COMPLETE)).toEqual([]);
    fireEvent.click(within(row).getByRole('button', { name: 'Send it again' }));
    await uploaded('a.txt');
    expect(service.slots).toHaveLength(2);
  });

  it('says the service holds part of the file, not that the file is too large, when a slot holds more than it says', async () => {
    const service = uploadService();
    // The bytes arrive and are kept; the answer is lost.
    service.script.put.push((transfer, usual) => {
      usual();
      transfer.breakConnection();
      return undefined;
    });
    const { api } = setup(service);
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    upload();
    await notUploaded('a.txt');
    // The service's record of the slot is behind the bytes it kept: read again, it says it holds
    // none (the real store brings the record up to date on other calls, not on this one).
    service.script.get.push(async (_body, usual) => {
      const reply = await usual();
      return ok({ ...(reply.body as object), received_bytes: '0' });
    });
    fireEvent.click(within(rowOf('a.txt')).getByRole('button', { name: 'Try again' }));
    await waitFor(() =>
      expect(rowOf('a.txt').textContent).toContain(
        'Uploading a.txt: The service already holds part of this file, and did not take the rest.',
      ),
    );
    const row = rowOf('a.txt');
    expect(row.textContent).toContain('Send it again: the file goes into a new upload.');
    expect(row.textContent).not.toContain('larger than the service accepts');
    expect(row.textContent).not.toContain('Remove it from the list');
    // The slot it leaves holds bytes, and the row says they stay.
    expect(row.textContent).toContain(
      'What was sent of it before stays with the service: it is listed under “Uploads the service holds”, where you can discard it.',
    );
    fireEvent.click(within(row).getByRole('button', { name: 'Send it again' }));
    await uploaded('a.txt');
    expect(service.slots).toHaveLength(2);
    expect(api.callsTo(CREATE).at(2)?.idempotency_key).not.toBe(
      api.callsTo(CREATE).at(0)?.idempotency_key,
    );
    expect(await service.held(service.slots.at(0))).toBe('abc');
    expect(service.slots.map((slot) => slot.complete)).toEqual([false, true]);
    expect(rowOf('a.txt').textContent).toContain(
      'What was sent of it before stays with the service',
    );
  });

  it('reads too large as too large when it is the slot, asked again, that is refused', async () => {
    const service = uploadService();
    service.script.put.push((transfer, usual) => {
      usual();
      transfer.breakConnection();
      return undefined;
    });
    setup(service);
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    upload();
    await notUploaded('a.txt');
    // Bytes were sent to this slot; now the service refuses the file itself, before any byte.
    service.script.create.push(() => fail(413, 'too_large', 'at most 2 bytes per file'));
    fireEvent.click(within(rowOf('a.txt')).getByRole('button', { name: 'Try again' }));
    await waitFor(() =>
      expect(rowOf('a.txt').textContent).toContain(
        'Uploading a.txt: This file is larger than the service accepts.',
      ),
    );
    expect(rowOf('a.txt').textContent).not.toContain('already holds part of this file');
    expect(within(rowOf('a.txt')).queryByRole('button', { name: /again/i })).toBeNull();
  });

  it('reads a first refusal of the bytes as too large for the service, with nothing left behind', async () => {
    const service = uploadService();
    service.script.put.push(() =>
      fail(413, 'too_large', 'the upload would exceed the limit; nothing was appended'),
    );
    setup(service);
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    upload();
    await notUploaded('a.txt');
    const row = rowOf('a.txt');
    expect(row.textContent).toContain(
      'Uploading a.txt: This file is larger than the service accepts.',
    );
    expect(within(row).queryByRole('button', { name: /again/i })).toBeNull();
    expect(row.textContent).not.toContain('stays with the service');
    // The service kept nothing of it: removing it leaves nothing to say.
    fireEvent.click(screen.getByRole('button', { name: 'Remove a.txt' }));
    expect(screen.queryByText(/stays with the service/)).toBeNull();
  });

  it('says on the row that the earlier upload stays with the service when a refused slot is left for a new one', async () => {
    const service = uploadService();
    service.script.complete.push(() => fail(409, 'conflict', 'the digest differs'));
    setup(service);
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc'), fileOf('b.txt', 'abc')]);
    upload();
    await notUploaded('a.txt');
    await uploaded('b.txt');
    expect(rowOf('a.txt').textContent).toContain(
      'What was sent of it before stays with the service: it is listed under “Uploads the service holds”, where you can discard it.',
    );
    // A file whose one slot was verified left nothing behind.
    expect(rowOf('b.txt').textContent).not.toContain('stays with the service');
  });

  it('says, when a file is removed, that what was sent of it stays with the service', async () => {
    const service = uploadService();
    // The first transfer is left open: that file stays under way.
    service.script.put.push(() => undefined);
    const { sent } = setup(service);
    await openImport();
    choose('Choose files…', [fileOf('under-way.txt', 'abc'), fileOf('listed.txt', 'abc')]);
    // Never sent: removing it says nothing of the service.
    fireEvent.click(screen.getByRole('button', { name: 'Remove listed.txt' }));
    expect(screen.queryByText(/stays with the service/)).toBeNull();
    upload();
    await waitFor(() => expect(sent.transfers).toHaveLength(1));
    choose('Choose files…', [fileOf('done.txt', 'abc')]);
    upload();
    await uploaded('done.txt');
    // Under way: its request is aborted, and what had gone is the service's.
    fireEvent.click(screen.getByRole('button', { name: 'Remove under-way.txt' }));
    const said = screen.getByText(
      '“under-way.txt” was removed from the list. What was already sent of it stays with the service: it is listed under “Uploads the service holds”, where you can discard it.',
    );
    expect(said.getAttribute('role')).toBe('status');
    // Uploaded and not imported: its slot is the service's too.
    fireEvent.click(screen.getByRole('button', { name: 'Remove done.txt' }));
    expect(
      screen.getByText(
        '“done.txt” was removed from the list. What was already sent of it stays with the service: it is listed under “Uploads the service holds”, where you can discard it.',
      ),
    ).toBeDefined();
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
    const { api, sent } = setup(service);
    await openImport();
    const file = fileOf('a.txt', 'abc');
    const read = vi.spyOn(file, 'arrayBuffer');
    choose('Choose files…', [file]);
    upload();
    await notUploaded('a.txt');
    fireEvent.click(within(rowOf('a.txt')).getByRole('button', { name: 'Try again' }));
    await uploaded('a.txt');
    // Each try reads the file once: nothing of it is kept in memory between two tries.
    expect(read).toHaveBeenCalledTimes(2);
    // The same slot, asked again under the same key with the same hash.
    const creates = api.callsTo(CREATE);
    expect(creates).toHaveLength(2);
    expect(creates.at(1)).toEqual(creates.at(0));
    // The slot said it holds the whole file: no byte was sent a second time.
    expect(sent.transfers).toHaveLength(1);
    expect(service.slots).toHaveLength(1);
    expect(await service.held(service.slots.at(0))).toBe('abc');
  });

  it('sends the bytes it hashed: one reading of the file gives both', async () => {
    const service = uploadService();
    const { api, sent } = setup(service);
    await openImport();
    // A file that is being written: every reading of it gives other content.
    const file = fileOf('a.txt', 'abc');
    let readings = 0;
    vi.spyOn(file, 'arrayBuffer').mockImplementation(() => {
      readings += 1;
      return Promise.resolve(new TextEncoder().encode(readings === 1 ? 'one' : 'two').buffer);
    });
    // A browser reads a File it is given to send; a reading taken here is not read again.
    const sliced = vi.spyOn(file, 'slice');
    choose('Choose files…', [file]);
    upload();
    await uploaded('a.txt');
    expect(readings).toBe(1);
    expect(sliced).not.toHaveBeenCalled();
    // What was sent is that reading, and not the File: the service, which hashes what arrives,
    // verified it against the hash that was announced.
    const transfer = sent.transfers.at(0);
    expect(transfer?.bytes).not.toBe(file);
    expect(await transfer?.bytes.text()).toBe('one');
    expect(await service.held(service.slots.at(0))).toBe('one');
    expect(api.callsTo(CREATE).at(0)?.sha256).toBe(api.callsTo(COMPLETE).at(0)?.sha256);
    expect(service.slots.at(0)?.complete).toBe(true);
  });

  it('opens a new slot when the file changed between two tries: the first slot was announced for other content', async () => {
    const service = uploadService();
    // The first try's bytes arrive in part, then the connection breaks.
    service.script.put.push((transfer) => {
      service.slots.at(0)?.parts.push(transfer.bytes.slice(0, 2));
      transfer.breakConnection();
      return undefined;
    });
    const { api, sent } = setup(service);
    await openImport();
    const file = fileOf('a.txt', 'abc');
    let readings = 0;
    vi.spyOn(file, 'arrayBuffer').mockImplementation(() => {
      readings += 1;
      return Promise.resolve(new TextEncoder().encode(readings === 1 ? 'abc' : 'xyz').buffer);
    });
    choose('Choose files…', [file]);
    upload();
    await notUploaded('a.txt');
    fireEvent.click(within(rowOf('a.txt')).getByRole('button', { name: 'Try again' }));
    await uploaded('a.txt');
    // The slot that holds "ab" of the old content is not finished with the new one's rest.
    const creates = api.callsTo(CREATE);
    expect(creates).toHaveLength(2);
    expect(creates.at(1)?.idempotency_key).not.toBe(creates.at(0)?.idempotency_key);
    expect(creates.at(1)?.sha256).not.toBe(creates.at(0)?.sha256);
    expect(service.slots).toHaveLength(2);
    expect(await sent.transfers.at(1)?.bytes.text()).toBe('xyz');
    expect(await service.held(service.slots.at(1))).toBe('xyz');
    expect(service.slots.map((slot) => slot.complete)).toEqual([false, true]);
    // The slot that holds "ab" stays where it is, and the row says so.
    expect(rowOf('a.txt').textContent).toContain(
      'What was sent of it before stays with the service: it is listed',
    );
  });

  it('asks nothing for a file whose length is not the one it was listed with, and says it changed', async () => {
    const { api, sent } = setup();
    await openImport();
    // Listed with 3 bytes; by the time it is read it has 5.
    const file = fileOf('a.txt', 'abc');
    vi.spyOn(file, 'arrayBuffer').mockResolvedValue(new TextEncoder().encode('abcde').buffer);
    choose('Choose files…', [file]);
    upload();
    await notUploaded('a.txt');
    const row = rowOf('a.txt');
    expect(row.textContent).toContain('Uploading a.txt: This file changed since you chose it.');
    expect(row.textContent).toContain(
      'It was listed with 3 bytes and now has 5. Nothing of it was sent. Remove it from the list and choose it again.',
    );
    // The same reading would come again: no retry is offered.
    expect(within(row).queryByRole('button', { name: /again/i })).toBeNull();
    expect(api.callsTo(CREATE)).toEqual([]);
    expect(sent.transfers).toEqual([]);
  });

  it('gives a file its turn only when it fits in memory beside the files under way, and keeps the order', async () => {
    setup();
    await openImport();
    // Two files that do not fit together, and a small one behind them.
    const half = MAX_FILE_BYTES / 2 + 1;
    const first = fileOfSize('first.bin', half);
    const second = fileOfSize('second.bin', half);
    let finishFirst: (read: ArrayBuffer) => void = () => undefined;
    vi.spyOn(first, 'arrayBuffer').mockReturnValue(
      new Promise<ArrayBuffer>((resolve) => {
        finishFirst = resolve;
      }),
    );
    const readSecond = vi.spyOn(second, 'arrayBuffer');
    const small = fileOf('small.txt', 'abc');
    const readSmall = vi.spyOn(small, 'arrayBuffer');
    choose('Choose files…', [first, second, small]);
    fireEvent.click(screen.getByRole('button', { name: 'Upload 3 files' }));
    await waitFor(() => expect(rowOf('first.bin').textContent).toContain('Reading the file…'));
    // The second would not fit beside the first; the small one would, and waits behind it.
    expect(rowOf('second.bin').textContent).toContain('Waiting for its turn.');
    expect(rowOf('small.txt').textContent).toContain('Waiting for its turn.');
    expect(readSecond).not.toHaveBeenCalled();
    expect(readSmall).not.toHaveBeenCalled();
    // The first ends (its stand-in does not have the length it reports): the others get their turn.
    finishFirst(new ArrayBuffer(3));
    await notUploaded('first.bin');
    await uploaded('small.txt');
    expect(readSecond).toHaveBeenCalledTimes(1);
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

  it('asks on the list of workspaces too, while a file is under way: the question follows the uploads', async () => {
    const service = uploadService();
    // The transfer is left open: the file is under way.
    service.script.put.push(() => undefined);
    const { sent } = setup(service, {
      [WORKSPACES]: () => ok({ items: [workspace], next_cursor: null }),
    });
    await openImport();
    choose('Choose files…', [fileOf('a.txt', 'abc')]);
    upload();
    await waitFor(() => expect(sent.transfers).toHaveLength(1));
    expect(leaving()).toBe(true);
    // The wordmark, on every page, leads to the list of workspaces: out of the workspace.
    fireEvent.click(screen.getByRole('link', { name: 'okf-jawn' }));
    await screen.findByRole('link', { name: 'Workshop notes' });
    expect(screen.queryByRole('heading', { name: 'Import files' })).toBeNull();
    // The upload goes on there, and closing the tab would end it.
    expect(sent.transfers.at(0)?.aborted).toBe(false);
    expect(leaving()).toBe(true);
    // Back in the workspace the file is still listed; removed, nothing is at risk anywhere.
    fireEvent.click(screen.getByRole('link', { name: 'Workshop notes' }));
    await screen.findByRole('heading', { name: 'Workshop notes', level: 1 });
    expect(leaving()).toBe(true);
    act(() => resetUploads());
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
