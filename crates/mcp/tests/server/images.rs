//! A read's selected images: image blocks after the text, or a line of the text that says why not.
//!
//! The fake application answers the read with configured `media`, and `get_object` per object
//! digest; every fetch passes through dispatch, so the fake access decides it like any call.

use okf_jawn_contract::access::Permission;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::read::ReadItemResponse;
use okf_jawn_mcp::{McpServer, read_result};
use rmcp::model::{CallToolResult, ImageContent};
use serde_json::Value;

use crate::check::{TestResult, some};
use crate::support::fakes::{Call, FakeAccess, World};
use crate::support::fixtures::{
    OTHER_WORKSPACE, Outcome, REPORT_MARKDOWN, WORKSPACE, agent, arguments, bundle, digest_of,
    image_read_input, image_read_response, in_region, media, object_block, object_request, on_page,
    text_of, zeros_base64,
};

/// Base64 of the eight-byte PNG signature.
const PNG: &str = "iVBORw0KGgo=";

/// Base64 of the first ten bytes of a JFIF file.
const JPEG: &str = "/9j/4AAQSkZJRg==";

/// Base64 of the six-byte document `<svg/>`.
const SVG: &str = "PHN2Zy8+";

fn reader() -> World {
    World::new(FakeAccess::default().granting("alice", &[Permission::Read]))
}

/// Call `show` as alice with `input` on a server over `world`.
async fn show(world: &World, name: &str, input: Value) -> Outcome<CallToolResult> {
    let server = McpServer::new(world.ports.clone(), bundle(name, "<p>app</p>")?)?;
    let result = server
        .call_tool_as(Some(&agent("alice")?), "show", Some(arguments(input)?))
        .await?;
    assert_eq!(result.is_error, Some(false), "{result:?}");
    Ok(result)
}

/// The image blocks of a result, in order, after checking that the text comes first.
fn images_of(result: &CallToolResult) -> Outcome<Vec<&ImageContent>> {
    let (first, rest) = some(result.content.split_first(), "a content block")?;
    assert!(first.as_text().is_some(), "the text comes first: {first:?}");
    Ok(rest
        .iter()
        .map(|block| some(block.as_image(), "an image block"))
        .collect::<Result<_, _>>()?)
}

/// The read's text followed by one line per image that was not inlined.
fn described(lines: &[String]) -> String {
    format!("{REPORT_MARKDOWN}\n\n{}", lines.join("\n"))
}

