/**
 * An original with no viewer (UI M4a, ruling A2; SPEC section 5: the original is retained even
 * when no extractor exists, and unsupported content is visible as unsupported): the page says in
 * words that it has no viewer for files of this media type, names the type and the size, and offers
 * the original's bytes as a download (`OriginalAccess.tsx`). Nothing is drawn, and nothing is
 * guessed about what the file holds.
 */
import { sizeWords } from '../../jobs/ArtifactDownload';
import type { OriginalProps } from '../types';
import { OriginalAccess } from './OriginalAccess';

export function OtherOriginal({ workspaceId, itemId, revision, path, appearance }: OriginalProps) {
  return (
    <div className="other-original grid gap-2">
      <p>
        There is no viewer here for files of this type (<code>{appearance.media_type}</code>). The
        original, {sizeWords(appearance.size)}, is kept: download it to open it with another
        program.
      </p>
      <OriginalAccess
        workspaceId={workspaceId}
        itemId={itemId}
        revision={revision}
        path={path}
        appearance={appearance}
      />
    </div>
  );
}
