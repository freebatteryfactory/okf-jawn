/**
 * The words of an event of the service's logs (ruling C8): its kind, who caused it, what it
 * touched, what was refused and how many occurrences one event stands for.
 *
 * Every table here is keyed by a generated union (`EventKind`, `AccessRoute`, `TransportName`,
 * `OperationName`), so a value added to the contract fails type checking until it has words. A
 * service newer than this build can still answer a value those types do not name; it is read
 * through `wordsFor` and said as a value this version does not know, never as a blank.
 *
 * The security kinds are the contract's `EventKind::is_security` (`crates/contract/src/events.rs`):
 * a sign-in or a refused one, a connector issued or revoked, and a refused request. In a
 * workspace's log only the refused request occurs; the others are the installation's.
 */
import type {
  AccessRoute,
  Event,
  EventActor,
  EventKind,
  OperationName,
  TransportName,
} from '../../../api/generated/types.gen';
import { wordsFor } from '../../../lib/words';
import { short } from '../../documents/describe';

/** The kinds listed as security events: the contract's `EventKind::is_security`. */
export const SECURITY_KINDS: ReadonlySet<string> = new Set<EventKind>([
  'connector_issued',
  'connector_revoked',
  'signed_in',
  'sign_in_refused',
  'permission_denied',
]);

interface KindWords {
  /** What happened, as an event's title. */
  said: string;
  /** One event of the kind, counted. */
  one: string;
  /** Several, counted. */
  many: string;
}

const KINDS: Record<EventKind, KindWords> = {
  changed: { said: 'Content changed', one: 'content change', many: 'content changes' },
  imported: { said: 'An import added items', one: 'import', many: 'imports' },
  job_updated: { said: 'A job changed', one: 'job update', many: 'job updates' },
  reviewed: { said: 'A review was recorded', one: 'review', many: 'reviews' },
  proposal_updated: {
    said: 'A proposal changed',
    one: 'proposal update',
    many: 'proposal updates',
  },
  connector_issued: {
    said: 'A connector was issued',
    one: 'connector issued',
    many: 'connectors issued',
  },
  connector_revoked: {
    said: 'A connector was revoked',
    one: 'connector revoked',
    many: 'connectors revoked',
  },
  signed_in: { said: 'A person signed in', one: 'sign-in', many: 'sign-ins' },
  sign_in_refused: {
    said: 'A sign-in was refused',
    one: 'refused sign-in',
    many: 'refused sign-ins',
  },
  permission_denied: {
    said: 'A request was refused',
    one: 'refused request',
    many: 'refused requests',
  },
};

/** How a person or a program came to the service, as an actor's route is said. */
export const ROUTE_WORDS: Record<AccessRoute, string> = {
  local_owner: 'as the installation’s local owner, in the browser on this computer',
  browser_session: 'in a browser session',
  mcp_delegation: 'through an MCP client (a connector or a delegated agent)',
  service: 'as a service of the installation',
};

/** A refused byte transport (a download, or an upload's bytes), as what was refused. */
const TRANSPORT_WORDS: Record<TransportName, string> = {
  upload_content: 'sending a file’s bytes into an upload',
  download_object: 'downloading an original or converted file',
  download_artifact: 'downloading an export or a backup of a workspace',
  download_tenant_artifact: 'downloading a backup of the installation',
};

