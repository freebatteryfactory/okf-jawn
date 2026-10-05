/** The host owns the model loop; this resource displays tool results only. */

import { useApp } from '@modelcontextprotocol/ext-apps/react';
import { StrictMode, useCallback, useState } from 'react';
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
import { PresentView } from '../features/views/PresentView';
import { omitUndefined } from '../lib/wire';
import '../styles.css';

const resultSchema = z.union([
  zReadItemResponse,
  zDiffResponse,
  zLogResponse,
  zPresentResponse,
  zGetSourcesResponse,
]);

/** Pick Source / Changes / Timeline / Present from structuredContent; unknown → text fallback. */
export function AppResult({
  result,
  callTool,
}: {
  result: unknown;
  callTool: (name: string, input: Record<string, unknown>) => Promise<unknown>;
}) {
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
  if ('resolved_bindings' in parsed.data)
    return <PresentView response={parsed.data} callTool={callTool} />;
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
  const callTool = useCallback(
    async (name: string, input: Record<string, unknown>) => {
      if (!app) throw new Error('Host connection unavailable');
      // Single wire-boundary omit: PresentView must pass fields through unchanged.
      return app.callServerTool({ name, arguments: omitUndefined(input) });
    },
    [app],
  );
  if (error) return <p role="alert">Host connection failed: {String(error)}</p>;
  if (result === null)
    return <p role="status">Waiting for a tool result from the connected host.</p>;
  return <AppResult result={result} callTool={callTool} />;
}

const element = document.getElementById('root');
if (element) {
  createRoot(element).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
