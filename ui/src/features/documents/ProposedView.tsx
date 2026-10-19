/**
 * The Views a proposal would store, each drawn as proposed, for the person deciding on it (SPEC
 * section 13: "proposed View/approval/render"; M4b ruling B2). A change stores a View when it
 * creates one, or edits an item that is a View at the proposal's base (`viewCandidates`); the
 * kind of an edited item is read there, as the service reads it.
 *
 * Each proposed file is read with the browser's one View parser (`parseViewFile`), its bindings in
 * this workspace, and drawn with `present_view` (`PresentView` over the Explorer's reads), which
 * saves nothing. A file the parser refuses is said in words, with the parser's own account and the
 * place in the document it names; nothing is presented for it, and nothing here decides anything:
 * the service checks the View again when the proposal is accepted, so Approve stays offered. A
 * refusal of `present_view` is the workspace's failure notice, kept through its retry, every code
 * in its plain words. `present_view` is a read, allowed in an archived workspace, so the drawing
 * is the same there.
 *
 * What a presentation answers is read with the generated schema before it is drawn; an answer the
 * schema refuses is a failure of its own words, never the service being out of reach.
 */
import { useQuery } from '@tanstack/react-query';
import { useId, useRef } from 'react';
import type { z } from 'zod';
import { presentViewQueryKey } from '../../api/generated/@tanstack/react-query.gen';
import { presentView } from '../../api/generated/sdk.gen';
import type {
  Proposal,
  SourceReference,
  ViewBinding,
  ViewDocument,
} from '../../api/generated/types.gen';
import { zPresentResponse, type zViewDocument } from '../../api/generated/zod.gen';
import { FailureNotice } from '../errors/FailureNotice';
import type { FailureMessage } from '../errors/failure';
import { useRetry } from '../errors/retry';
import { type ViewCandidate, viewCandidates } from '../proposals/proposals';
import { PresentView } from '../views/PresentView';
import { parseViewFile, type ViewFile, type ViewFileProblem } from '../views/view-file';
import { itemOptions } from './DocumentView';
import { workspaceViewData } from './view-data';

/** What each refusal of the parser means, said of the proposed file. */
const PROBLEM_WORDS: Record<ViewFileProblem, string> = {
  no_block: 'its text has no View block',
  two_blocks: 'its text has more than one View block',
  not_closed: 'its View block is not closed',
  not_last: 'its View block is not at the end of its text',
  not_json: 'its View block is not JSON',
  repeated_key: 'its View block names one key twice',
  version: 'it is written in a View format this build does not read',
  schema: 'a part of it is not what a View holds',
};

/** An answer of `present_view` the generated schema does not read. */
class UnreadablePresentation extends Error {}

const unreadableWords: FailureMessage = {
  title: 'The server returned a presentation this client cannot read.',
  advice: 'Try again. If it happens again, this page and the server may be of different versions.',
  action: { kind: 'retry', label: 'Try again' },
};

export interface ProposedViewsProps {
  workspaceId: string;
  proposal: Proposal;
}

/** Each View the proposal would store, drawn as proposed; nothing for a proposal that stores none. */
export function ProposedViews({ workspaceId, proposal }: ProposedViewsProps) {
  return viewCandidates(proposal).map((candidate) =>
    candidate.kind === 'created' ? (
      <ProposedView
        key={candidate.key}
        workspaceId={workspaceId}
        path={candidate.path}
        file={candidate.file}
      />
    ) : (
      <EditedItem
        key={candidate.key}
        workspaceId={workspaceId}
        base={proposal.base_revision}
        candidate={candidate}
      />
    ),
  );
}

/** An edited item, drawn as a proposed View when it is a View at the proposal's base. */
function EditedItem({
  workspaceId,
  base,
  candidate,
}: {
  workspaceId: string;
  base: string;
  candidate: Extract<ViewCandidate, { kind: 'edited' }>;
}) {
  const item = useQuery({
    ...itemOptions(workspaceId, candidate.itemId, base),
    // What a resolved revision holds never changes.
    staleTime: Number.POSITIVE_INFINITY,
  });
  const failure = useRetry({
    request: `${candidate.itemId} ${base}`,
    failed: item.isError,
    error: item.error,
    run: () => item.refetch(),
    loaded: () => null,
  });
  if (failure.shown)
    return (
      <FailureNotice
        context="Reading whether a changed item is a View"
        error={failure.error}
        retry={failure}
        workspaceId={workspaceId}
      />
    );
  const summary = item.data?.summary;
  if (summary?.kind !== 'view') return null;
  return <ProposedView workspaceId={workspaceId} path={summary.path} file={candidate.file} />;
}