/** A refused operation, as what was refused. */
const OPERATION_WORDS: Record<OperationName, string> = {
  list_workspaces: 'listing the workspaces',
  create_workspace: 'creating a workspace',
  open_workspace: 'opening a workspace',
  update_workspace: 'changing a workspace’s name or description',
  archive_workspace: 'archiving a workspace',
  unarchive_workspace: 'unarchiving a workspace',
  purge_workspace: 'purging a workspace',
  get_purge: 'reading a purge’s record',
  export_workspace: 'exporting a workspace',
  backup_workspace: 'backing up a workspace',
  restore_workspace: 'restoring a workspace from a backup',
  backup_installation: 'backing up the installation',
  list_items: 'listing a folder',
  get_item: 'reading an item’s Markdown and properties',
  create_item: 'creating a note',
  save_draft: 'saving a draft',
  list_drafts: 'listing drafts',
  discard_draft: 'discarding a draft',
  move_item: 'moving an item',
  set_lifecycle: 'changing an item’s lifecycle',
  delete_item: 'removing an item',
  purge_item: 'purging an item',
  create_folder: 'creating a folder',
  move_folder: 'moving a folder',
  delete_folder: 'deleting a folder',
  list_types: 'listing the types',
  set_type: 'saving a type',
  read_item: 'reading an item',
  get_sources: 'reading an item’s sources',
  get_object: 'reading an original file',
  create_sandbox_capability: 'opening a file in the sandbox',
  search_items: 'searching',
  get_links: 'reading an item’s links',
  get_graph: 'reading the graph of links',
  log_items: 'reading the history',
  diff_items: 'comparing versions',
  commit_items: 'taking a snapshot',
  restore_items: 'rewinding items',
  blame_item: 'reading who changed each line',
  open_proposal: 'proposing a change',
  list_proposals: 'listing proposals',
  get_proposal: 'reading a proposal',
  accept_proposal: 'approving a proposal',
  decline_proposal: 'declining a proposal',
  add_comment: 'commenting on a proposal',
  list_comments: 'reading a proposal’s discussion',
  create_confirmation: 'confirming a verification or an approval',
  create_review: 'verifying an item',
  list_reviews: 'listing reviews',
  create_upload: 'adding files',
  complete_upload: 'finishing an upload',
  get_upload: 'reading an upload',
  list_uploads: 'listing uploads',
  discard_upload: 'discarding an upload',
  start_import: 'starting an import',
  get_job: 'reading a job',
  list_jobs: 'listing jobs',
  get_tenant_job: 'reading a job of the installation',
  list_tenant_jobs: 'listing the installation’s jobs',
  retry_job: 'retrying a job',
  cancel_job: 'cancelling a job',
  redigest_item: 'converting an item again',
  correct_digest: 'correcting a converted text',
  get_rules: 'reading the naming rules',
  set_rules: 'saving the naming rules',
  preview_names: 'previewing names',
  apply_names: 'applying names',
  get_attention: 'reading Attention',
  rebuild_index: 'rebuilding the search index',
  get_view: 'opening a visual',
  present_view: 'presenting a visual',
  resolve_view: 'refreshing a visual',
  export_view: 'exporting a visual',
  get_catalog: 'reading the visual components',
  get_receipt: 'reading a record of returned sources',
  list_events: 'reading a workspace’s events',
  list_tenant_events: 'reading the installation’s security events',
  get_session: 'reading the session',
  create_connector: 'issuing a connector',
  list_connectors: 'listing connectors',
  revoke_connector: 'revoking a connector',
  get_health: 'checking the service’s health',
  get_readiness: 'checking the service’s readiness',
};

/** The same words, looked up by a kind off the wire, which may be one this build does not know. */
const KIND_LOOKUP: ReadonlyMap<string, KindWords> = new Map(Object.entries(KINDS));

function kindOf(kind: string): KindWords | undefined {
  return KIND_LOOKUP.get(kind);
}

/** Whether an event is listed as a security event. A kind this build does not know is not. */
export function isSecurity(event: Event): boolean {
  return SECURITY_KINDS.has(event.kind);
}

/** What happened, as an event's title. */
export function kindWords(kind: string): string {
  return kindOf(kind)?.said ?? `An event of a kind this version does not know (“${kind}”)`;
}

