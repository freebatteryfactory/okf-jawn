/**
 * The workspace's graph in the cache, and what a drawing of it holds.
 *
 * The graph is the bounded one the service answers at the revision the workspace shows
 * (`get_graph`: its items, the links among them, and whether it was cut short); the page never
 * walks the workspace to make one of its own. At most `GRAPH_MAX_NODES` items are asked for. A
 * drawing holds every item answered and only the links between two items it shows: a link that
 * leads nowhere, or to an item outside the graph, has nothing to point at and is said in the list
 * instead.
 */
import { getGraphOptions } from '../../api/generated/@tanstack/react-query.gen';
import type { GetGraphResponse, Link, Revision } from '../../api/generated/types.gen';

/** The most items asked for in one graph. */
export const GRAPH_MAX_NODES = 200;

export function graphOptions(workspaceId: string, revision: Revision) {
  return {
    ...getGraphOptions({
      body: {
        workspace_id: workspaceId,
        at: { kind: 'revision', revision },
        max_nodes: GRAPH_MAX_NODES,
      },
    }),
    // A resolved revision's graph does not change.
    staleTime: Number.POSITIVE_INFINITY,
  };
}

/** An item's name where it is drawn or listed: its title, else its path. */
export function nameOf(item: { title: string; path: string }): string {
  return item.title || item.path;
}

export interface DrawnNode {
  data: { id: string; label: string };
}
export interface DrawnEdge {
  data: { id: string; source: string; target: string };
}

/** What a drawing of the graph holds. */
export function graphElements(graph: GetGraphResponse): { nodes: DrawnNode[]; edges: DrawnEdge[] } {
  const shown = new Set(graph.nodes.map((node) => node.id));
  const nodes = graph.nodes.map((node) => ({ data: { id: node.id, label: nameOf(node) } }));
  const edges: DrawnEdge[] = [];
  graph.edges.forEach((edge, index) => {
    const target = edge.to ?? null;
    if (target === null || !shown.has(edge.from) || !shown.has(target)) return;
    edges.push({ data: { id: `link-${index}`, source: edge.from, target } });
  });
  return { nodes, edges };
}

/** Where a link leads, among the items of the graph. */
export type Leads = { kind: 'item'; itemId: string } | { kind: 'nowhere' } | { kind: 'outside' };

export function leadsTo(link: Link, shown: ReadonlySet<string>): Leads {
  const target = link.to ?? null;
  if (target === null) return { kind: 'nowhere' };
  return shown.has(target) ? { kind: 'item', itemId: target } : { kind: 'outside' };
}
