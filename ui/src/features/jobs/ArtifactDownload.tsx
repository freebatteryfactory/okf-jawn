/**
 * What a finished job produced, and its download (ruling B4): the artifact's kind in words, its
 * size (exact, and about how large), its SHA-256, and a plain link to it (`<a href download>`) when
 * its own rule offers it to this person in this session (`download.ts`); otherwise the words that
 * say why, where the link would be.
 *
 * It is shown only for a job the service answered as succeeded (`JobRow`'s outcome): nothing here
 * infers that an export finished. The link is not a request of the page's: the browser follows it
 * and saves the file, and the service decides again whether to serve it.
 */
import type { AccessRoute, ArtifactKind, DownloadArtifact } from '../../api/generated/types.gen';
import { wordsFor } from '../../lib/words';
import { Missing } from '../explorer/rights';
import { describeSize } from '../imports/files';
import { ROUTE_WORDS } from '../tree/Committed';
import { type Download, downloadOf } from './download';
import { useControlRights } from './job-control';

/** What an artifact is, inside a sentence. */
export const ARTIFACT: Record<ArtifactKind, string> = {
  export: 'an export of the workspace',
  workspace_backup: 'a backup of the workspace',
  installation_backup: 'a backup of the installation',
  view_export: 'an export of a View',
};

/** What the link downloads. */
const DOWNLOADS: Record<ArtifactKind, string> = {
  export: 'the export of the workspace',
  workspace_backup: 'the backup of the workspace',
  installation_backup: 'the backup of the installation',
  view_export: 'the export of the View',
};

/** A size off the wire (a decimal string): exact, grouped, and about how large from 1 KB on. */
export function sizeWords(size: string): string {
  if (!/^\d+$/.test(size)) return `${size} bytes`;
  const exact = BigInt(size).toLocaleString('en');
  const bytes = Number(size);
  if (bytes < 1024) return `${exact} ${bytes === 1 ? 'byte' : 'bytes'}`;
  return `${exact} bytes (${describeSize(bytes)})`;
}

/** A route in words; one this build does not know, by the service's own word. */
const routeSaid = (route: AccessRoute) => ROUTE_WORDS[route] ?? `“${route}”`;

/** The routes a download is allowed from, in words: "a, b or c". */
function routesSaid(routes: readonly AccessRoute[]): string {
  const said = routes.map(routeSaid);
  const last = said.at(-1) ?? '';
  return said.length > 1 ? `${said.slice(0, -1).join(', ')} or ${last}` : last;
}

/** Why the link is not offered, as a sentence, for a reason other than a missing permission. */
function whyNot(
  offer: Extract<Download, { kind: 'unknown-kind' | 'outside' | 'not-this-session' }>,
  what: string,
): string {
  switch (offer.kind) {
    case 'unknown-kind':
      return 'This page does not know who may download something of this kind, so it offers no link to it.';
    case 'outside':
      return `The service named a download path outside this application (“${offer.path}”), so this page offers no link to it.`;
    case 'not-this-session':
      return `Only ${routesSaid(offer.routes)} may download ${what}; this session is ${routeSaid(offer.route)}.`;
  }
}

export interface ArtifactDownloadProps {
  workspaceId: string;
  artifact: DownloadArtifact;
}

export function ArtifactDownload({ workspaceId, artifact }: ArtifactDownloadProps) {
  const rights = useControlRights(workspaceId);
  const offer = downloadOf(artifact, rights.permissions, rights.route);
  const what =
    wordsFor(ARTIFACT, artifact.kind) ??
    `something of a kind this page does not know (“${artifact.kind}”)`;
  return (
    <>
      <p>
        It produced {what}: {sizeWords(artifact.size)}.
      </p>
      <p>
        SHA-256: <code>{artifact.sha256}</code>
      </p>
      {offer.kind === 'offered' ? (
        <p>
          <a className="underline" href={offer.href} download>
            Download {wordsFor(DOWNLOADS, artifact.kind)}
          </a>
        </p>
      ) : offer.kind === 'no-permission' ? (
        <Missing action={`Downloading ${what}`} permission={offer.permission} />
      ) : (
        <p className="meta">{whyNot(offer, what)}</p>
      )}
    </>
  );
}
