/**
 * A file dragged from the computer and let go anywhere but on the import page's drop box must not
 * open in place of the app: a browser's own answer to an uncancelled drop of a file is to leave the
 * page for that file, and whatever the page held (files listed and not yet sent among it) is lost
 * without a question.
 *
 * So one listener on the window, mounted with the application's root (`routes/__root.tsx`) and
 * therefore on every route and whatever the folder tree shows, cancels the `dragover` and the
 * `drop` of every drag that carries files. Cancelling is all it does: it lists nothing and starts
 * no upload. Only the drop box takes files (`ImportPage.tsx`): it reads the drop in its own
 * handler and cancels nothing itself, so this listener is also what makes a browser deliver a
 * drop to it (a browser delivers a drop only where the `dragover` was cancelled).
 *
 * A drag that carries no file (a row of the folder tree, a selection of text, text dragged in from
 * another application) is not touched: it is told from a file drag by the `Files` entry in the
 * transfer's types, which a file drag has and a text drag has not.
 *
 * One answer everywhere. The listeners are in the capturing phase, so a drag of files is cancelled
 * before any part of the page sees it. The draft editor's library (CodeMirror) skips an event that
 * is already cancelled, so it never reads a dropped text file into a draft: whether the folder tree
 * is listed (its drag backend cancels drags too) makes no difference. A text drag still reaches the
 * editor and inserts.
 */
import { useEffect } from 'react';
import { carriesFiles } from './files';

function cancelFileDrag(event: DragEvent) {
  if (carriesFiles(event.dataTransfer)) event.preventDefault();
}

/** Cancel every drag and drop of files, wherever on the page it lands. Mount once, at the root. */
export function useFileDropGuard() {
  useEffect(() => {
    window.addEventListener('dragover', cancelFileDrag, { capture: true });
    window.addEventListener('drop', cancelFileDrag, { capture: true });
    return () => {
      window.removeEventListener('dragover', cancelFileDrag, { capture: true });
      window.removeEventListener('drop', cancelFileDrag, { capture: true });
    };
  }, []);
}
