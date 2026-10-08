# okf-jawn: complete product specification

Owner: Eassa Ayoub. Baseline: the supplied October 3, 2026 Build Brief and explicit later decisions. This file preserves product meaning; generated API schemas preserve wire structure. Implementation and qualification state are in README.md and verification.json, not implied by this specification.

## 1. Product and audience

An open-source application for turning a pile of documents into a human-readable, maintainable context workspace that people and their chosen agents can both use. The experience combines a familiar folder tree, document/artifact viewer, source panel, and plain-language version history.

The product ships blank. Users create and open any number of workspaces and choose their own folder organization, type names, and properties. Example workshop files live only in samples/workshop and are imported normally. Context engineering is taught through organizing, inspecting, locating, preserving and updating meaning, not merely prettifying names.

The host's chat is the chat. Built-in chat, agent loops, Rig, AG-UI, CopilotKit, model-provider clients and provider keys are canceled, not deferred. MCP is the primary agent interface. The application remains useful without a connected model.

Keep the full human experience: folder navigation, search, tabs, resizable panes, rich document views, editing and properties, naming-rule form and advanced YAML, preview/apply, graph/backlinks, version history, comparisons, attribution, proposals, review, Attention, persistent visual artifacts, and export.

## 2. Architecture and deployment

The implementation is a Rust application service with TypeScript/React interfaces built with Vite. One set of typed operations serves the browser, CLI and MCP adapter. The public OpenAPI document is generated from Rust declarations; generated clients and schemas consume it. UI components are composed with existing libraries, not generated automatically from endpoint shapes.

Bun is the JavaScript package manager and tooling runtime: one root workspace and bun.lock, package scripts, frontend generators, bundling and the tooling tests. The application server is Rust; Bun is not a server runtime and Node/pnpm are not project prerequisites. TypeScript 7 type checking remains a separate check, because executing or transpiling TypeScript is not type checking. The `tsc` checker is TypeScript 7; tools that call the compiler API (Hey API) use the side-by-side TypeScript 6 API package until they support TypeScript 7. Authored code is checked with `strict`, `noUncheckedIndexedAccess` and `exactOptionalPropertyTypes`; a generated-code exception exists only where a pinned generator's output cannot meet a rule, is declared in its own project, and carries a probe that signals when it can be removed.

The production Application lives in core's application module and implements the operation interface over injected core-owned ports. Core never depends on a concrete storage, ingestion or transport crate. Server startup constructs the real adapters and injects them; startup wires dependencies and does not hold application behavior.

Git versions notes and source cards. A local content-addressed store preserves source bytes and retained derived artifacts. SQLite preserves jobs, reviews and tool-result records; separate derived indexes are rebuildable. OpenTelemetry is diagnostics only and may link to app records. It is never the only evidence for a review or completed import.

Run the same service locally in a browser and hosted in a container; the two authentication entry paths are defined in section 11. The documented hosted path is WorkOS AuthKit/Connect plus GalaxyGate. No Cloudflare path is included. One hosted tenant may have many workspaces.

BrowserPod, Forgejo/Gitea, S3 deployment, a hosted OTel stack and Windmill are outside this iteration. A desktop launcher is in scope as a trial: Electrobun first, Tauri 2 if Electrobun fails the trial. It starts the server on a loopback port, shows the workspace in the system web engine and adds no product behavior; local authentication (§11) applies unchanged. Because that engine is WebKit on macOS and Linux, the workspace browser tests run in both Chromium and WebKit. Signed installers and automatic updates are decided after the trial. Bun remains the JavaScript toolchain. Object storage interfaces must not dictate user folder organization.

The service, CLI, browser tests and examples default to 127.0.0.1:7711 locally. Remote endpoints require HTTPS. The CLI and MCP adapters must not become independent uncoordinated repository writers. One process owns a data directory's writable Git, SQLite and object stores at a time, through an exclusive lock on a file in that directory held for the process's life; a launcher or stdio relay connects to the running service instead of starting a second writer. A data directory on a network filesystem is unsupported unless qualified. Stored data carries a format version: startup refuses data newer than it understands without changing it, and takes a recoverable backup before an upgrade migration, then verifies the result.

