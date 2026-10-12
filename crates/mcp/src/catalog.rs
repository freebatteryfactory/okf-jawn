//! The generated MCP catalogs and the tool-name-to-operation table.
//!
//! `api/mcp-tools.json` and `api/mcp-apps.json` are committed generator output, so they are
//! compiled in. The App bundle the resource names is uncommitted build output and is read at
//! runtime (see `server`). A tool name is an operation's agent alias in the contract's
//! operation table (`for_each_operation`); nothing here lists tools by hand. Loading refuses a
//! catalog that lists a tool the table does not alias, or that declares any resource other than
//! the one App, so what is listed and what can be called or read never disagree.

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::metadata::OperationName;
use rmcp::model::{Resource, Tool};
use serde_json::Value;

use crate::decode_tools;

macro_rules! tool_aliases {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        /// Every operation's agent tool alias and id, in table order; an empty alias is no tool.
        const TOOL_ALIASES: &[(&str, &str)] = &[$(($alias, stringify!($id))),*];
    };
}

/// The decoded catalogs one server serves for its whole life.
#[derive(Debug, Clone)]
pub struct Catalog {
    /// Every generated tool, exactly as `api/mcp-tools.json` declares it.
    tools: Vec<Tool>,
    /// The one declared App resource.
    app: Resource,
    /// The App resource's declared MIME type.
    app_mime_type: String,
}

/// The URI of the one App resource every tool's `_meta.ui.resourceUri` names.
pub const APP_URI: &str = "ui://okf-jawn/app.html";

/// The MCP Apps HTML profile; a host renders only this type as an App.
pub const APP_MIME_TYPE: &str = "text/html;profile=mcp-app";

/// The committed model and app tool catalog.
const TOOLS_JSON: &str = include_str!("../../../api/mcp-tools.json");

/// The committed App resource declaration.
const APPS_JSON: &str = include_str!("../../../api/mcp-apps.json");

okf_jawn_contract::for_each_operation!(tool_aliases);

impl Catalog {
    /// Decode the committed catalogs and check them against the operation table.
    ///
    /// # Errors
    /// Returns `Internal` when a catalog does not decode as rmcp's types, when a listed tool is
    /// no operation's alias, or when the resources are not exactly the one App with its MIME type.
    pub fn load() -> Result<Self, ApiError> {
        Self::from_documents(TOOLS_JSON, APPS_JSON)
    }

    /// Decode a tool catalog and an App declaration and check them, as `load` does.
    fn from_documents(tools_json: &str, apps_json: &str) -> Result<Self, ApiError> {
        let tools_document: Value =
            serde_json::from_str(tools_json).map_err(|error| catalog_error(&error))?;
        let tools = decode_tools(&tools_document).map_err(|error| catalog_error(&error))?;
        if let Some(tool) = tools
            .iter()
            .find(|tool| operation_of_tool(&tool.name).is_none())
        {
            return Err(ApiError::new(
                ErrorCode::Internal,
                format!(
                    "api/mcp-tools.json lists `{}`, which is no operation's alias",
                    tool.name
                ),
            ));
        }
        let apps_document: Value =
            serde_json::from_str(apps_json).map_err(|error| catalog_error(&error))?;
        let resources: Vec<Resource> = serde_json::from_value(
            apps_document
                .get("resources")
                .cloned()
                .unwrap_or(Value::Null),
        )
        .map_err(|error| catalog_error(&error))?;
        let app = match resources.as_slice() {
            [only] if only.uri == APP_URI => only.clone(),
            _ => {
                return Err(ApiError::new(
                    ErrorCode::Internal,
                    format!("api/mcp-apps.json must declare exactly one resource, {APP_URI}"),
                ));
            }
        };
        let app_mime_type = match app.mime_type.as_deref() {
            Some(APP_MIME_TYPE) => APP_MIME_TYPE.to_owned(),
            _ => {
                return Err(ApiError::new(
                    ErrorCode::Internal,
                    format!("api/mcp-apps.json must declare {APP_URI} as {APP_MIME_TYPE}"),
                ));
            }
        };
        Ok(Self {
            tools,
            app,
            app_mime_type,
        })
    }

