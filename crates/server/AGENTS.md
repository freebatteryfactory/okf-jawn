# HTTP / auth lane

Read root AGENTS.md and SPEC.md.

Supply executable startup that constructs the storage and ingest adapters, injects them into `core::application::ApplicationService`, and serves it; startup wires dependencies and holds no operation behavior. Implement both authentication entry paths from SPEC section 11: local (persistent installation identity, single-use launch token exchanged at `/auth/local` for the session cookie, Host/Origin/CSRF checks, connector-secret bearer auth for local MCP) and hosted (WorkOS AuthKit browser sessions and Connect validation). Hosted mode with missing or invalid configuration fails startup; it never falls back to local. Also supply static assets, declared binary/SSE/OAuth/MCP routes, readiness and graceful shutdown. Existing JSON router expects a real Application and authenticated Principal. No guest-owner fallback and no trust granted for loopback.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
