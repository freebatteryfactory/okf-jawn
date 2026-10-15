/**
 * Paths the person did not mean: what the import page does not send because it can see it is
 * wrong, and how a name is drawn so that it cannot read as another. The service stays the
 * authority; these are the page's own refusals, before anything is sent.
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
import { z } from 'zod';
import { tidyPath } from '../../src/features/imports/files';
import {
  collisionKey,
  describeProblem,
  pathProblem,
  shownName,
} from '../../src/features/imports/paths';
import { MAX_FILE_BYTES } from '../../src/features/imports/upload-list';
import { resetUploads } from '../../src/features/imports/uploading';
import { mockApi, ok } from './support/api';
import {
  CREATE,
  dropOf,
  droppedFile,
  droppedFolder,
  fileOf,
  fileOfSize,
  uploadService,
} from './support/imports';
import { mockTransfers } from './support/transfers';
import { explorerHandlers, renderAt, WS, writableWorkspace } from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const RLO = '\u202E';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  act(() => resetUploads());
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

async function openImport() {
  const service = uploadService();
  const api = mockApi(
    explorerHandlers({ [OPEN]: () => ok(writableWorkspace), ...service.handlers }),
  );
  const sent = mockTransfers(service.transfer);
  renderAt(`/w/${WS}/import`);
  await screen.findByRole('heading', { name: 'Import files' });
  return { api, sent, service };
}

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
}

const rows = () =>
  within(screen.getByRole('list', { name: /^Files to import/ })).getAllByRole('listitem');
/** The row whose path, as drawn, is `path`. */
const rowOf = (path: string) => {
  const row = rows().find(
    (candidate) => candidate.querySelector('.upload-path')?.textContent === path,
  );
  if (row === undefined) throw new Error(`no row for ${path}`);
  return row;
};
const wire = (api: ReturnType<typeof mockApi>) =>
  api.callsTo(CREATE).map((body) => [body.relative_path, body.filename]);
const uploadButton = () => screen.getByRole('button', { name: /^Upload \d+ files?$/ });

