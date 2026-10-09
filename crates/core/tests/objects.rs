//! The three functions that compose ports are tested against minimal fakes of those ports:
//! `authorize_object` decides what a digest is to an item revision, `check_revision` refuses a
//! purged revision, and `retain_dataset` retains and records a dataset. Each case fails when the
//! rule it names is removed.

use std::collections::BTreeMap;

use okf_jawn_contract::common::TextRange;
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::extraction::{ExtractionStatus, ExtractionSummary, TextOrigin};
use okf_jawn_contract::identity::{Digest, ItemId, PurgeId, Revision, TenantId, WorkspaceId};
use okf_jawn_contract::item::{ItemDocument, ItemKind, ItemStatus, ItemSummary};
use okf_jawn_contract::read::{AssetRole, Selection};
use okf_jawn_contract::views::{Dataset, ViewBinding};
use okf_jawn_core::jobs::{DerivedKind, DerivedObject, RevisionMapping};
use okf_jawn_core::reading::{
    MAX_CONVERSION_RECORD_BYTES, ObjectRole, authorize_object, check_revision,
};
use okf_jawn_core::storage::StorageScope;
use okf_jawn_core::views::{DATASET_MEDIA_TYPE, dataset_bytes, materialize, retain_dataset};
use uuid::Uuid;

use check::{TestResult, err_of, some};
use fakes::{FakeBlobs, FakeRecords, FakeVersions, Open};
use records::{Built, ITEM, appearance, citation, converted, digest, record, revision};

fn scope() -> Built<StorageScope> {
    Ok(StorageScope {
        tenant_id: TenantId::try_from("tenant-local".to_owned())?,
        workspace_id: WorkspaceId(Uuid::from_u128(1)),
    })
}

fn item() -> ItemId {
    ItemId(Uuid::from_u128(ITEM))
}

/// The source card of the fixture item: original `a`, record `b`, an export `c`, a page image
/// `d`.
fn card() -> Built<FakeVersions> {
    let source = appearance()?;
    Ok(FakeVersions {
        document: ItemDocument {
            summary: ItemSummary {
                id: item(),
                path: citation(revision('1')?, Selection::All)?.path,
                title: "Report".to_owned(),
                description: String::new(),
                type_name: "source".to_owned(),
                kind: ItemKind::Source,
                revision: revision('1')?,
                status: ItemStatus::Stable,
                archived: false,
                media_type: Some(source.media_type.clone()),
                extraction: Some(ExtractionSummary {
                    status: ExtractionStatus::Completed,
                    text_origin: TextOrigin::Converter,
                    corrected: false,
                }),
            },
            body: String::new(),
            properties: BTreeMap::new(),
            source: Some(source),
            draft: None,
        },
    })
}

/// A blob store that serves the fixture conversion record.
fn serving_the_record() -> Built<FakeBlobs> {
    Ok(FakeBlobs::new(Open::Serve(serde_json::to_vec(&record()?)?)))
}

fn dataset_object(revision: Revision, item_id: ItemId, fill: char) -> Built<DerivedObject> {
    Ok(DerivedObject {
        item_id,
        revision,
        digest: digest(fill)?,
        kind: DerivedKind::Dataset,
        media_type: DATASET_MEDIA_TYPE.to_owned(),
    })
}

/// What `digest` is to the fixture item at revision `1`, as `authorize_object` decides it.
async fn role_of(
    versions: &FakeVersions,
    blobs: &FakeBlobs,
    records: &FakeRecords,
    digest: &Digest,
) -> Result<ObjectRole, ApiError> {
    let internal =
        |error: Box<dyn std::error::Error>| ApiError::new(ErrorCode::Internal, error.to_string());
    let scope = scope().map_err(internal)?;
    let at = revision('1').map_err(internal)?;
    authorize_object(versions, blobs, records, &scope, item(), &at, digest).await
}

#[tokio::test]
async fn the_original_and_the_record_are_decided_without_reading_the_record() -> TestResult {
    let versions = card()?;
    let blobs = FakeBlobs::new(Open::Fail);
    let records = FakeRecords::new(Vec::new(), None);
    assert_eq!(
        role_of(&versions, &blobs, &records, &digest('a')?).await?,
        ObjectRole::Original
    );
    assert_eq!(
        role_of(&versions, &blobs, &records, &digest('b')?).await?,
        ObjectRole::ConversionRecord
    );
    assert_eq!(blobs.opens(), 0);
    Ok(())
}

#[tokio::test]
async fn an_asset_listed_in_the_record_is_that_asset() -> TestResult {
    let versions = card()?;
    let blobs = serving_the_record()?;
    let records = FakeRecords::new(Vec::new(), None);
    assert_eq!(
        role_of(&versions, &blobs, &records, &digest('d')?).await?,
        ObjectRole::Asset(AssetRole::PageImage)
    );
    assert_eq!(
        role_of(&versions, &blobs, &records, &digest('c')?).await?,
        ObjectRole::StructuredExport
    );
    assert_eq!(blobs.opens(), 2);
    Ok(())
}

