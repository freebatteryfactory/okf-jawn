/** Mount the human workspace; the user's chosen agent owns chat. */

import './app/schema-runtime';
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { createQueryClient } from './app/query-client';
import { App, createAppRouter } from './app/router';
import './styles.css';

const root = document.getElementById('root');
if (root === null) throw new Error('Missing application root');
createRoot(root).render(
  <StrictMode>
    <App router={createAppRouter()} queryClient={createQueryClient()} />
  </StrictMode>,
);