describe('a path the page can see is wrong', () => {
  it.each([
    ['a “..” step', 'top/../../etc/a.txt', 'parent'],
    ['a “..” step at its start', '../a.txt', 'parent'],
    ['a root', '/abs/b.txt', 'absolute'],
    ['a network share', '//server/share/b.txt', 'absolute'],
    ['a disk drive', 'C:/Users/x/c.txt', 'drive'],
    ['a disk drive in a small letter', 'c:/Users/x/c.txt', 'drive'],
    ['a line feed', 'f/line\nfeed.txt', 'control'],
    ['a tab', 'f/a\tb.txt', 'control'],
    ['a line feed in a folder’s name', 'line\nfeed/a.txt', 'control'],
    ['a control character in the second folder’s name', 'ok/a\u0007b/c.txt', 'control'],
    ['a delete character', 'f/a\u007Fb.txt', 'control'],
    ['a C1 control', 'f/a\u0085b.txt', 'control'],
  ])('is a problem for %s', (_what, path, kind) => {
    expect(pathProblem(tidyPath(path))?.kind).toBe(kind);
  });

  it.each([
    ['a plain path', 'reports/2025/a.txt'],
    ['a name with dots that is no step', 'reports/..a/b...txt'],
    ['a single dot step', './a.txt'],
    ['a colon inside a name', 'notes/12:30 call.txt'],
    ['a name that only starts like a drive', 'C:notes.txt'],
    ['a step that looks like a drive below the first', 'docs/C:/x.txt'],
    ['letters of other scripts and a space', 'заметки/会議 メモ.txt'],
  ])('is none for %s', (_what, path) => {
    expect(pathProblem(tidyPath(path))).toBeNull();
  });

  it('finds a name or a folder over the length the request schema allows, and says the bound', () => {
    const limits = { filename: z.string().max(8), folder: z.string().max(12) };
    expect(pathProblem({ folder: 'docs', filename: '12345.txt' }, limits)).toEqual({
      kind: 'too-long',
      part: 'name',
      max: 8,
    });
    expect(pathProblem({ folder: 'docs', filename: '1234.txt' }, limits)).toBeNull();
    expect(pathProblem({ folder: 'a-folder-of-13', filename: 'a.txt' }, limits)).toEqual({
      kind: 'too-long',
      part: 'folder',
      max: 12,
    });
    expect(pathProblem({ folder: 'a-folder-12c', filename: 'a.txt' }, limits)).toBeNull();
    expect(describeProblem({ kind: 'too-long', part: 'name', max: 8 })).toBe(
      'its name is longer than the service accepts (8 characters at most).',
    );
    expect(describeProblem({ kind: 'too-long', part: 'folder', max: 12 })).toBe(
      'its folder path is longer than the service accepts (12 characters at most).',
    );
  });

  it('finds no path too long: the generated request schema states its bounds in bytes, not as lengths', () => {
    // `CreateUploadRequest` states its bounds as `x-max-utf8-bytes`, which the generated zod
    // schema does not carry; the page's own count of bytes is UI part 2's.
    expect(
      pathProblem({ folder: 'x'.repeat(5000), filename: `${'y'.repeat(5000)}.txt` }),
    ).toBeNull();
  });

  it('is listed as one that cannot be imported, with the reason, and is not sent', async () => {
    const { api } = await openImport();
    choose('Choose a folder…', [
      fileOf('a.txt', 'abc', 'top/../../etc/a.txt'),
      fileOf('b.txt', 'abc', '/abs/b.txt'),
      fileOf('c.txt', 'abc', 'C:\\Users\\x\\c.txt'),
      fileOf('line\nfeed.txt', 'abc', 'f/line\nfeed.txt'),
      fileOf('ok.txt', 'abc', 'f/ok.txt'),
    ]);
    expect(screen.getByText('5 files added.')).toBeDefined();
    const said = (path: string) => rowOf(path).querySelector('.upload-state')?.textContent;
    expect(said('top/../../etc/a.txt')).toBe(
      'Cannot be imported: its path has a “..” step, which leads out of the folder it came in. It will not be sent.',
    );
    expect(said('/abs/b.txt')).toBe(
      'Cannot be imported: its path starts at the top of a disk or of a network share, not in a folder you chose. It will not be sent.',
    );
    expect(said('C:/Users/x/c.txt')).toBe(
      'Cannot be imported: its path names a disk drive, such as “C:”. It will not be sent.',
    );
    // The line feed is drawn as a mark, never as a break in the list.
    expect(said('f/line⟨U+000A⟩feed.txt')).toBe(
      'Cannot be imported: its name holds a control character, such as a line break. It will not be sent.',
    );
    expect(said('f/ok.txt')).toBe('Not sent yet.');
    // Only the one good file is offered, sent and counted.
    expect(uploadButton().textContent).toBe('Upload 1 file');
    fireEvent.click(uploadButton());
    await screen.findByText('The file is uploaded and checked.');
    expect(wire(api)).toEqual([['f', 'ok.txt']]);
    expect(rowOf('/abs/b.txt').textContent).toContain('Cannot be imported');
    expect(api.violations).toEqual([]);
  });
});

describe('backslashes', () => {
  it('are path separators, from a picker and from a drop', async () => {
    const { api } = await openImport();
    choose('Choose a folder…', [
      fileOf('e.txt', 'abc', 'win\\dir\\sub/e.txt'),
      // A browser may put the whole Windows path in the name.
      fileOf('reports\\2025\\q1.txt', 'abc'),
    ]);
    const target = screen.getByRole('group', { name: 'Choose what to import' });
    await screen.findByRole('treeitem', { name: 'notes' });
    const drop = dropOf([droppedFolder('in\\box', [droppedFile(fileOf('f.txt', 'abc'))])]);
    fireEvent.dragEnter(target, { dataTransfer: drop });
    fireEvent.dragOver(target, { dataTransfer: drop });
    fireEvent.drop(target, { dataTransfer: drop });
    await screen.findByText('1 file added.');
    expect(rows().map((row) => row.querySelector('.upload-path')?.textContent)).toEqual([
      'win/dir/sub/e.txt',
      'reports/2025/q1.txt',
      'in/box/f.txt',
    ]);
    fireEvent.click(uploadButton());
    await screen.findByText('All 3 files are uploaded and checked.');
    expect(wire(api)).toEqual([
      ['win/dir/sub', 'e.txt'],
      ['reports/2025', 'q1.txt'],
      ['in/box', 'f.txt'],
    ]);
  });
});

