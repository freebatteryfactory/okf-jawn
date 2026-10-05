# Ingestion lane

Read root AGENTS.md and SPEC.md.

**Directories:** `crates/ingest/`

**Gates** (construction gates in `verification.json` owned by this lane):

| Gate | Command or receipt |
| --- | --- |
| `converter-worker-memory-ceiling` | `cargo test -p okf-jawn-ingest --features runtime` |
| `job-crash-against-record-store` | `.artifacts/qualification/` or a construction receipt for import restart: kill and restart against the real `RecordStore`; no duplicate effect |

**Receipt:** conversion and job-handler tests; restart and idempotency evidence under `.artifacts/` (not a claim until the SPEC §12 check passes).

## Ports to implement

| Port | Defined in | Built with |
| --- | --- | --- |
| `Converter` | `core::conversion` | `docling` (`default-features = false`, `features = ["pdf"]`) |
| `JobHandler` | `core::jobs` | the ports below |
| `JobQueue` | `core::jobs` | Tokio, in process |

## Ports to call (implemented by storage)

`RecordStore::{claim_job, update_progress, record_artifact, get_artifact, complete_job, fail_job, pending_jobs, expire_leases}`, `UploadStore::get`, `BlobStore::{put, open, materialize}`, `VersionStore::{head, show, commit}`, `SearchIndex::{index_revision, rebuild}`, `EventLog::append`.

## Rules

- **A claim is all a handler gets and all it needs.** `ClaimedJob` carries the scope, the lease, the `JobSpec` (the request's inputs with revisions resolved), the initiator and the job's `MutationId`. Do not read request state from anywhere else.
- **A repeated `MutationId` is a no-op that returns the prior row.** A handler may run twice for one job. Write every commit with `CommitChanges { mutation_id: claimed.mutation_id, author: claimed.initiator, .. }` and name new items with `core::storage::derive_item_id(claimed.mutation_id, n)`, so the second run produces the same edits and `VersionStore::commit` answers with the first revision (`replayed: true`) instead of writing again.
- **Conversion outcomes are results, not errors.** `Converter::convert` returns `Conversion` with status `Success`, `PartialSuccess`, `Unsupported` or `Failure`. Write the source card in every case (`Extraction::Converted`, `Unsupported` or `Failed`); the bytes are already retained. Return `Err` only when the worker itself is at fault.
- **Docling through its real API.** Build `DocumentConverter` from `ConversionSettings` and `ConversionInput.timeout` (`document_timeout`), and open the source with `SourceDocument::from_bytes` using the format from `ConversionInput.file_name`: a retained object's path has no extension. Keep `DoclingDocument::export_to_json` as `ConvertedDocument.structured` beside the Markdown. No Python runtime; do not enable `asr`, `fetch-images` or `vlm` without a deliberate shared change. Record real native and model asset requirements with verified sources, versions and hashes.
- **An asset says how big it is and whose words its caption is.** For a picture, set `ConvertedAsset.pixel_size` from `PictureImage.width` and `height`, `media_type` from `PictureImage.mimetype`, and `caption` from `Node::Picture.caption` with `CaptionOrigin::Source`. Use `CaptionOrigin::Process` only for text the conversion generated; never label generated text as the source's.
- **Progress and cancellation.** Call `update_progress` between units of work and stop when the returned job is cancelled. `document_timeout` is checked only between PDF pages; the worker supervisor enforces the hard time and memory bound.
- **OKF validity is not decided here.** Use `okf-core` to build concept content; the `CandidateCheck` the application supplies runs before the commit.
- iii Phase 0 qualification ended REJECTED_WITH_FALLBACK; do not re-adopt iii or add `iii-sdk` as a product dependency. Durable job truth is `RecordStore`; queue delivery is not completion. Reconcile unfinished work through `pending_jobs()` (no tenant argument).

Generator-input rule: change only this lane's authored inputs; run `gen` and commit outputs; gen-check must pass; integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
