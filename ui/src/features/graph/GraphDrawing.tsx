/**
 * The graph drawn: its items as nodes and the links among them as arrows (cytoscape, with its
 * canvas renderer and its built-in `cose` layout), each node named by the item's title. Pressing a
 * node opens the item, at the revision the graph was read at.
 *
 * The drawing is for the eye and the pointer. It is one image to assistive technology, whose name
 * says that the table and the list below it hold the same items and links: those are what a
 * keyboard and a screen reader use. A browser that cannot draw on a canvas gets a sentence saying
 * so in its place, and no drawing is attempted.
 *
 * Its colours are the page's: the text colour the container is drawn in, so the drawing follows a
 * light or a dark scheme as the rest of the page does.
 */
import cytoscape from 'cytoscape';
import { useEffect, useRef, useState } from 'react';
import type { GetGraphResponse } from '../../api/generated/types.gen';
import { graphElements } from './graph';

export interface GraphDrawingProps {
  graph: GetGraphResponse;
  /** A node was pressed: open its item. */
  onOpen: (itemId: string) => void;
}

/** Whether this browser can draw on a canvas at all. */
function canDraw(): boolean {
  try {
    return document.createElement('canvas').getContext('2d') !== null;
  } catch {
    return false;
  }
}

export function GraphDrawing({ graph, onOpen }: GraphDrawingProps) {
  const container = useRef<HTMLDivElement | null>(null);
  const open = useRef(onOpen);
  open.current = onOpen;
  const [drawable] = useState(canDraw);
  const count = graph.nodes.length;
  useEffect(() => {
    const element = container.current;
    if (!drawable || element === null) return;
    const ink = getComputedStyle(element).color;
    const { nodes, edges } = graphElements(graph);
    const cy = cytoscape({
      container: element,
      elements: { nodes, edges },
      layout: { name: 'cose', animate: false },
      style: [
        {
          selector: 'node',
          style: {
            label: 'data(label)',
            color: ink,
            'font-size': 11,
            'text-valign': 'bottom',
            'text-margin-y': 4,
            'background-color': ink,
            'background-opacity': 0.25,
            'border-color': ink,
            'border-width': 1,
            width: 14,
            height: 14,
          },
        },
        {
          selector: 'edge',
          style: {
            'line-color': ink,
            'target-arrow-color': ink,
            'target-arrow-shape': 'triangle',
            'curve-style': 'bezier',
            width: 1,
            opacity: 0.6,
          },
        },
      ],
      boxSelectionEnabled: false,
      autoungrabify: true,
    });
    cy.on('tap', 'node', (event) => open.current(event.target.id()));
    return () => cy.destroy();
  }, [drawable, graph]);
  if (!drawable) {
    return (
      <p
        role="img"
        aria-label="The graph is not drawn here: the table and the list below hold its items and links."
      >
        This browser cannot draw the graph. The table and the list below hold the same items and
        links.
      </p>
    );
  }
  return (
    <div
      ref={container}
      className="graph-drawing"
      role="img"
      aria-label={`A drawing of ${count} ${count === 1 ? 'item' : 'items'} and the links between them; the table and the list below hold the same items and links.`}
    />
  );
}