#[tokio::test]
async fn a_recorded_dataset_is_decided_before_the_record_is_read() -> TestResult {
    let versions = card()?;
    let recorded = vec![dataset_object(revision('1')?, item(), 'f')?];
    // The record cannot be read: the store fails, or reports a record past what core reads.
    for open in [Open::Fail, Open::Oversized(MAX_CONVERSION_RECORD_BYTES + 1)] {
        let blobs = FakeBlobs::new(open);
        let records = FakeRecords::new(recorded.clone(), None);
        assert_eq!(
            role_of(&versions, &blobs, &records, &digest('f')?).await?,
            ObjectRole::Derived(DerivedKind::Dataset)
        );
        assert_eq!(blobs.opens(), 0);
        // The same unreadable record is an error for a digest the record would have to decide.
        let unknown = err_of(role_of(&versions, &blobs, &records, &digest('9')?).await)?;
        assert_ne!(unknown.code, ErrorCode::NotFound);
    }
    Ok(())
}

#[tokio::test]
async fn a_dataset_recorded_for_another_revision_or_item_is_not_found() -> TestResult {
    let versions = card()?;
    let blobs = serving_the_record()?;
    let other_item = ItemId(Uuid::from_u128(ITEM + 1));
    for object in [
        dataset_object(revision('2')?, item(), 'f')?,
        dataset_object(revision('1')?, other_item, 'f')?,
    ] {
        let records = FakeRecords::new(vec![object], None);
        let refused = err_of(role_of(&versions, &blobs, &records, &digest('f')?).await)?;
        assert_eq!(refused.code, ErrorCode::NotFound);
    }
    Ok(())
}

#[tokio::test]
async fn an_unknown_digest_is_not_found() -> TestResult {
    let versions = card()?;
    let blobs = serving_the_record()?;
    let records = FakeRecords::new(Vec::new(), None);
    let refused = err_of(role_of(&versions, &blobs, &records, &digest('9')?).await)?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    Ok(())
}

#[tokio::test]
async fn an_unmapped_revision_is_readable_and_a_mapped_one_is_invalidated() -> TestResult {
    let at = revision('1')?;
    let replacement = revision('2')?;
    let purge_id = PurgeId(Uuid::from_u128(13));
    let unmapped = FakeRecords::new(Vec::new(), None);
    check_revision(&unmapped, &scope()?, &at).await?;

    let mapped = FakeRecords::new(
        Vec::new(),
        Some(RevisionMapping {
            purge_id,
            replacement: Some(replacement.clone()),
        }),
    );
    let refused = err_of(check_revision(&mapped, &scope()?, &at).await)?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    let detail = some(refused.detail.as_deref(), "the invalidation detail")?;
    let ErrorDetail::Invalidated {
        purge_id: named,
        replacement: re_pin,
    } = detail
    else {
        return Err(format!("expected Invalidated, got {detail:?}").into());
    };
    assert_eq!(*named, purge_id);
    assert_eq!(re_pin.as_ref(), Some(&replacement));
    Ok(())
}

fn binding() -> Built<ViewBinding> {
    Ok(ViewBinding {
        name: "revenue".to_owned(),
        source: citation(
            revision('1')?,
            Selection::Lines {
                range: TextRange { start: 5, end: 8 },
            },
        )?,
        units: BTreeMap::new(),
        transforms: Vec::new(),
        materialized: None,
    })
}

fn dataset(binding: &ViewBinding) -> Built<Dataset> {
    let extraction = converted()?;
    Ok(materialize(binding, &record()?, &extraction, &[]).map_err(|failure| failure.message)?)
}

#[tokio::test]
async fn a_retained_dataset_is_put_by_its_digest_and_recorded_for_its_binding() -> TestResult {
    let binding = binding()?;
    let dataset = dataset(&binding)?;
    let (expected_bytes, expected_digest) = dataset_bytes(&dataset)?;
    let blobs = FakeBlobs::new(Open::Fail);
    let records = FakeRecords::new(Vec::new(), None);
    let scope = scope()?;

    let first = retain_dataset(&blobs, &records, &scope, &binding, &dataset).await?;
    assert_eq!(first.item_id, binding.source.item_id);
    assert_eq!(first.revision, binding.source.revision);
    assert_eq!(first.kind, DerivedKind::Dataset);
    assert_eq!(first.media_type, DATASET_MEDIA_TYPE);
    assert_eq!(first.digest, expected_digest);

    let puts = blobs.puts()?;
    let put = some(puts.first(), "the dataset's bytes")?;
    assert_eq!(puts.len(), 1);
    assert_eq!(put.bytes, expected_bytes);
    assert_eq!(put.expected.as_ref(), Some(&expected_digest));
    assert_eq!(put.limit, u64::try_from(expected_bytes.len())?);
    assert_eq!(records.recorded()?, vec![first.clone()]);

    // Retaining again names the same digest and records nothing new.
    let again = retain_dataset(&blobs, &records, &scope, &binding, &dataset).await?;
    assert_eq!(again, first);
    assert_eq!(records.recorded()?, vec![first]);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "support/ports.rs"]
mod fakes;
#[path = "support/records.rs"]
mod records;
