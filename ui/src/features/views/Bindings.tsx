/** Provide already-authorized read results to presentation components. */

import { createContext, useContext } from 'react';
import type { TopLevelSpec } from 'vega-lite';
import type { ReadItemResponse, ViewBinding } from '../../api/generated/types.gen';

export interface ResolvedPresentation {
  charts: ReadonlyMap<string, TopLevelSpec>;
  sources: ReadonlyMap<string, ReadItemResponse>;
  bindings: ReadonlyMap<string, ViewBinding>;
  tables: ReadonlyMap<
    string,
    ReadonlyArray<Readonly<Record<string, string | number | boolean | null>>>
  >;
}
export const BindingsContext = createContext<ResolvedPresentation | null>(null);
export function useBindings(): ResolvedPresentation {
  const value = useContext(BindingsContext);
  if (value === null) throw new Error('Presentation requires authorized source bindings');
  return value;
}