    /// Every generated tool, in catalog order.
    #[must_use]
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// The declared App resource, with its `_meta.ui.csp`.
    #[must_use]
    pub const fn app(&self) -> &Resource {
        &self.app
    }

    /// The App resource's declared MIME type.
    #[must_use]
    pub fn app_mime_type(&self) -> &str {
        &self.app_mime_type
    }
}

/// The operation a tool name calls: the operation whose alias column is `name`.
///
/// A canonical operation id is not a tool name, so `read_item` resolves to nothing while
/// `show` resolves to `read_item`; an operation with no alias is never reachable by name.
#[must_use]
pub fn operation_of_tool(name: &str) -> Option<OperationName> {
    let (_, id) = TOOL_ALIASES
        .iter()
        .find(|(alias, _)| !alias.is_empty() && *alias == name)?;
    OperationName::ALL
        .iter()
        .copied()
        .find(|operation| operation.as_str() == *id)
}

fn catalog_error(error: &serde_json::Error) -> ApiError {
    ApiError::new(
        ErrorCode::Internal,
        format!("A generated MCP catalog does not decode as rmcp's types: {error}"),
    )
}

#[cfg(test)]
mod tests {
    //! Source files may not include the shared test idiom (the foundation policy test), so these
    //! unit tests return `Result` and name what they expected in their own errors, as core's do.

    use okf_jawn_contract::error::{ApiError, ErrorCode};
    use serde_json::{Value, json};

    use super::{APP_MIME_TYPE, APP_URI, APPS_JSON, Catalog, TOOLS_JSON};

    type Checked = Result<(), Box<dyn std::error::Error>>;

    /// The committed tool catalog with one more tool, a copy of the first renamed `name`.
    fn tools_with(name: &str) -> Result<String, Box<dyn std::error::Error>> {
        let mut document: Value = serde_json::from_str(TOOLS_JSON)?;
        let tools = document
            .get_mut("tools")
            .and_then(Value::as_array_mut)
            .ok_or("the committed catalog has no tools array")?;
        let mut extra = tools.first().ok_or("the catalog has no tool")?.clone();
        extra
            .as_object_mut()
            .ok_or("a tool is not an object")?
            .insert("name".to_owned(), json!(name));
        tools.push(extra);
        Ok(document.to_string())
    }

    fn app(uri: &str, mime_type: &str) -> Value {
        json!({"uri": uri, "name": "app", "mimeType": mime_type})
    }

    /// The refusal of a load that must have failed.
    fn refused(loaded: Result<Catalog, ApiError>) -> Result<ApiError, String> {
        match loaded {
            Ok(catalog) => Err(format!("expected a refusal, loaded {catalog:?}")),
            Err(error) => Ok(error),
        }
    }

    #[test]
    fn the_committed_catalogs_load() -> Checked {
        let catalog = Catalog::load()?;
        assert_eq!(catalog.tools().len(), 13);
        assert_eq!(catalog.app().uri, APP_URI);
        assert_eq!(catalog.app_mime_type(), APP_MIME_TYPE);
        Ok(())
    }

    #[test]
    fn a_listed_tool_that_is_no_operations_alias_is_refused() -> Checked {
        for name in ["save_draft", "read_item", "invented"] {
            let error = refused(Catalog::from_documents(&tools_with(name)?, APPS_JSON))?;
            assert_eq!(error.code, ErrorCode::Internal, "{name}");
            assert!(error.message.contains(name), "{}", error.message);
        }
        Ok(())
    }

    #[test]
    fn resources_other_than_the_one_app_are_refused() -> Checked {
        let declarations = [
            json!({"resources": []}),
            json!({"resources": [app(APP_URI, APP_MIME_TYPE), app("ui://okf-jawn/two.html", APP_MIME_TYPE)]}),
            json!({"resources": [app("ui://okf-jawn/other.html", APP_MIME_TYPE)]}),
            json!({"resources": [app(APP_URI, "text/html")]}),
        ];
        for declaration in declarations {
            let error = refused(Catalog::from_documents(
                TOOLS_JSON,
                &declaration.to_string(),
            ))?;
            assert_eq!(error.code, ErrorCode::Internal, "{declaration}");
        }
        Ok(())
    }
}