## 3. Workspace, items and vocabulary

An OKF bundle is the portable folder. Arbitrary hierarchy is permitted. Maintain folder index.md documents and a change log. The reserved format files follow the selected OKF specification's own rules; ordinary concepts preserve unknown frontmatter extensions and require a nonempty type.

Built-in rendering roles are Note, Source and View. User type names are not a closed enum. Properties have schema-driven form controls with an advanced raw representation. Ordinary editing and navigation must not require Git vocabulary.

Canonical operation identifiers use verb_noun, with standard/library terminology when meanings match. UI terms are projections: Timeline, Snapshot, Changes, Rewind, Propose, Approve, Decline, Who, Comment, Verify/Verified, Archive, Expires, Import, Export, Attention. Attribution says who changed a passage, not who originated every fact. Review, merge and business approval are different concepts.

Revision is a resolved Git commit. At::Latest is a request selector resolved once before the operation reads. Source references retain workspace, stable item identity, resolved relative path, revision, digest where relevant, and selection. A rename must not make historical references point to today's content.

Instants are UTC in storage and on the wire (RFC 3339); the browser shows them in the reader's locale and time zone. A document date whose interpretation is ambiguous stays flagged, not converted.

## 4. Sources and content addressing

Bytes are identified by SHA-256. A source occurrence records the name, supplied folder, origin and relationships of one appearance. A digest records a conversion of particular bytes with a particular converter and settings. These are separate identities even if the viewer displays one document.

Deduplicate storage, not meaning. Identical bytes in different workspaces or separately meaningful occurrences retain separate source entries. An explicit retry must not create a duplicate occurrence. Alias consolidation is deliberate, not automatic solely because the hash matches. This resolves the brief's conflicting one-card-per-hash wording.

Preserve supplied filenames and paths, detected type, byte length, embedded metadata and container parent relationships. A new hash is not automatically a new version or a reason to deprecate old content. Successor relationships must be established explicitly.

The local store is shared only within the appropriate local/tenant boundary, outside user-controlled folder paths. A source card refers to an object without exposing an unauthorized filesystem path. Hash possession does not authorize a read. Export resolves internal references into portable relative files. Backups and garbage collection include retained history and app records as appropriate.

## 5. Import, conversion and corrections

Multi-file and folder upload preserves relative paths. Upload slots are authenticated, size-bounded and finalized against expected length/hash. The original is retained even when no extractor exists. Unsupported content is visible as unsupported, not silently empty or successfully converted.

Import is durable work: status, attempts, cancellation, retry and reconciliation survive interruption. Jobs run on an in-process Tokio worker over durable `RecordStore`, which is the application's source of job truth. Acceptance is persisted with `create_job` before the job is enqueued or reported to the caller; `create_job` is unique on the request's `MutationId`, so a retried request returns the job it already created. `JobQueue` only delivers wake-ups and may deliver twice: a worker runs a job only after `claim_job` grants it a lease and returns a `ClaimedJob`, and progress, completion and failure are written through that lease, so a superseded attempt cannot complete the job. Handlers (`JobHandler::handle`) are idempotent on the job's `MutationId`, and a handler stops when the state returned by `update_progress` shows the job was cancelled. A delivered wake-up is not proof of durable completion. At startup the worker reconciles from the store: `expire_leases` returns the jobs of dead attempts to the runnable set, and every job in `pending_jobs` is enqueued again. An attempt that dies while the service keeps running does not leave its job waiting for the next restart: its lease expires and the job is retried. Blob writes, SQLite transactions and Git commits need explicit recovery because they are not one transaction. Product crash durability is claimed only after the crash/restart check in section 12 passes against the real RecordStore and import implementation.

