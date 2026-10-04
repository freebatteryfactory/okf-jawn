/** The host owns the model loop; this resource displays tool results only. */

import { useApp } from '@modelcontextprotocol/ext-apps/react';
import { StrictMode, useCallback, useEffect, useState } from 'react';
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
import '../styles.css';

const resultSchema = z.union([
  zReadItemResponse,
  zDiffResponse,
  zLogResponse,
  zPresentResponse,
  zGetSourcesResponse,
]);
function App() {
  const { app, error } = useApp({
    appInfo: { name: 'okf-jawn', version: '0.1.0' },
    capabilities: {},
  });
  const [result, setResult] = useState<unknown>(null);
  useEffect(() => {
    if (!app) return;
    app.ontoolresult = (message) => {
      setResult(message.structuredContent);
    };
  }, [app]);
  const callTool = useCallback(
    async (name: string, input: Record<string, unknown>) => {
      if (!app) throw new Error('Host connection unavailable');
      return app.callServerTool({ name, arguments: input });
    },
    [app],
  );
  if (error) return <p role="alert">Host connection failed: {String(error)}</p>;
  if (result === null)
    return <p role="status">Waiting for a tool result from the connected host.</p>;
  const parsed = resultSchema.safeParse(result);
  if (!parsed.success)
    return (
      <p role="alert">
        The tool result does not match a supported source, changes, or timeline schema. It has not
        been rendered as trusted source content.
      </p>
    );
  if ('resolved_bindings' in parsed.data)
    return <PresentView response={parsed.data} callTool={callTool} />;
  if ('sources' in parsed.data) return <Sources result={parsed.data} />;
  if ('markdown' in parsed.data) return <SourceExcerpt result={parsed.data} />;
  if ('commits' in parsed.data) return <Timeline result={parsed.data} />;
  return <Changes result={parsed.data} />;
}
const element = document.getElementById('root');
if (!element) throw new Error('Missing MCP App root');
createRoot(element).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
