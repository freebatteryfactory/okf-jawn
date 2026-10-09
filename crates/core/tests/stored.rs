//! Values read back from storage meet their schema: serde alone accepts a field beside a unit
//! variant's tag, the stored decoders do not.

use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::identity::Revision;
use okf_jawn_core::conversion::ConversionSettings;
use okf_jawn_core::echo::ECHO_LIMIT;
use okf_jawn_core::jobs::JobSpec;
use serde_json::json;
use uuid::Uuid;

use check::{TestResult, err_of};

fn import() -> Result<JobSpec, Box<dyn std::error::Error>> {
    Ok(JobSpec::Import {
        base_revision: Revision::try_from("a".repeat(40))?,
        upload_ids: vec![okf_jawn_contract::identity::UploadId(Uuid::from_u128(9))],
        destination: None,
        apply_naming_rules: true,
        settings: ConversionSettings::default(),
    })
}

#[test]
fn serde_alone_accepts_a_field_beside_a_unit_variant() -> TestResult {
    // The gap this module closes: plain serde decodes it.
    let stray = json!({ "kind": "rebuild_index", "unexpected": 1 });
    let accepted: JobSpec = serde_json::from_value(stray.clone())?;
    assert_eq!(accepted, JobSpec::RebuildIndex);
    let refused = err_of(JobSpec::from_stored(stray))?;
    assert_eq!(refused.code, ErrorCode::Internal);
    for kind in ["backup_workspace", "backup_installation"] {
        err_of(JobSpec::from_stored(
            json!({ "kind": kind, "unexpected": true }),
        ))?;
    }
    Ok(())
}

#[test]
fn every_well_formed_specification_round_trips_through_storage() -> TestResult {
    for spec in [
        import()?,
        JobSpec::RebuildIndex,
        JobSpec::BackupInstallation,
    ] {
        assert_eq!(JobSpec::from_stored(spec.to_stored()?)?, spec);
    }
    let unknown_kind = err_of(JobSpec::from_stored(json!({ "kind": "compact" })))?;
    assert_eq!(unknown_kind.code, ErrorCode::Internal);
    Ok(())
}

#[test]
fn a_stored_value_is_quoted_in_a_refusal_only_up_to_a_bound() -> TestResult {
    // The validator quotes the offending value; a stored value of megabytes must not come back
    // whole. Multi-byte characters put the cut inside a character unless it is moved back.
    let huge = "é".repeat(100_000);
    let refused = err_of(JobSpec::from_stored(json!({ "kind": huge })))?;
    assert_eq!(refused.code, ErrorCode::Internal);
    let prefix = "a stored job specification does not meet its schema: ";
    assert!(refused.message.starts_with(prefix), "{}", refused.message);
    assert!(
        refused.message.len() <= prefix.len() + ECHO_LIMIT + '…'.len_utf8(),
        "{} bytes",
        refused.message.len()
    );
    assert!(refused.message.ends_with('…'));
    // A short value is quoted whole.
    let short = err_of(JobSpec::from_stored(json!({ "kind": "compact" })))?;
    assert!(short.message.contains("compact"));
    assert!(!short.message.ends_with('…'));
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
