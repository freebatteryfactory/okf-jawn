# HTTP / auth lane

Read root AGENTS.md and SPEC.md.

This lane's directories and gate command are in the root AGENTS.md table, and its construction gates are the `verification.json` entries whose `owner` is `server` (each names its command).

Supply executable startup that constructs the storage and ingest adapters, injects them into `core::application::ApplicationService`, and serves it; startup wires dependencies and holds no operation behavior. Implement both authentication entry paths from SPEC section 11: local (persistent installation identity, single-use launch token exchanged at `/auth/local` for the session cookie, Host/Origin/CSRF checks, connector-secret bearer auth for local MCP) and hosted (WorkOS AuthKit browser sessions and Connect validation). Hosted mode with missing or invalid configuration fails startup; it never falls back to local. Also supply static assets, declared binary/SSE/OAuth/MCP routes, sandbox-origin HTML serving (`serve_sandbox_representation` transport; WebMCP and the sandbox viewer UI live in workspace-ui), readiness and graceful shutdown. Existing JSON router expects a real Application and authenticated Principal. No guest-owner fallback and no trust granted for loopback.

The identity middleware is the only place a `Principal` or `SessionId` extension is inserted, and it is layered outside the router. Only a request authenticated by the browser session cookie may carry `AccessRoute::BrowserSession` or `AccessRoute::LocalOwner`: core lets those two routes reach drafts and decides by route alone, so giving either to the CLI, a connector secret or any other non-browser client exposes drafts to it (SPEC §8). An unknown path and a wrong method must return `ApiError` JSON (today they are axum defaults with an empty body). Body-rejection messages must not echo unbounded client input.

TODO: select and vendor session/cookie crates only after a recorded vendor note exists; do not invent cookie-jar dependencies here.

Generator-input rule: change only this lane's authored inputs; run `gen` and commit outputs; gen-check must pass; integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
