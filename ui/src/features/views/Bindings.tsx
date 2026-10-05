/** Provide already-authorized read results to presentation components. */

import { createContext, useContext } from 'react';
import type { TopLevelSpec } from 'vega-lite';
import type { z } from 'zod';
import type { zReadItemResponse, zViewBinding } from '../../api/generated/zod.gen';

export interface ResolvedPresentation {
  charts: ReadonlyMap<string, TopLevelSpec>;
  sources: ReadonlyMap<string, z.infer<typeof zReadItemResponse>>;
  bindings: ReadonlyMap<string, z.infer<typeof zViewBinding>>;
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
