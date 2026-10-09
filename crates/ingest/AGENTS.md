# Ingestion lane

Read root AGENTS.md and SPEC.md.

This lane's directories and gate command are in the root AGENTS.md table, and its construction gates are the `verification.json` entries whose `owner` is `ingest` (each names its command). Crash durability is not claimed until the SPEC §12 check passes.

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
- **A repeated `MutationId` is a no-op that returns the prior row.** A handler may run twice for one job. A mutation identity names at most one commit: write the `n`-th commit of a job with `CommitChanges { mutation_id: core::storage::derive_commit_mutation_id(claimed.mutation_id, n), author: claimed.initiator, .. }`, counting `n` the same way on every attempt, and name new items with `core::storage::derive_item_id(claimed.mutation_id, n)`, so the second run produces the same edits and `VersionStore::commit` answers each commit with its first revision (`replayed: true`) instead of writing again.
- **Conversion outcomes are results, not errors.** `Converter::convert` returns `Conversion` with status `Success`, `PartialSuccess`, `Unsupported` or `Failure`. Write the source card in every case (`Extraction::Converted`, `Unsupported` or `Failed`); the bytes are already retained. Return `Err` only when the worker itself is at fault.
- **Docling through its real API.** Build `DocumentConverter` from `ConversionSettings` and `ConversionInput.timeout` (`document_timeout`), and open the source with `SourceDocument::from_bytes` using the format from `ConversionInput.file_name`: a retained object's path has no extension. Keep `DoclingDocument::export_to_json` as `ConvertedDocument.structured` beside the Markdown. No Python runtime; do not enable `asr`, `fetch-images` or `vlm` without a deliberate shared change. Record real native and model asset requirements with verified sources, versions and hashes.
- **An asset says how big it is and whose words its caption is.** For a picture, set `ConvertedAsset.pixel_size` from `PictureImage.width` and `height`, `media_type` from `PictureImage.mimetype`, and `caption` from `Node::Picture.caption` with `CaptionOrigin::Source`. Use `CaptionOrigin::Process` only for text the conversion generated; never label generated text as the source's.
- **An item the converter leaves unlocated is located through the text layer, or stays unlocated.** The library drops the box of a PDF caption, so the item exports with `prov: []`. The reference behaviour is `locate::locate_items` in `qualification/docling/src/locate.rs`, which the Docling qualification runs on the corpus: read `docling::pdf_text_layer_pages` from the same bytes, search the pages of the item's parent (or the last page of its nearest located earlier sibling), and take the page and box of the text-layer item only when exactly one has exactly the item's text and its box has area and lies inside the page. Record which source located each item. An item the rule does not find is recorded as unlocated; never give it a guessed box, and never its parent's. The rule does not guarantee that the box it finds is the item's own: `LOCATE_LIMITS` in that file states the two known wrong placements (the same words standing once on the earlier sibling's page; another line of the page that is exactly the item's text), and each lookup records `distance` to the item whose page was searched, without a threshold. Whether the product bounds that distance is the owner's decision; ask before building this gate.
- **Text a font cannot map is flagged, not indexed.** When a PDF font has no Unicode for a glyph the library prints `/` and the glyph's name (`/SM590000`, `/g115`); the words are not in the file and nothing decodes them. The reference behaviour is `glyphs::undecoded_glyphs` (with `placeholder_glyph_tokens` and `is_placeholder_glyph_name`) in `qualification/docling/src/glyphs.rs`, which restates the library's own rule and must be read again on every Docling version change. Detect the placeholders in text items and table cells, keep them out of the search index as words, and report each page that has any as partly extracted. The detector has two known limits: it flags real text of the placeholder's shape that stands after a space (`/B747`, `/v100`, `/tmp123`), and it misses a placeholder glued to a preceding character (`x/g12`). A signal from the library that a glyph had no Unicode is preferred to this detector as soon as the library gives one.
- **Progress and cancellation.** Call `update_progress` between units of work and stop when the returned job is cancelled. `document_timeout` is checked only between PDF pages; the worker supervisor enforces the hard time and memory bound.
- **OKF validity is not decided here.** Use `okf-core` to build concept content; the `CandidateCheck` the application supplies runs before the commit.
- Durable job truth is `RecordStore`; queue delivery is not completion. Run a job only under the lease `claim_job` returns, and write progress, completion and failure through that lease. Reconcile unfinished work through `expire_leases()` and `pending_jobs()` (no tenant argument). No external job engine or queue service is added as a dependency.

Generator-input rule: change only this lane's authored inputs; run `gen` and commit outputs; gen-check must pass; integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