/** One proposed View file: drawn as proposed, or why it cannot be read as a View. */
function ProposedView({
  workspaceId,
  path,
  file,
}: {
  workspaceId: string;
  path: string;
  file: ViewFile;
}) {
  const headingId = useId();
  const read = parseViewFile(file, workspaceId);
  return (
    <section aria-labelledby={headingId} className="proposed-view">
      <h3 id={headingId}>Proposed View: {path}</h3>
      {read.ok ? (
        <Presented workspaceId={workspaceId} view={wireView(read.document)} />
      ) : (
        <div role="note" className="notice">
          <p>
            <strong>
              This proposed file cannot be shown as a View: {PROBLEM_WORDS[read.problem]}.
            </strong>
          </p>
          <p>
            The check here said: <q>{read.message}</q>
            {read.pointer === null ? null : (
              <>
                {' '}
                (at <code>{read.pointer}</code>)
              </>
            )}
            .
          </p>
          <p>
            The service checks the View again when the proposal is accepted, and refuses it there if
            it is not one: this page’s check decides nothing.
          </p>
        </div>
      )}
    </section>
  );
}

/** A View document drawn with `present_view`, which saves nothing. */
function Presented({ workspaceId, view }: { workspaceId: string; view: ViewDocument }) {
  const body = { workspace_id: workspaceId, view };
  const presented = useQuery({
    queryKey: presentViewQueryKey({ body }),
    queryFn: async ({ signal }): Promise<z.infer<typeof zPresentResponse>> => {
      const { data } = await presentView({ body, signal, throwOnError: true });
      const parsed = zPresentResponse.safeParse(data);
      if (!parsed.success) throw new UnreadablePresentation(parsed.error.message);
      return parsed.data;
    },
    // A candidate is drawn once from the sources it pins: asking again would only redraw it.
    staleTime: Number.POSITIVE_INFINITY,
  });
  const placeRef = useRef<HTMLDivElement | null>(null);
  const failure = useRetry({
    request: JSON.stringify(body),
    failed: presented.isError,
    error: presented.error,
    run: () => presented.refetch(),
    loaded: () => placeRef.current,
  });
  return (
    <>
      <p className="meta">
        How this View would look, drawn from the sources it cites. Showing it saves nothing.
      </p>
      {presented.isPending && !failure.retrying && <p role="status">Drawing the proposed View…</p>}
      {failure.shown && (
        <FailureNotice
          context="Showing the proposed View"
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
          words={failure.error instanceof UnreadablePresentation ? unreadableWords : undefined}
        />
      )}
      {presented.data !== undefined && !failure.shown && (
        // Focusable from script only: where keyboard focus goes when a retry drew the View.
        <div ref={placeRef} tabIndex={-1}>
          <PresentView response={presented.data} data={workspaceViewData} />
        </div>
      )}
    </>
  );
}

type ParsedView = z.infer<typeof zViewDocument>;

/**
 * A parsed View as the wire type spells it. A parsed value's type lets an absent optional read
 * `undefined`, which the wire type does not; a parsed value never holds one, so nothing the file
 * holds is left out here.
 */
function wireView({ bindings, charts, ...required }: ParsedView): ViewDocument {
  return {
    ...required,
    bindings: bindings.map(wireBinding),
    ...(charts === undefined ? {} : { charts }),
  };
}

function wireBinding({
  materialized,
  source,
  ...required
}: ParsedView['bindings'][number]): ViewBinding {
  return {
    ...required,
    source: wireSource(source),
    ...(materialized === undefined ? {} : { materialized }),
  };
}

function wireSource({
  digest,
  locations,
  ...required
}: ParsedView['bindings'][number]['source']): SourceReference {
  return {
    ...required,
    ...(digest === undefined ? {} : { digest }),
    ...(locations === undefined ? {} : { locations }),
  };
}
