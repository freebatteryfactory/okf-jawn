//! Serialized request examples are typed; expected outcomes live separately in tests.

use std::path::Path;

use crate::output::write_json;
use okf_jawn_contract::common::{PageRange, PageRequest};
use okf_jawn_contract::identity::{At, IdempotencyKey, ItemId, Revision, WorkspaceId};
use okf_jawn_contract::read::{ReadItemRequest, ReadView, Selection};

pub(crate) fn generate(directory: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let workspace_id: WorkspaceId =
        serde_json::from_str("\"11111111-1111-4111-8111-111111111111\"")?;
    let item_id: ItemId = serde_json::from_str("\"22222222-2222-4222-8222-222222222222\"")?;
    let idempotency_key: IdempotencyKey =
        serde_json::from_str("\"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa\"")?;
    let read = ReadItemRequest {
        workspace_id,
        item_id,
        at: At::Latest,
        view: ReadView::Pages,
        selection: Selection::Pages {
            range: PageRange { start: 1, end: 2 },
        },
        max_bytes: 32768,
        max_images: 2,
        cursor: None,
    };
    write_json(
        &directory.join("read-item.json"),
        &serde_json::to_value(read)?,
    )?;
    let list = okf_jawn_contract::item::ListItemsRequest {
        workspace_id,
        at: At::Revision {
            revision: Revision::try_from("a".repeat(40))?,
        },
        folder: String::new(),
        page: PageRequest {
            cursor: None,
            limit: 50,
        },
    };
    write_json(
        &directory.join("list-items.json"),
        &serde_json::to_value(list)?,
    )?;
    write_json(
        &directory.join("create-workspace.json"),
        &serde_json::to_value(okf_jawn_contract::workspace::CreateWorkspaceRequest {
            name: "My workspace".to_owned(),
            description: "A blank user-organized context workspace".to_owned(),
            idempotency_key,
        })?,
    )?;
    Ok(())
}
