/** Approved layout vocabulary. Source-bearing components accept binding names, not fabricated evidence. */
import { defineCatalog } from '@json-render/core';
import { schema } from '@json-render/react/schema';
import { z } from 'zod';

const binding = z.string().min(1).max(120);
export const catalog = defineCatalog(schema, {
  components: {
    Stack: {
      props: z.object({ title: z.string().max(160).optional() }),
      slots: ['default'],
      description: 'Group approved components vertically.',
    },
    Columns: {
      props: z.object({}),
      slots: ['default'],
      description: 'Place approved components side by side when space permits.',
    },
    SourceExcerpt: {
      props: z.object({ binding }),
      description: 'Read the exact source identified by an authorized binding.',
    },
    DataTable: {
      props: z.object({ binding }),
      description: 'Show resolved source data in an accessible table.',
    },
    Chart: {
      props: z.object({ binding, title: z.string().max(160) }),
      description: 'Display a server-validated chart and its data table.',
    },
    SourceList: {
      props: z.object({ bindings: z.array(binding).max(50) }),
      description: 'List the exact source references supporting this view.',
    },
  },
  actions: {},
});