#[tokio::test]
async fn a_multimodal_read_returns_its_selected_images_as_image_blocks_in_order() -> TestResult {
    let world = reader();
    let (png, jpeg) = (digest_of('c'), digest_of('d'));
    let response = image_read_response(
        WORKSPACE,
        "multimodal",
        &[
            media(&png, "image/png", Some("Revenue by quarter"), &on_page(2)),
            media(&jpeg, "Image/JPEG; q=1", None, &on_page(3)),
        ],
    );
    world
        .application
        .answer_with("read_item", response.clone())?;
    world
        .application
        .serve_object(&png, Ok(object_block(&png, "image/png", PNG, 8, false)))?;
    world.application.serve_object(
        &jpeg,
        Ok(object_block(&jpeg, "image/jpeg", JPEG, 10, false)),
    )?;
    let result = show(
        &world,
        "images-two.html",
        image_read_input("multimodal", 1024, 2),
    )
    .await?;
    assert_eq!(text_of(&result)?, REPORT_MARKDOWN);
    let images = images_of(&result)?;
    assert_eq!(images.len(), 2);
    let first = some(images.first(), "the first image")?;
    assert_eq!(
        (first.data.as_str(), first.mime_type.as_str()),
        (PNG, "image/png")
    );
    // The block carries the type the bytes were served with, not the one the read declared.
    let second = some(images.get(1), "the second image")?;
    assert_eq!(
        (second.data.as_str(), second.mime_type.as_str()),
        (JPEG, "image/jpeg")
    );
    assert_eq!(result.structured_content, Some(response));
    let alice = |operation| Call {
        operation,
        subject: "alice".to_owned(),
    };
    assert_eq!(
        world.application.calls()?,
        vec![alice("read_item"), alice("get_object"), alice("get_object")]
    );
    // Each fetch cites the read's resolution and asks for what is left of the image budget.
    assert_eq!(
        world.application.requests("get_object")?,
        vec![
            object_request(WORKSPACE, &png, 1024),
            object_request(WORKSPACE, &jpeg, 1016),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn images_over_the_budget_are_described_not_inlined() -> TestResult {
    let world = reader();
    let (fits, complete, partial) = (digest_of('c'), digest_of('d'), digest_of('e'));
    world.application.answer_with(
        "read_item",
        image_read_response(
            WORKSPACE,
            "multimodal",
            &[
                media(&fits, "image/png", None, &on_page(2)),
                media(&complete, "image/png", Some("Margins"), &on_page(3)),
                media(&partial, "image/png", None, &on_page(4)),
            ],
        ),
    )?;
    let first = zeros_base64(900);
    world.application.serve_object(
        &fits,
        Ok(object_block(&fits, "image/png", &first, 900, false)),
    )?;
    // Complete, but larger than the 124 bytes it was asked for.
    world.application.serve_object(
        &complete,
        Ok(object_block(
            &complete,
            "image/png",
            &zeros_base64(900),
            900,
            false,
        )),
    )?;
    // Exactly the 124 bytes it was asked for, of a 5000-byte object: never inlined in part.
    world.application.serve_object(
        &partial,
        Ok(object_block(
            &partial,
            "image/png",
            &zeros_base64(124),
            5000,
            true,
        )),
    )?;
    let result = show(
        &world,
        "images-budget.html",
        image_read_input("multimodal", 1024, 3),
    )
    .await?;
    let images = images_of(&result)?;
    assert_eq!(images.len(), 1);
    assert_eq!(some(images.first(), "the image")?.data, first);
    assert_eq!(
        text_of(&result)?,
        described(&[
            format!(
                "[Image not inlined: picture \"Margins\", page 3, object {complete}: \
                 too large (900 bytes; 124 bytes of the image budget left)]"
            ),
            format!(
                "[Image not inlined: picture, page 4, object {partial}: \
                 too large (5000 bytes; 124 bytes of the image budget left)]"
            ),
        ])
    );
    assert_eq!(
        world.application.requests("get_object")?,
        vec![
            object_request(WORKSPACE, &fits, 1024),
            object_request(WORKSPACE, &complete, 124),
            object_request(WORKSPACE, &partial, 124),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn once_the_image_budget_is_spent_no_image_is_fetched() -> TestResult {
    let world = reader();
    let (whole, after) = (digest_of('c'), digest_of('d'));
    world.application.answer_with(
        "read_item",
        image_read_response(
            WORKSPACE,
            "multimodal",
            &[
                media(&whole, "image/png", None, &on_page(1)),
                media(&after, "image/png", None, &on_page(2)),
            ],
        ),
    )?;
    world.application.serve_object(
        &whole,
        Ok(object_block(
            &whole,
            "image/png",
            &zeros_base64(256),
            256,
            false,
        )),
    )?;
    world
        .application
        .serve_object(&after, Ok(object_block(&after, "image/png", PNG, 8, false)))?;
    let result = show(
        &world,
        "images-spent.html",
        image_read_input("multimodal", 256, 2),
    )
    .await?;
    assert_eq!(images_of(&result)?.len(), 1);
    assert_eq!(
        text_of(&result)?,
        described(&[format!(
            "[Image not inlined: picture, page 2, object {after}: \
             too large (no image budget left)]"
        )])
    );
    assert_eq!(
        world.application.requests("get_object")?,
        vec![object_request(WORKSPACE, &whole, 256)]
    );
    Ok(())
}

#[tokio::test]
async fn a_media_type_that_is_no_image_is_described() -> TestResult {
    let world = reader();
    let (declared, served) = (digest_of('c'), digest_of('d'));
    world.application.answer_with(
        "read_item",
        image_read_response(
            WORKSPACE,
            "multimodal",
            &[
                media(
                    &declared,
                    "application/pdf",
                    Some("Appendix\nscan"),
                    &in_region(5),
                ),
                media(&served, "image/png", None, &on_page(6)),
            ],
        ),
    )?;
    world.application.serve_object(
        &served,
        Ok(object_block(&served, "text/plain", "aGVsbG8=", 5, false)),
    )?;
    let result = show(
        &world,
        "images-types.html",
        image_read_input("multimodal", 1024, 2),
    )
    .await?;
    assert_eq!(images_of(&result)?.len(), 0);
    assert_eq!(
        text_of(&result)?,
        described(&[
            format!(
                "[Image not inlined: picture \"Appendix scan\", a region of page 5, \
                 object {declared}: not an inline image type (application/pdf)]"
            ),
            format!(
                "[Image not inlined: picture, page 6, object {served}: \
                 not an inline image type (text/plain)]"
            ),
        ])
    );
    // A media type the read declares as no image is not fetched.
    assert_eq!(
        world.application.requests("get_object")?,
        vec![object_request(WORKSPACE, &served, 1024)]
    );
    Ok(())
}

/// SVG can carry script and the document is untrusted, so an SVG is never an image block:
/// neither one the read declares nor bytes served as SVG under a raster declaration.
#[tokio::test]
async fn an_svg_image_is_described_never_inlined() -> TestResult {
    let world = reader();
    let (declared, served) = (digest_of('c'), digest_of('d'));
    world.application.answer_with(
        "read_item",
        image_read_response(
            WORKSPACE,
            "multimodal",
            &[
                media(&declared, "image/svg+xml", Some("Logo"), &on_page(1)),
                media(&served, "image/png", None, &on_page(2)),
            ],
        ),
    )?;
    for digest in [&declared, &served] {
        world.application.serve_object(
            digest,
            Ok(object_block(digest, "image/svg+xml", SVG, 6, false)),
        )?;
    }
    let result = show(
        &world,
        "images-svg.html",
        image_read_input("multimodal", 1024, 2),
    )
    .await?;
    assert_eq!(images_of(&result)?.len(), 0);
    assert_eq!(
        text_of(&result)?,
        described(&[
            format!(
                "[Image not inlined: picture \"Logo\", page 1, object {declared}: \
                 not an inline image type (image/svg+xml)]"
            ),
            format!(
                "[Image not inlined: picture, page 2, object {served}: \
                 not an inline image type (image/svg+xml)]"
            ),
        ])
    );
    assert_eq!(
        world.application.requests("get_object")?,
        vec![object_request(WORKSPACE, &served, 1024)]
    );
    Ok(())
}

#[tokio::test]
async fn a_refused_fetch_is_described_and_the_read_still_succeeds() -> TestResult {
    let world = reader();
    let (refused, served) = (digest_of('c'), digest_of('d'));
    let response = image_read_response(
        WORKSPACE,
        "pages",
        &[
            media(&refused, "image/png", None, &on_page(1)),
            media(&served, "image/png", None, &on_page(2)),
        ],
    );
    world
        .application
        .answer_with("read_item", response.clone())?;
    world.application.serve_object(
        &refused,
        Err(ApiError::new(
            ErrorCode::NotFound,
            "The object is not part of this revision",
        )),
    )?;
    world.application.serve_object(
        &served,
        Ok(object_block(&served, "image/png", PNG, 8, false)),
    )?;
    let result = show(
        &world,
        "images-refused.html",
        image_read_input("pages", 1024, 2),
    )
    .await?;
    let images = images_of(&result)?;
    assert_eq!(images.len(), 1);
    assert_eq!(some(images.first(), "the image")?.data, PNG);
    assert_eq!(
        text_of(&result)?,
        described(&[format!(
            "[Image not inlined: picture, page 1, object {refused}: \
             the fetch was refused (not_found)]"
        )])
    );
    assert_eq!(result.structured_content, Some(response));
    Ok(())
}

/// The read cites a workspace the caller may not read, so dispatch refuses each fetch before
/// the application: were a fetch to bypass dispatch, the application would serve the image.
#[tokio::test]
async fn a_fetch_dispatch_refuses_is_reported_never_bypassed() -> TestResult {
    let world = World::new(
        FakeAccess::default()
            .granting("alice", &[Permission::Read])
            .refusing(OTHER_WORKSPACE),
    );
    let image = digest_of('c');
    world.application.answer_with(
        "read_item",
        image_read_response(
            OTHER_WORKSPACE,
            "multimodal",
            &[media(&image, "image/png", Some("Chart"), &on_page(1))],
        ),
    )?;
    world
        .application
        .serve_object(&image, Ok(object_block(&image, "image/png", PNG, 8, false)))?;
    let result = show(
        &world,
        "images-forbidden.html",
        image_read_input("multimodal", 1024, 1),
    )
    .await?;
    assert_eq!(images_of(&result)?.len(), 0);
    assert_eq!(
        text_of(&result)?,
        described(&[format!(
            "[Image not inlined: picture \"Chart\", page 1, object {image}: \
             the fetch was refused (forbidden)]"
        )])
    );
    assert_eq!(
        world.application.requests("get_object")?,
        Vec::<Value>::new()
    );
    let appended = world.events.appended()?;
    assert_eq!(appended.len(), 1);
    let (_, event) = some(appended.first(), "the refusal")?;
    assert_eq!(event.kind, EventKind::PermissionDenied);
    assert_eq!(event.operation, Some(OperationName::GetObject));
    Ok(())
}

#[tokio::test]
async fn a_read_that_selects_no_image_is_text_alone_and_fetches_nothing() -> TestResult {
    let world = reader();
    let response = image_read_response(WORKSPACE, "multimodal", &[]);
    world
        .application
        .answer_with("read_item", response.clone())?;
    let result = show(
        &world,
        "images-none.html",
        image_read_input("multimodal", 1024, 0),
    )
    .await?;
    let decoded: ReadItemResponse = serde_json::from_value(response)?;
    assert_eq!(result, read_result(&decoded, 1024, &[])?);
    assert_eq!(result.content.len(), 1);
    assert_eq!(text_of(&result)?, REPORT_MARKDOWN);
    assert_eq!(
        world.application.requests("get_object")?,
        Vec::<Value>::new()
    );
    Ok(())
}
