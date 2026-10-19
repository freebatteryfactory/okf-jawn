/**
 * Changes: a comparison of two explicit versions, shared by MCP Apps and the Explorer (SPEC
 * section 9: "Components are shared with the Explorer").
 *
 * It names both versions, shortened, each with its whole revision in a title, and keeps both
 * references: what is shown is exactly the comparison the service answered, never today's
 * documents in place of a historical input. Each changed file goes through the one diff reader
 * (`changed-files.tsx`): what happened to it in words, then its lines, without Git's patch header
 * (SPEC section 3: "Ordinary editing and navigation must not require Git vocabulary"). The lines
 * are bounded over the whole comparison and the rest counted. Previous and next windows make
 * every line available, and long lines have bounded chunks; a confirmation never hides a
 * proposed suffix or a later file without a way to inspect it (ruling R5). The comparison's own
 * warnings are notes.
 *
 * It takes no data of its own: the caller passes the answer, and the place it is shown in gives
 * it its name and heading level (MCP Apps shows it as a page's part, "Changes", at level two).
 */
import { useRef, useState } from 'react';
import type { z } from 'zod';
import type { zDiffResponse } from '../../api/generated/zod.gen';
import { keyed } from '../../lib/keys';
import { short } from '../documents/describe';
import { MAX_DIFF_LINES } from '../drafts/patch';
import { ChangedFile, keyedFiles, MoreLines, readChanges } from './changed-files';

export interface ChangesProps {
  result: z.infer<typeof zDiffResponse>;
  /** The comparison's name where it is shown; "Changes" by default. */
  title?: string;
  /** Its heading's level; two by default. */
  level?: 2 | 3 | 4 | 5;
}

export function Changes(props: ChangesProps) {
  // An explicit comparison has its own inspection position; another revision starts at its first
  // window, even when the containing page keeps this component in the same place.
  return <ChangesWindow key={`${props.result.from}:${props.result.to}`} {...props} />;
}

function ChangesWindow({ result, title = 'Changes', level = 2 }: ChangesProps) {
  const Heading = `h${level}` as const;
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const [start, setStart] = useState(0);
  const { files, before, more } = readChanges(result.changes, MAX_DIFF_LINES, {
    start,
    inspectLongLines: true,
  });
  const moveTo = (next: number) => {
    setStart(next);
    headingRef.current?.focus();
  };
  return (
    <section aria-label={title} className="changes">
      <Heading ref={headingRef} tabIndex={-1}>
        {title}
      </Heading>
      <p className="meta">
        From revision <code title={result.from}>{short(result.from)}</code> to revision{' '}
        <code title={result.to}>{short(result.to)}</code>.
      </p>
      {result.changes.length === 0 ? (
        <p>Nothing changed between these versions.</p>
      ) : (
        keyedFiles(files).map(({ key, value: file }) => (
          <ChangedFile
            key={key}
            change={file.change}
            hunks={file.hunks}
            hiddenLines={file.hiddenLines}
          />
        ))
      )}
      <MoreLines more={more} />
      {(before > 0 || more > 0) && (
        <p className="meta">
          {before > 0 ? `${before.toLocaleString('en')} earlier lines are on previous pages. ` : ''}
          <button
            type="button"
            aria-disabled={before === 0}
            onClick={() => {
              if (before > 0) moveTo(Math.max(0, start - MAX_DIFF_LINES));
            }}
          >
            Previous changed lines
          </button>{' '}
          <button
            type="button"
            aria-disabled={more === 0}
            onClick={() => {
              if (more > 0) moveTo(start + MAX_DIFF_LINES);
            }}
          >
            Next changed lines
          </button>
        </p>
      )}
      {keyed(result.warnings, (warning) => JSON.stringify(warning)).map(
        ({ key, value: warning }) => (
          <p key={key} role="note" className="notice">
            {warning.message}
            {warning.location ? ` (${warning.location})` : ''}
          </p>
        ),
      )}
    </section>
  );
}