describe('two paths that are the same place', () => {
  it('have one key when they differ only in letter case, or in how a letter is composed', () => {
    const key = (path: string) => collisionKey(tidyPath(path));
    expect(key('f/Case.txt')).toBe(key('f/case.TXT'));
    // "é" as one character, and as "e" with a combining accent (how a Mac names a file).
    expect(key('caf\u00E9.txt')).toBe(key('cafe\u0301.txt'));
    expect(key('f/a.txt')).not.toBe(key('g/a.txt'));
    expect(key('f/a.txt')).not.toBe(key('f/a.txt '));
  });

  it('are both marked, and neither is sent until one is removed', async () => {
    const { api } = await openImport();
    choose('Choose a folder…', [
      fileOf('Case.txt', 'abc', 'f/Case.txt'),
      fileOf('case.txt', 'abcd', 'f/case.txt'),
      fileOf('other.txt', 'abc', 'f/other.txt'),
    ]);
    expect(rowOf('f/Case.txt').textContent).toContain(
      'Its path is the same as that of “f/case.txt” in this list, or differs only in capital and small letters: both cannot be imported to one place. It is not sent while both are listed: remove one of them.',
    );
    expect(rowOf('f/case.txt').textContent).toContain(
      'Its path is the same as that of “f/Case.txt” in this list',
    );
    expect(rowOf('f/other.txt').textContent).not.toContain('Its path is the same');
    expect(uploadButton().textContent).toBe('Upload 1 file');
    fireEvent.click(uploadButton());
    await screen.findByText('The file is uploaded and checked.');
    expect(wire(api)).toEqual([['f', 'other.txt']]);
    expect(rowOf('f/Case.txt').textContent).toContain('Not sent yet.');
    expect(rowOf('f/case.txt').textContent).toContain('Not sent yet.');
    // One is removed: the other is free, and is sent.
    fireEvent.click(screen.getByRole('button', { name: 'Remove f/case.txt' }));
    expect(rowOf('f/Case.txt').textContent).not.toContain('Its path is the same');
    expect(uploadButton().textContent).toBe('Upload 1 file');
    fireEvent.click(uploadButton());
    await waitFor(() =>
      expect(wire(api)).toEqual([
        ['f', 'other.txt'],
        ['f', 'Case.txt'],
      ]),
    );
  });

  it('holds back a file listed after one of the same place was sent, and says so on both', async () => {
    const { api } = await openImport();
    choose('Choose files…', [fileOf('Notes.md', 'abc')]);
    fireEvent.click(uploadButton());
    await screen.findByText('The file is uploaded and checked.');
    choose('Choose files…', [fileOf('notes.md', 'abcd')]);
    expect(rowOf('notes.md').textContent).toContain(
      'It is not sent while both are listed: remove one of them.',
    );
    // The one already with the service is marked too; it is the import that would collide.
    expect(rowOf('Notes.md').textContent).toContain(
      'Its path is the same as that of “notes.md” in this list',
    );
    expect(rowOf('Notes.md').textContent).toContain('Remove one of them before you import.');
    expect(uploadButton().textContent).toBe('Upload 0 files');
    expect(uploadButton().getAttribute('aria-disabled')).toBe('true');
    expect(wire(api)).toEqual([['', 'Notes.md']]);
  });

  it('is not a collision with a file that is never sent, nor with a file of another workspace', async () => {
    const elsewhere = '12121212-1212-4212-8212-121212121212';
    const service = uploadService();
    mockApi(
      explorerHandlers({
        [OPEN]: (body) => ok({ ...writableWorkspace, id: body.workspace_id }),
        ...service.handlers,
      }),
    );
    mockTransfers(service.transfer);
    const { router } = renderAt(`/w/${WS}/import`);
    await screen.findByRole('heading', { name: 'Import files' });
    // Too large to be sent, and one of the same place that can be.
    choose('Choose files…', [fileOfSize('BIG.bin', MAX_FILE_BYTES + 1), fileOf('big.bin', 'abc')]);
    expect(rowOf('big.bin').textContent).not.toContain('Its path is the same');
    expect(rowOf('BIG.bin').textContent).not.toContain('Its path is the same');
    expect(uploadButton().textContent).toBe('Upload 1 file');
    // The same place in another workspace is another place.
    await act(() =>
      router.navigate({ to: '/w/$workspaceId/import', params: { workspaceId: elsewhere } }),
    );
    await screen.findByText('No files are listed yet.');
    choose('Choose files…', [fileOf('Big.bin', 'abcd')]);
    expect(rowOf('Big.bin').textContent).not.toContain('Its path is the same');
    expect(uploadButton().textContent).toBe('Upload 1 file');
    // And the button does what it says: the file is sent, not held back by the other workspace's.
    fireEvent.click(uploadButton());
    await waitFor(() =>
      expect(rowOf('Big.bin').textContent).toContain('Uploaded: the service checked'),
    );
    expect(service.slots.map((slot) => slot.filename)).toEqual(['Big.bin']);
  });
});

