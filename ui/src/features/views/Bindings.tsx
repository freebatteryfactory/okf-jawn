/** Provide already-authorized read results to presentation components. */

import { createContext, type ReactNode, useContext } from 'react';
import type { TopLevelSpec } from 'vega-lite';
import type { z } from 'zod';
import type {
  zReadItemResponse,
  zSourceReference,
  zViewBinding,
} from '../../api/generated/zod.gen';
import type { DerivedDataset } from './transforms';

/**
 * How the host opens a source the View cites, at the revision the View read it: the label given
 * (the source's path and revision, as the View says them), made into the host's own link. A host
 * with nowhere to open a source (an MCP App) gives none, and the label stays text.
 */
export type SourceLink = (source: z.infer<typeof zSourceReference>, label: ReactNode) => ReactNode;

/** `label`, as the host's link to `source` when it gives one. */
export function linked(
  sourceLink: SourceLink | undefined,
  source: z.infer<typeof zSourceReference>,
  label: ReactNode,
): ReactNode {
  return sourceLink === undefined ? label : sourceLink(source, label);
}

export interface ResolvedPresentation {
  charts: ReadonlyMap<string, TopLevelSpec>;
  sources: ReadonlyMap<string, z.infer<typeof zReadItemResponse>>;
  bindings: ReadonlyMap<string, z.infer<typeof zViewBinding>>;
  tables: ReadonlyMap<
    string,
    ReadonlyArray<Readonly<Record<string, string | number | boolean | null>>>
  >;
  /** Why a binding's dataset was refused (malformed, not the converter's, stale): that chart's alert. */
  datasetErrors?: ReadonlyMap<string, string>;
  /**
   * Why the server could not produce a binding's dataset: the message of its `dataset_unavailable`
   * warning, exactly as sent, for the table that reads it (a table has no chart status to say so).
   */
  datasetWarnings?: ReadonlyMap<string, string>;
  /** Why a binding's source could not be read: that source excerpt's alert, never the View's. */
  sourceErrors?: ReadonlyMap<string, string>;
  /**
   * Why a named chart cannot be drawn whatever its data: a specification compile rejects, or the
   * server's own failed status for it. The key is the chart's name; the empty key is the View's one
   * `vega_lite` chart. That chart's alert alone.
   */
  chartErrors?: ReadonlyMap<string, string>;
  /**
   * The bindings each chart reads, as the server's `ready` status names them; the key as in
   * `chartErrors`. A chart is given every one of them.
   */
  chartBindings?: ReadonlyMap<string, readonly string[]>;
  /**
   * What each chart's lookups read that its bindings' transforms make, in the order to make it
   * (`planChart`); the key as in `chartErrors`. None when a chart looks up nothing transformed.
   */
  derivedDatasets?: ReadonlyMap<string, readonly DerivedDataset[]>;
  /** The host's link to a cited source; none, and sources are named as text. */
  sourceLink?: SourceLink | undefined;
}
export const BindingsContext = createContext<ResolvedPresentation | null>(null);
export function useBindings(): ResolvedPresentation {
  const value = useContext(BindingsContext);
  if (value === null) throw new Error('Presentation requires authorized source bindings');
  return value;
}
