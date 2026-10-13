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
 * A drag that carries no file (a row of the folder tree, a selection of text) is not touched.
 *
 * Limit. The listener is in the bubbling phase, so it does not switch off a part of the page that
 * handles a dropped file itself: the draft editor's library (CodeMirror) has a drop handler that
 * reads a dropped text file into the text, and this listener leaves it as it was. Moving the
 * listener to the capturing phase would switch that off too (CodeMirror skips an event that is
 * already cancelled); that is a decision about the editor, not made here.
 */
import { useEffect } from 'react';
import { carriesFiles } from './files';

function cancelFileDrag(event: DragEvent) {
  if (carriesFiles(event.dataTransfer)) event.preventDefault();
}

/** Cancel every drag and drop of files, wherever on the page it lands. Mount once, at the root. */
export function useFileDropGuard() {
  useEffect(() => {
    window.addEventListener('dragover', cancelFileDrag);
    window.addEventListener('drop', cancelFileDrag);
    return () => {
      window.removeEventListener('dragover', cancelFileDrag);
      window.removeEventListener('drop', cancelFileDrag);
    };
  }, []);
}
