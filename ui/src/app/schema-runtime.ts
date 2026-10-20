/**
 * Validate without generated code under the server's script-src 'self' page policy (M4a A5).
 * Zod 4.6's jitless mode also skips its caught Function probe, which browsers report as a CSP
 * violation. Import this before the router and its schemas; all validation remains enabled.
 */
import { z } from 'zod';

z.config({ jitless: true });