Use the pinned Docling Rust implementation through its actual supported API; do not replace it merely because a newer release exists. Establish its real native library and model asset requirements, with verified download locations, versions, hashes and configured local paths; never invent a hash or call a missing asset qualified. Retain structured Markdown, images/captions, relevant page renders, source locators and converter/version/settings. Heavy conversion runs in bounded workers with time, memory and cancellation controls. Model assets and native libraries are checked deployment inputs, not magic downloads hidden in reads.

Every source location records its provenance: direct from the document representation, inferred by matching text or layout, or unresolved. The interface never presents an inferred location with the certainty of a direct one.

Uploaded bytes are preserved while conversion is pending or after it fails, and the item shows that state accurately. "Bytes retained, extraction unsupported" is a fallback state, not completion of supported-format ingestion. No fake success, empty digest or text-only substitute may stand in for the converter.

Generated extraction and human corrections remain distinguishable. Re-extraction must not erase corrections. AI enrichment occurs only when the user's external agent proposes it, with truthful origin labels. There is no model in the ingestion service.

Archives and messages with attachments preserve parent/child relationships. Guard path traversal, symlinks, excessive expansion, nesting and resource exhaustion before extracting. Unknown formats remain stored with an explicit representation warning.

Spreadsheets expose sheet names, columns, counts, samples and bounded row/column selection. Do not flatten a large sheet into a single Markdown tool result. Formula/value/format distinctions and extraction limitations must be visible where they matter.

## 6. Naming and normalization

Rules live in versioned .okf/rules.yaml and are readable by agents. The same typed rules power a schema-driven form, an advanced YAML editor, deterministic preview and application.

Support casing, separators, prefixes/suffixes, extension handling, declared date interpretation and output formats, type-dependent destinations, collision handling, and duplicate observations. Preserve originals and aliases. Ambiguous dates are flagged instead of guessed. Applying an accepted preview uses its base revision and becomes one recoverable versioned change, including link updates.

A preview never writes. A stale preview must not silently apply to changed source state. Do not remove the human rule controls in favor of YAML-only configuration.

## 7. Reading and search

The read operation offers outline, text, multimodal, pages and original. It accepts sections, inclusive pages/lines, sheet ranges, response budgets and continuation. Return exact resolved citations, warnings and an explicit truncation indicator; pagination stays tied to the same revision and representation.

Ordinary reads are not required to inline every image. Multimodal-capable clients may request selected actual image blocks. Original bytes and large media have authenticated streaming/chunk paths. Text-only clients retain usable descriptions and source references.

Search and folder lists return descriptions/snippets and identities rather than whole document dumps. Links/backlinks and a bounded graph derive from the same source state. Search results and graph nodes are scoped to caller access.

Search, link and graph data are a rebuildable derived index behind core's SearchIndex port. Storage implements it with SQLite FTS, including index updates and rebuild mechanics. The Application owns authorization, workspace scoping, revision resolution and coordinating index updates and rebuilds with content changes. Reviews, jobs and retrieval receipts are not rebuildable; rebuilding search never removes durable application records.

Design envelope: 10,000 items per workspace, with indexed search at p95 under 500 ms on a warm local index. The reference machine, corpus and query set are fixed when first measured; only then do the numbers become release criteria. Speed never justifies dropping source detail or shortening an answer.

The human viewer includes Markdown, PDF, images, sandboxed HTML/artifacts and snapshots, Office extraction with original access, spreadsheet tables/ranges, plain text/code and editing. Original and digest may be compared side by side. Browser caching keys include workspace, source, revision, representation and selection.

## 8. History, proposals and review

Drafts may autosave; named snapshots are explicit. One draft exists per (item, editor). Saving a draft never creates a revision. `get_item` may return the caller's own draft beside committed content; `read_item`, search, export, MCP tools and agent routes never see drafts. Snapshot (`commit_items`) commits the caller's selected drafts in one commit. It is blocked only when an item being snapshotted was itself changed or deleted since that draft's base; unrelated changes elsewhere in the workspace never block it. The typed conflict lists every affected item with its draft base, the current revision and a diff, and the editor rebases each by saving against the new head. Create, move, delete, import, Rewind and accepting a proposal remain immediate commits. History, Changes and Who project versioned content. Rewind restores selected historical state in a new snapshot and does not erase history. Keep conflict resolution understandable; raw Git internals are not the operator interface.

