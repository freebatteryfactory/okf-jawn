/** Mount the human workspace; the user's chosen agent owns chat. */

import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { createQueryClient } from './query-client';
import { App, createAppRouter } from './router';
import '../styles.css';

const root = document.getElementById('root');
if (root === null) throw new Error('Missing application root');
createRoot(root).render(
  <StrictMode>
    <App router={createAppRouter()} queryClient={createQueryClient()} />
  </StrictMode>,
);
