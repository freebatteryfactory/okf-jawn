/**
 * The workspace's graph: how its items link to each other, at the revision the workspace shows,
 * as the service answers it (`graph.ts`), drawn (`GraphDrawing.tsx`) and listed.
 *
 * The list is the drawing's accessible equivalent, with the same items and links: a table of the
 * items (each a link to the item at the revision the graph was read at, with how many links of
 * the graph leave it and reach it) and a list of the links (from, to, the link's own text and its
 * line). A link that leads nowhere, or to an item outside the graph, says so. When the service
 * cut the graph short, the page says so above it. A graph that could not be read is a failure
 * notice, never an empty graph.
 */
import { useQuery } from '@tanstack/react-query';
import { Link, useNavigate } from '@tanstack/react-router';
import { useId, useRef } from 'react';
import type { GetGraphResponse, ItemSummary, Revision } from '../../api/generated/types.gen';
import { keyed } from '../../lib/keys';
import { short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { GraphDrawing } from './GraphDrawing';
import { GRAPH_MAX_NODES, graphOptions, leadsTo, nameOf } from './graph';

export interface GraphPageProps {
  workspaceId: string;
  /** The revision the workspace shows. */
  head: Revision;
}

export function GraphPage({ workspaceId, head }: GraphPageProps) {
  const headingId = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const navigate = useNavigate();
  const graph = useQuery(graphOptions(workspaceId, head));
  const failure = useRetry({
    request: `${workspaceId} ${head}`,
    failed: graph.isError,
    error: graph.error,
    run: () => graph.refetch(),
    loaded: () => headingRef.current,
  });
  const answer = graph.data;
  return (
    <section className="workspace-page" aria-labelledby={headingId}>
      <h2 id={headingId} ref={headingRef} tabIndex={-1} className="page-heading">
        Graph
      </h2>
      <p className="meta">
        How the items of this workspace link to each other at revision{' '}
        <code title={head}>{short(head)}</code>, the one it shows, as the service gives it: at most{' '}
        {GRAPH_MAX_NODES} items at once.
      </p>
      {graph.isPending && !failure.retrying && <p role="status">Reading the graph…</p>}
      {failure.shown && (
        <FailureNotice
          context="Reading the graph"
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
        />
      )}
      {answer !== undefined && (
        <>
          {answer.truncated && (
            <p role="note" className="notice">
              The graph is cut short: it shows the {answer.nodes.length} items the service gave, at
              most {GRAPH_MAX_NODES} at once. Items and links beyond them are not shown.
            </p>
          )}
          {answer.nodes.length === 0 ? (
            <p>No items to show.</p>
          ) : (
            <>
              <GraphDrawing
                graph={answer}
                onOpen={(itemId) =>
                  void navigate({
                    to: '/w/$workspaceId/i/$itemId',
                    params: { workspaceId, itemId },
                    search: { rev: answer.revision },
                  })
                }
              />
              <GraphList workspaceId={workspaceId} graph={answer} />
            </>
          )}
        </>
      )}
    </section>
  );
}

/** The graph's items and links, for a keyboard and a screen reader. */
function GraphList({ workspaceId, graph }: { workspaceId: string; graph: GetGraphResponse }) {
  const itemsId = useId();
  const linksId = useId();
  const shown = new Set(graph.nodes.map((node) => node.id));
  const byId = new Map<string, ItemSummary>(graph.nodes.map((node) => [node.id, node]));
  const out = new Map<string, number>();
  const into = new Map<string, number>();
  for (const edge of graph.edges) {
    out.set(edge.from, (out.get(edge.from) ?? 0) + 1);
    const to = edge.to ?? null;
    if (to !== null) into.set(to, (into.get(to) ?? 0) + 1);
  }
  const itemLink = (item: ItemSummary) => (
    <Link
      to="/w/$workspaceId/i/$itemId"
      params={{ workspaceId, itemId: item.id }}
      search={{ rev: graph.revision }}
    >
      {nameOf(item)}
    </Link>
  );
  return (
    <>
      <table className="graph-items">
        <caption id={itemsId}>Items in the graph</caption>
        <thead>
          <tr>
            <th scope="col">Item</th>
            <th scope="col">Path</th>
            <th scope="col">Links out</th>
            <th scope="col">Links in</th>
          </tr>
        </thead>
        <tbody>
          {keyed(graph.nodes, (node) => node.id).map(({ key, value: node }) => (
            <tr key={key}>
              <td>{itemLink(node)}</td>
              <td>{node.path}</td>
              <td>{out.get(node.id) ?? 0}</td>
              <td>{into.get(node.id) ?? 0}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <h3 id={linksId}>Links in the graph</h3>
      {graph.edges.length === 0 ? (
        <p>No item of the graph links to another.</p>
      ) : (
        <ul aria-labelledby={linksId} className="graph-links">
          {keyed(graph.edges, (edge) => JSON.stringify(edge)).map(({ key, value: edge }) => {
            const from = byId.get(edge.from);
            const leads = leadsTo(edge, shown);
            const target = leads.kind === 'item' ? byId.get(leads.itemId) : undefined;
            return (
              <li key={key}>
                {from === undefined ? 'An item outside this graph' : itemLink(from)}
                {' → '}
                {target !== undefined
                  ? itemLink(target)
                  : leads.kind === 'nowhere'
                    ? `${edge.to_path} (leads nowhere: nothing is at that path)`
                    : `${edge.to_path} (an item outside this graph)`}
                : “{edge.label}”, line {edge.line}
              </li>
            );
          })}
        </ul>
      )}
    </>
  );
}