Archive is reversible and keeps history. Purge is explicit and destructive: it removes the original bytes, derivatives, index entries, historical content and installation-managed backups of an item or a workspace, and marks the revisions, citations and Views that referred to them as invalidated. Purge never claims to erase exports, clones or backups outside the application's control. Workspace purge is built first; item purge follows with the same guarantees.

A connected answerer is read-only. An explicitly allowed drafter may create proposals. Neither receives merge or review authority. Proposals carry base and proposed revisions and allowed content changes; they cannot set the review identity or turn an unapproved proposal into a fact.

An authorized person can accept or decline the exact proposal shown. Stale preconditions require reconciliation. A review covers the exact displayed content and revision. Later changes display previous coverage accurately instead of extending the badge. Imported human-looking metadata is not authenticated review evidence.

The authenticated principal (section 11) answers who the request acts for; it is not evidence of human review. Human confirmation requires an explicit revision-bound browser action and application checks. Consuming a confirmation records the MutationId that consumed it; a retry of the same mutation after a crash may re-consume successfully, but a different mutation cannot. Never infer review from a token's email claim, a delegated user's identity, or the string human: in a file. Browser automation/delegation is not made impossible by hiding an endpoint; do not promise otherwise.

Every non-read mutation carries a durable MutationId under an idempotency key scoped to tenant, subject, client and operation. Completed mutations, and mutations released after a failed attempt, are retained for a bounded period (seven days). A mutation whose lease expired without completion or release stays until it is reconciled. A failed write releases its lease, and a retry with the same key and the same request runs as a resumed attempt under the same MutationId; the same key with a different request is a conflict. Each attempt holds a lease token, so a superseded attempt can neither complete nor release the mutation. Reconciliation is not Git-only: every store that creates a durable row (jobs, connectors, comments, uploads, confirmations, drafts) takes the MutationId and enforces it as unique, so a resumed attempt, which re-runs the handler, never duplicates an effect. A resumed `create_connector` rotates the secret of the connector it already created and returns it; replaying a completed one returns AlreadyIssued without the secret. Typed wire errors expose AlreadyIssued, InProgress and draft/idempotency Conflict details the UI can act on.

## 9. MCP, MCP Apps and WebMCP

rmcp owns the outward protocol. Model tools cover workspace discovery, listing, search, reading, links, sources, log, diff, attribution, optional propose, catalog and present. The `workspaces` tool lists the workspaces the connection may see, each with its head revision; an agent pins later reads to that revision. App-only byte retrieval is separately marked. Permissions apply independently of visibility hints, tool names and HTTP methods.

Generate MCP schemas and annotations from the declared operation semantics. Return supported text/image/resource-link and structured content through SDK constructors. No generated annotation is an access-control mechanism. Keep tools usable with a textual fallback when a host cannot display an App.

MCP Apps supplies source, Changes, Timeline and constrained presentation views. Components are shared with the Explorer. The model selects an operation or composes approved catalog components; it does not supply executable code or authoritative replacement evidence. An Open in workspace action may expand the view without unexpectedly rearranging tabs.

WebMCP is optional by capability detection and an explicit setting. Page tools expose the selected source/revision/range, open a comparison/source, preview names, and prepare a proposal. The small set uses the page's existing authenticated client functions. No Verify/Approve page tools. Actual browser/host support requires qualification.

## 10. Persistent Views

A View is a versioned Markdown note with human explanation, cited bindings and a fenced Vega-Lite or json-render specification. Bindings retain document, resolved revision, digest, selection, units and transformations. A materialized dataset may be retained in the blob store. Every chart has an accessible data-table representation. A chart that fails to render shows its own alert; the View's text and other charts still render. A saved or presented View binds only to sources in its own workspace.

Pinned is default: exact historical inputs do not refresh silently. Newer source data is an observation, not proof that the historical chart is wrong. Explicit live mode reports each resolved revision and as-of time. A review never automatically covers future live data.