/** `count` events of `kind`, counted in words: "2 content changes". */
export function countedKind(kind: string, count: number): string {
  const words = kindOf(kind);
  if (words === undefined) {
    return count === 1
      ? '1 event of a kind this version does not know'
      : `${count} events of a kind this version does not know`;
  }
  return `${count} ${count === 1 ? words.one : words.many}`;
}

/** The route an actor came by, in words. */
export function routeWords(route: string): string {
  return wordsFor(ROUTE_WORDS, route) ?? `by a way this version does not know (“${route}”)`;
}

/** Who caused an event: its subject, the route it came by and its client. */
export function actorWords(actor: EventActor): string {
  const client =
    actor.client_id === null || actor.client_id === undefined || actor.client_id === ''
      ? ''
      : `, with client ${actor.client_id}`;
  return `${actor.subject}, ${routeWords(actor.route)}${client}`;
}

/** Who caused an event, or why nobody is named; null for an event that names nobody. */
export function whoWords(event: Event): string | null {
  if (event.actor !== null && event.actor !== undefined) return actorWords(event.actor);
  if (event.kind === 'sign_in_refused') {
    return 'Nobody known: the attempt was refused before anyone was identified.';
  }
  return null;
}

/** What a refused request asked for: its operation, or the byte transport it used. */
export function refusedWords(event: Event): string | null {
  if (event.operation !== null && event.operation !== undefined) {
    return (
      wordsFor(OPERATION_WORDS, event.operation) ??
      `an operation this version does not know (“${event.operation}”)`
    );
  }
  if (event.transport !== null && event.transport !== undefined) {
    return (
      wordsFor(TRANSPORT_WORDS, event.transport) ??
      `a transfer this version does not know (“${event.transport}”)`
    );
  }
  return null;
}

/**
 * How many occurrences one event stands for, when more than one: refused sign-ins of one source
 * past its allowance are counted by the minute, and refused requests may be (the contract's
 * `Event::count`). Null for one occurrence, which is what an absent count means.
 */
export function countWords(event: Event): string | null {
  const count = event.count;
  if (count === null || count === undefined || count <= 1) return null;
  if (event.kind === 'sign_in_refused' || event.kind === 'permission_denied') {
    return `${count} ${kindOf(event.kind)?.many ?? 'occurrences'} in this minute, recorded as one event.`;
  }
  return `This event stands for ${count} occurrences.`;
}

/**
 * What an event touched, in words: its connector, item, job, proposal, revision, and its workspace
 * when that is not the one whose log this is (`ownWorkspace`). Null when it names none.
 */
export function touchedWords(event: Event, ownWorkspace?: string): string | null {
  const parts: string[] = [];
  if (event.workspace_id && event.workspace_id !== ownWorkspace) {
    parts.push(`workspace ${event.workspace_id.slice(0, 8)}`);
  }
  if (event.connector_id) parts.push(`connector ${event.connector_id.slice(0, 8)}`);
  if (event.item_id) parts.push(`item ${event.item_id.slice(0, 8)}`);
  if (event.job_id) parts.push(`job ${event.job_id.slice(0, 8)}`);
  if (event.proposal_id) parts.push(`proposal ${event.proposal_id.slice(0, 8)}`);
  if (event.revision) parts.push(`revision ${short(event.revision)}`);
  return parts.length === 0 ? null : parts.join(', ');
}

/**
 * The events that are not security events, counted by kind in words, in the order each kind was
 * first met: "3 other events: 2 content changes, 1 job update." Empty when there are none.
 */
export function othersWords(events: readonly Event[]): string {
  const counts = new Map<string, number>();
  for (const event of events) {
    if (!isSecurity(event)) counts.set(event.kind, (counts.get(event.kind) ?? 0) + 1);
  }
  const total = [...counts.values()].reduce((sum, count) => sum + count, 0);
  if (total === 0) return '';
  const each = [...counts].map(([kind, count]) => countedKind(kind, count)).join(', ');
  return `${total} other ${total === 1 ? 'event' : 'events'}: ${each}.`;
}
