/**
 * Configure validation before loading the application graph. A static router import lets its
 * shared chunks construct schemas before configuration, triggering Zod's CSP-refused eval probe.
 */
import './app/schema-runtime';

await import('./app/mount');