Use json-render core/React and the official MCP Apps bridge, not a second Node MCP server. Use Vega's expression interpreter under a strict CSP. Deny arbitrary network data sources and executable extensions. Renderer specs and reference bindings require runtime validation in addition to wire-schema validation.

The agent may present a candidate, propose it as a View, and have the person accept it into the tree. Reopening, version comparisons, attribution and export operate on the saved artifact. Source text and numeric datasets must resolve from cited application data, not text the agent labels as original.

## 11. Identity, isolation and safety

There are two explicit authentication entry paths, selected by deployment configuration. Both produce the same internal Principal and pass through the same core authorization rules. There is no built-in identity directory, password or account platform in either path, and a missing or invalid hosted configuration never falls back to local authentication.

Local: the installation has one persistent local identity, created on first use and stored in application-owned data; it owns the installation's local workspaces. WorkOS is not required. The browser authenticates through a local session: startup prints a single-use launch URL whose unpredictable token is exchanged for an HttpOnly, SameSite=Strict session cookie. Host and Origin checks, CSRF protection for cookie-authenticated mutations and constrained CORS still apply; loopback binding is never treated as authorization. Local MCP clients use separate connector credentials that the owner issues and revokes: read by default, propose only when enabled, never review or approve. A local MCP client that launches a process (stdio) runs the CLI's `mcp --stdio` command, a relay that forwards each message to the running local service with a connector credential. It holds no store of its own, so the service remains the only writer, and if no service is running it starts one. An agent never receives the browser owner's session credential.

Hosted: WorkOS AuthKit provides browser sessions and WorkOS Connect provides MCP delegation behind GalaxyGate. Publish protected-resource metadata, configure the resource audience, validate JWKS signatures/issuer/audience/expiry, and enforce workspace permissions in the application. Connect access tokens identify the caller (subject, org, client) and do not carry workspace permissions; the application resolves grants through WorkOS authorization role assignments (org-level and per-workspace resources). Subject and client identity are retained separately. A delegated user's identity is not evidence of human review. No Cloudflare path.

Principal identity is not authorization. The Principal carries subject, tenant, route, client and optional delegation ceiling; effective permissions come from AccessControl as WorkspaceGrant and TenantGrant. Workspace list/open responses include the caller's effective permissions so the UI can show the right actions. The hosted adapter may cache Read, Write and Propose grants for at most 60 seconds; Approve, Review and Admin are always checked fresh. The local adapter does not cache. Logging out ends the session immediately.

No passwords, tokens, connector secrets or WorkOS keys appear in browser logs, vendor notes or source. A newly issued connector secret is returned exactly once and never stored in plain text; replaying a completed create_connector returns AlreadyIssued without the secret, while a resumed attempt rotates the secret of the connector it created (section 8).

Uploaded HTML is hostile data until constrained by actual isolation. Use a sandboxed separate origin, no credentials or privileged APIs, restricted navigation/messages and explicit external-resource policy. Do not treat a normal browser iframe as automatically safe.

Untrusted document instructions can still mislead a model with read-only tools. Attention warnings are advisory and false positives are expected; no clean scan becomes a safe-for-AI certificate. Outbound actions and workspace reads stay separately authorized.

Receipt records identify exactly what this server returned, including revision and range, and whether it was supplied to agent context or human display. They do not claim what an external host retained or what the model internally used. Traces may be sampled; durable evidence is not.

## 12. Qualification and deployment obligations

The crash/restart check submits an import, kills execution immediately after acknowledgement, restarts and retries, then opens the same completed result without duplicate occurrence/commit or lost original. It runs against the real RecordStore, the Git/CAS/SQLite import path and the recovery behavior of section 5 (construction gate `job-crash-against-record-store`); nothing short of that proves product crash durability. There is one execution system: do not build a second one alongside the Tokio worker.

