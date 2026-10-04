//! Operator command vocabulary is a projection of canonical operation identifiers.

/// Return a familiar operator alias without changing the operation meaning.
#[must_use]
pub fn operator_alias(operation: &str) -> Option<&'static str> {
    match operation {
        "log_items" => Some("timeline"),
        "diff_items" => Some("changes"),
        "commit_items" => Some("snapshot"),
        "restore_items" => Some("rewind"),
        "blame_item" => Some("who"),
        "accept_proposal" => Some("approve"),
        "decline_proposal" => Some("decline"),
        "start_import" => Some("import"),
        "export_workspace" => Some("export"),
        "get_attention" => Some("attention"),
        "create_review" => Some("verify"),
        _ => None,
    }
}