describe('a name as it is drawn', () => {
  it.each([
    ['a right-to-left override', `evil${RLO}txt.exe`, 'evil⟨U+202E⟩txt.exe'],
    ['a zero-width space', 'in\u200Bvoice.pdf', 'in⟨U+200B⟩voice.pdf'],
    ['a left-to-right isolate', 'a\u2066b', 'a⟨U+2066⟩b'],
    ['a byte-order mark', '\uFEFFreadme.md', '⟨U+FEFF⟩readme.md'],
    ['a soft hyphen', 're\u00ADport', 're⟨U+00AD⟩port'],
    ['a line separator', 'a\u2028b', 'a⟨U+2028⟩b'],
    ['a blank filler letter', 'a\u3164b', 'a⟨U+3164⟩b'],
    ['a control character', 'a\u0007b', 'a⟨U+0007⟩b'],
    ['a tag character beyond the basic plane', 'a\u{E0041}b', 'a⟨U+E0041⟩b'],
  ])('shows %s as a mark of its code point', (_what, name, drawn) => {
    expect(shownName(name)).toBe(drawn);
  });

  it('leaves every character that is drawn as it is', () => {
    const name = 'Résumé (final) – 会議メモ & заметки_v2.tar.gz';
    expect(shownName(name)).toBe(name);
  });

  it('cannot read as another name on the page, while the name itself is what is sent', async () => {
    const { api } = await openImport();
    // Drawn by a browser, the override makes this read "evilexe.txt".
    const name = `evil${RLO}txt.exe`;
    choose('Choose a folder…', [fileOf(name, 'abc', `f/${name}`)]);
    const row = rowOf('f/evil⟨U+202E⟩txt.exe');
    expect(row.textContent).not.toContain(RLO);
    // The name is isolated from the words beside it, whatever direction it is written in.
    expect(row.querySelector('.upload-path')?.tagName).toBe('BDI');
    expect(screen.getByRole('button', { name: 'Remove f/evil⟨U+202E⟩txt.exe' })).toBeDefined();
    fireEvent.click(uploadButton());
    await screen.findByText('The file is uploaded and checked.');
    expect(wire(api)).toEqual([['f', name]]);
    expect(document.body.textContent).not.toContain(RLO);
  });

  it('is drawn the same way where a folder of a drop is named', async () => {
    await openImport();
    const target = screen.getByRole('group', { name: 'Choose what to import' });
    await screen.findByRole('treeitem', { name: 'notes' });
    const drop = dropOf([droppedFolder(`in${RLO}xob`, [])]);
    fireEvent.dragEnter(target, { dataTransfer: drop });
    fireEvent.dragOver(target, { dataTransfer: drop });
    fireEvent.drop(target, { dataTransfer: drop });
    await screen.findByText(
      'No file was added to the list. The folder “in⟨U+202E⟩xob” is empty: an empty folder is not kept.',
    );
  });
});
