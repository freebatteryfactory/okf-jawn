/** The host owns the model loop; this resource displays tool results only. */

import { useApp } from '@modelcontextprotocol/ext-apps/react';
import { StrictMode, useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { z } from 'zod';
import {
  zDiffResponse,
  zGetSourcesResponse,
  zLogResponse,
  zPresentResponse,
  zReadItemResponse,
} from '../api/generated/zod.gen';
import { SourceExcerpt } from '../features/documents/SourceExcerpt';
import { Sources } from '../features/documents/Sources';
import { Changes } from '../features/history/Changes';
import { Timeline } from '../features/history/Timeline';
import type { ViewData } from '../features/views/data';
import { PresentView } from '../features/views/PresentView';
import '../styles.css';
import { serverToolData } from './view-data';

const resultSchema = z.union([
  zReadItemResponse,
  zDiffResponse,
  zLogResponse,
  zPresentResponse,
  zGetSourcesResponse,
]);

/** Pick Source / Changes / Timeline / Present from structuredContent; unknown → text fallback. */
export function AppResult({ result, data }: { result: unknown; data: ViewData }) {
  const parsed = resultSchema.safeParse(result);
  if (!parsed.success) {
    const text =
      typeof result === 'string'
        ? result
        : result === null || result === undefined
          ? ''
          : JSON.stringify(result);
    return (
      <p role="status">
        {text.length > 0
          ? text
          : 'The tool result does not match a supported Source, Changes, Timeline, or Present schema.'}
      </p>
    );
  }
  if ('resolved_bindings' in parsed.data) return <PresentView response={parsed.data} data={data} />;
  if ('sources' in parsed.data) return <Sources result={parsed.data} />;
  if ('markdown' in parsed.data) return <SourceExcerpt result={parsed.data} />;
  if ('commits' in parsed.data) return <Timeline result={parsed.data} />;
  return <Changes result={parsed.data} />;
}

function App() {
  const [result, setResult] = useState<unknown>(null);
  const { app, error } = useApp({
    appInfo: { name: 'okf-jawn', version: '0.1.0' },
    capabilities: {},
    onAppCreated: (created) => {
      // Handlers must be registered before connect(); events can arrive immediately after.
      created.ontoolresult = (message) => {
        setResult(message.structuredContent);
      };
    },
  });
  // A View's reads go to the host's server tools; view-data.ts is the one wire boundary.
  const data = useMemo(
    () =>
      serverToolData(async (call) => {
        if (!app) throw new Error('Host connection unavailable');
        return app.callServerTool(call);
      }),
    [app],
  );
  if (error) return <p role="alert">Host connection failed: {String(error)}</p>;
  if (result === null)
    return <p role="status">Waiting for a tool result from the connected host.</p>;
  return <AppResult result={result} data={data} />;
}

const element = document.getElementById('root');
if (element) {
  createRoot(element).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
