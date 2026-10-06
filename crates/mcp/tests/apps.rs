//! The committed MCP App declaration, decoded by rmcp's own resource type.
//!
//! `api/mcp-apps.json` is checked as JSON elsewhere; here every `resources` entry must
//! deserialize into `rmcp::model::Resource` and serialize back to the same document, so the
//! generated file is something an rmcp server can really list.

use std::path::Path;

use rmcp::model::Resource;
use serde_json::Value;

use check::{TestResult, some};

const APP_URI: &str = "ui://okf-jawn/app.html";
const APP_MIME_TYPE: &str = "text/html;profile=mcp-app";

fn declared_resources() -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../api/mcp-apps.json");
    let document: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let resources = some(
        document.get("resources"),
        "`resources` in api/mcp-apps.json",
    )?;
    Ok(some(resources.as_array(), "`resources` to be an array")?.clone())
}

#[test]
fn every_declared_resource_decodes_as_an_rmcp_resource() -> TestResult {
    let entries = declared_resources()?;
    assert!(
        !entries.is_empty(),
        "api/mcp-apps.json declares no resource"
    );
    for entry in entries {
        let resource: Resource = serde_json::from_value(entry.clone())?;
        assert_eq!(resource.uri, APP_URI);
        assert_eq!(resource.mime_type.as_deref(), Some(APP_MIME_TYPE));
        assert_eq!(
            serde_json::to_value(&resource)?,
            entry,
            "the decoded resource must serialize back to the committed entry"
        );
    }
    Ok(())
}

#[test]
fn the_content_security_policy_survives_the_round_trip() -> TestResult {
    for entry in declared_resources()? {
        let declared = some(
            entry.pointer("/_meta/ui/csp"),
            "`_meta.ui.csp` in the committed entry",
        )?;
        let resource: Resource = serde_json::from_value(entry.clone())?;
        let meta = some(resource.meta.as_ref(), "`_meta` on the decoded resource")?;
        let decoded = some(
            meta.0.get("ui").and_then(|ui| ui.get("csp")),
            "`_meta.ui.csp` on the decoded resource",
        )?;
        assert_eq!(decoded, declared);
        let encoded = serde_json::to_value(&resource)?;
        assert_eq!(encoded.pointer("/_meta/ui/csp"), Some(declared));
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
