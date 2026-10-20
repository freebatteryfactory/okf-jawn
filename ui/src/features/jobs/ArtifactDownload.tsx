/**
 * What a finished job produced, and its download (ruling B4): the artifact's kind in words, its
 * size (exact, and about how large), its SHA-256, and a plain link to it (`<a href download>`) when
 * its own rule offers it to this person in this session (`download.ts`); otherwise the words that
 * say why, where the link would be.
 *
 * It is shown only for a job the service answered as succeeded (`JobRow`'s outcome, and the row of
 * a job of the installation, `admin/installation/TenantJobRow.tsx`): nothing here infers that an
 * export or a backup finished. The link is not a request of the page's: a plain
 * `<a href={download_path} download>` is the one exception to the transport rule (ruling C6; an
 * archive can be larger than memory, see `download.ts`). The browser follows it and saves the
 * file, and the service decides again whether to serve it.
 *
 * Whose permissions decide. A workspace's artifact is offered by the person's permissions in that
 * workspace, as `open_workspace` answered them (`ArtifactDownload`). An artifact of the
 * installation (its backup) belongs to no workspace, and the contract names no tenant grant a
 * client can read: it is listed by `list_tenant_jobs`, which the service answers only to an
 * administrator of the installation, so a row it answered is shown with `admin` held there
 * (`TenantArtifactDownload`). The session's route is the one `get_session` answered either way.
 */
import { useQuery } from '@tanstack/react-query';
import type {
  AccessRoute,
  ArtifactKind,
  DownloadArtifact,
  Permission,
} from '../../api/generated/types.gen';
import { wordsFor } from '../../lib/words';
import { Missing } from '../explorer/rights';
import { describeSize } from '../imports/files';
import { sessionOptions } from '../imports/limits';
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

/** An artifact of a workspace, offered by the person's permissions there. */
export function ArtifactDownload({ workspaceId, artifact }: ArtifactDownloadProps) {
  const rights = useControlRights(workspaceId);
  return (
    <ArtifactOffer artifact={artifact} permissions={rights.permissions} route={rights.route} />
  );
}

/**
 * What the person holds on the installation, as far as a page can know it: the service answered
 * `list_tenant_jobs`, whose permission is `admin` (see the header).
 */
const LISTED_TENANT_JOBS: readonly Permission[] = ['admin'];

/** An artifact of the installation, from a row `list_tenant_jobs` answered. */
export function TenantArtifactDownload({ artifact }: { artifact: DownloadArtifact }) {
  const session = useQuery({ ...sessionOptions(), enabled: false });
  return (
    <ArtifactOffer
      artifact={artifact}
      permissions={LISTED_TENANT_JOBS}
      route={session.data?.principal.route ?? null}
    />
  );
}

interface ArtifactOfferProps {
  artifact: DownloadArtifact;
  /** What the person holds where the artifact lives. */
  permissions: readonly Permission[];
  /** Their session's route; null while it is not read. */
  route: AccessRoute | null;
}

function ArtifactOffer({ artifact, permissions, route }: ArtifactOfferProps) {
  const offer = downloadOf(artifact, permissions, route);
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
