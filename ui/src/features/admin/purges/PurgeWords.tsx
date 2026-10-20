/**
 * What a purge removes and what it cannot erase, in SPEC section 8's words (ruling C3), said in
 * the dialog of a workspace purge and of an item purge alike:
 *
 * - "Purge is explicit and destructive: it removes the original bytes, derivatives, index
 *   entries, historical content and installation-managed backups of an item or a workspace, and
 *   marks the revisions, citations and Views that referred to them as invalidated."
 * - "Purge never claims to erase exports, clones or backups outside the application's control."
 * - "A purge does not complete while a managed backup or retained export that holds the target
 *   can still be restored or downloaded"
 * - "The purge leaves a record of what was removed, by identity, with no content."
 * - "After an item purge, a citation of another, unchanged item follows its rewritten revision and
 *   keeps its review coverage."
 *
 * A workspace purge also removes the exports the application retains of it, and an item purge
 * rewrites the managed backups that hold the item: the operations' own descriptions
 * (`purge_workspace`, `purge_item` in `api/operations.json`).
 */

/** The dialog's description: what confirming does, for a workspace or for one item. */
export function PurgeWords({ of }: { of: 'workspace' | 'item' }) {
  return (
    <>
      {of === 'workspace' ? (
        <p>
          A purge removes the original bytes, derivatives, index entries, historical content and
          installation-managed backups of this workspace, with the exports the application retains
          of it, and marks the revisions, citations and Views that referred to them as invalidated.
          It cannot be undone.
        </p>
      ) : (
        <p>
          A purge removes the original bytes, derivatives, index entries and historical content of
          this item, rewrites the installation-managed backups that hold it, and marks the
          revisions, citations and Views that referred to it as invalidated. A citation of another,
          unchanged item follows its rewritten revision and keeps its review coverage. It cannot be
          undone.
        </p>
      )}
      <p>
        A purge never erases exports, clones or backups outside the application’s control: a copy
        already downloaded, cloned or kept elsewhere stays as it is.
      </p>
      <p>
        It does not complete while a managed backup or a retained export that holds the {of} can
        still be restored or downloaded. It leaves a record of what was removed, by identity, with
        no content.
      </p>
    </>
  );
}

/** What a record says beside its counts: the copies outside the application stay. */
export const OUTSIDE_WORDS =
  'A purge never erases exports, clones or backups outside the application’s control.';