Portable export gathers references into an independently readable folder/archive. Full backup additionally preserves promised app records, retained objects and versions, except what Purge (§8) removed. Restore must be exercised. The container requires persistent mounts, health/readiness distinction, HTTPS ingress, explicit WorkOS configuration and restart behavior.

The workshop is a usage scenario, not app state. Compare representations without confusing reorganizing facts with adding new facts. A useful model question or unresolved answer is a valid outcome. The final export should work without the polished interface.

## 13. Phase 0 acceptance and full construction

Phase 0 builds the real foundation. It is complete only after clean-checkout generation, repeat generation with no file-set or byte drift, compilation/exercise of representative consumers, semantic schema controls, and architecture-relevant external library/host qualification. Its green claims no finished product behavior. Blocking is gate-scoped: a qualification may block the foundation, construction, or final acceptance. A later-gate requirement must not be satisfied by adding a temporary earlier-phase implementation.

Generation must not depend on running storage, conversion or the UI, and may never require success-returning product stubs. Synthetic API examples may ship with the API documentation. FixtureApplication and other tests/support implementations never ship as the production application, never seed real workspaces and never become runtime fallback behavior.

The integration owner controls shared types, operation declarations, manifests/lockfiles, generation, deployment and independent acceptance. Seven lanes implement complete responsibilities in isolated worktrees, in parallel against the shared interfaces once the generator foundation is qualified; construction is not a mandatory storage-then-UI sequence. Builders run targeted tests and write regressions; they do not weaken acceptance, hand-edit generated files, suppress lints or silently substitute an imagined dependency.

Before repair distinguish an implementation defect, unfinished neighboring work, and a wrong shared assumption. Fix the defect, continue only genuinely independent work, or request a shared correction. Stop after repeated attempts with no new diagnosis. Budget exhaustion reports unfinished work rather than redefining complete.

Integration assembles during construction. Diagnostic CI may run without demanding a releasable repository at every intermediate commit. Final acceptance tests connected journeys and both capabilities and constraints: a drafter can propose, cannot approve, and a legitimate reviewer can accept the exact reviewed revision. Universal refusal is a product failure.

Required journeys include import/inspect/correct/reopen; naming preview/apply/link preservation; proposed View/approval/render/table/reopen/export; current versus historical review; authenticated cross-workspace denial without breaking permitted access; cancellation/retry/crash recovery; raw HTTP/MCP clients alongside generated clients; browser keyboard and accessibility behavior; portable export and full restore.

The full application is the release target. Do not shrink the tree, graph, naming form, rich viewers or persistent Views to get a quicker green. Do not maximize files, warnings fixed, snapshots accepted, or hours spent. Accept the actual user task with its constraints intact.

## 14. Source conventions and documentation

Stable Rust, forbid authored unsafe, deny all/pedantic plus selected panic/unchecked-operation lints. No allow/expect/cfg_attr suppression or cap-lints workaround. One purpose header plus important invariant; types before behavior; coherent request/response/error groups may share a file. File length is advisory, not a reason to fragment meaning. Generated outputs are not hand-edited.

Rust identities and descriptions are authoritative for wire semantics. The emitted OpenAPI, JSON Schemas, tool catalog, CLI metadata and vendor-generated client distribute them. API generation proves shape, not workflow correctness. Preserve unknown OKF extensions and test omitted/null, tagging, naming and resolved revisions semantically across boundaries.

Root README, AGENTS and this SPEC are the canonical prose. vendors.json is lookup metadata, not another product specification. Local AGENTS files contain only lane differences. CLAUDE.md imports AGENTS.md. Plain task commands remain usable without vendor-specific agent discovery. Ownership is explicit in these documents and enforced locally by isolated worktrees. CODEOWNERS is review routing, not a local editing lock. A ruleset on `main` requires the two always-running CI jobs and blocks force-push and deletion; squash and rebase merging are disabled, so history reaches `main` only through merge commits. No required review exists until real owners replace the placeholder handles.

No documentation shadow implementation, new governance framework, custom workflow language or dependency library is introduced to build this product. Use the libraries' actual APIs. Report what was executed separately from what was authored.
