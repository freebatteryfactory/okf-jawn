//! A read's selected images: image blocks after the text, or a line of the text that says why not.
//!
//! The fake application answers the read with configured `media`, and `get_object` per object
//! digest; every fetch passes through dispatch, so the fake access decides it like any call.

use okf_jawn_contract::access::{AccessRoute, DelegationCeiling, Permission};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::read::ReadItemResponse;
use okf_jawn_mcp::{McpServer, read_result};
use rmcp::model::{CallToolResult, ImageContent};
use serde_json::{Value, json};

use crate::check::{TestResult, some};
use crate::support::fakes::{Call, FakeAccess, World};
use crate::support::fixtures::{
    OTHER_WORKSPACE, Outcome, PNG_SIGNATURE, REPORT_MARKDOWN, WORKSPACE, agent, arguments,
    base64_of, bundle, digest_of, image_read_input, image_read_response, in_region, media,
    object_block, object_request, on_page, png_base64, text_of, zeros_base64,
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
    let first = png_base64(900);
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
            &png_base64(256),
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

/// Read one image whose object is `bytes`, declared and served as `served`, within a 1024-byte
/// budget (on a server whose bundle file is `name`), and check what becomes of it: an image block of exactly those bytes and that type
/// when `shown`, and otherwise no block and the one line that says the content is not the type
/// it claims. Either way the structured content is the read's response and one fetch was made.
async fn check_served(name: &str, served: &str, bytes: &[u8], shown: bool) -> TestResult {
    let case = format!("{bytes:02x?} served as {served}");
    let world = reader();
    let image = digest_of('c');
    let response = image_read_response(
        WORKSPACE,
        "multimodal",
        &[media(&image, served, None, &on_page(1))],
    );
    world
        .application
        .answer_with("read_item", response.clone())?;
    let data = base64_of(bytes);
    world.application.serve_object(
        &image,
        Ok(object_block(&image, served, &data, bytes.len(), false)),
    )?;
    let result = show(&world, name, image_read_input("multimodal", 1024, 1)).await?;
    let images = images_of(&result)?;
    if shown {
        assert_eq!(images.len(), 1, "{case}");
        let block = some(images.first(), "the image")?;
        assert_eq!(
            (block.data.as_str(), block.mime_type.as_str()),
            (data.as_str(), served),
            "{case}"
        );
        assert_eq!(text_of(&result)?, REPORT_MARKDOWN, "{case}");
    } else {
        assert_eq!(images.len(), 0, "{case}");
        assert_eq!(
            text_of(&result)?,
            described(&[format!(
                "[Image not inlined: picture, page 1, object {image}: \
                 the content is not the type it claims ({served})]"
            )]),
            "{case}"
        );
    }
    assert_eq!(result.structured_content, Some(response), "{case}");
    assert_eq!(
        world.application.requests("get_object")?,
        vec![object_request(WORKSPACE, &image, 1024)],
        "{case}"
    );
    Ok(())
}

/// The fixtures' encoder is written apart from the server's reading of base64; it agrees with
/// RFC 4648's vectors and with the constants this file states by hand.
#[test]
fn the_test_encoder_writes_standard_padded_base64() {
    let vectors: [(&[u8], &str); 7] = [
        (b"", ""),
        (b"f", "Zg=="),
        (b"fo", "Zm8="),
        (b"foo", "Zm9v"),
        (b"foob", "Zm9vYg=="),
        (b"fooba", "Zm9vYmE="),
        (b"foobar", "Zm9vYmFy"),
    ];
    for (bytes, text) in vectors {
        assert_eq!(base64_of(bytes), text);
    }
    assert_eq!(base64_of(&PNG_SIGNATURE), PNG);
    assert_eq!(base64_of(b"\xff\xd8\xff\xe0\x00\x10JFIF"), JPEG);
    assert_eq!(base64_of(b"<svg/>"), SVG);
    assert_eq!(base64_of(&[0; 124]), zeros_base64(124));
    assert_eq!(base64_of(&[0xFB, 0xFF, 0xBF]), "+/+/");
}

/// Bytes that begin with the signature of the type they are served as become an image block:
/// PNG's eight bytes, JPEG's three, either GIF version, and a RIFF container of form `WEBP`
/// whatever its four size bytes are.
#[tokio::test]
async fn an_image_whose_bytes_begin_with_its_types_signature_is_shown() -> TestResult {
    let cases: [(&str, &[u8]); 11] = [
        ("image/png", &PNG_SIGNATURE),
        ("image/png", b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR"),
        ("image/jpeg", b"\xff\xd8\xff"),
        ("image/jpeg", b"\xff\xd8\xff\xe0\x00\x10JFIF"),
        ("image/jpeg", b"\xff\xd8\xff\xdb\x00\x43"),
        ("image/gif", b"GIF87a"),
        ("image/gif", b"GIF89a\x01\x00\x01\x00"),
        ("image/webp", b"RIFF\x24\x00\x00\x00WEBP"),
        ("image/webp", b"RIFF\xff\xff\xff\xffWEBPVP8 "),
        // The type is matched as the inline rule matches it: by its essence, whatever its case.
        ("Image/PNG; charset=binary", &PNG_SIGNATURE),
        ("IMAGE/WEBP", b"RIFF\x00\x00\x00\x00WEBP"),
    ];
    for (served, bytes) in cases {
        check_served("images-signature-right.html", served, bytes, true).await?;
    }
    Ok(())
}

/// Bytes with a wrong signature are described, for each type: a signature wrong only in its
/// last byte, wrong after its first byte, (WebP) wrong in either half, and a right signature
/// that sits after other bytes instead of at the start.
#[tokio::test]
async fn an_image_whose_bytes_do_not_begin_with_its_types_signature_is_described() -> TestResult {
    let cases: [(&str, &[u8]); 25] = [
        ("image/png", b"\x89PNG\r\n\x1a\x0b\x00\x00\x00\rIHDR"),
        ("image/png", b"\x89QNG\r\n\x1a\n"),
        ("image/png", b"\x89\x00\x00\x00\x00\x00\x00\x00"),
        ("image/png", &[0; 64]),
        ("image/jpeg", b"\xff\xd8\xfe\xe0\x00\x10JFIF"),
        ("image/jpeg", b"\xff\xd9\xff\xe0"),
        ("image/jpeg", b"\xff\x00\x00\x00"),
        ("image/gif", b"GIF88a\x01\x00\x01\x00"),
        ("image/gif", b"GIF89b\x01\x00\x01\x00"),
        ("image/gif", b"G\x00\x00\x00\x00\x00"),
        ("image/webp", b"RIFF\x24\x00\x00\x00WAVE"),
        ("image/webp", b"RIFF\x24\x00\x00\x00WEBQ"),
        ("image/webp", b"RIFX\x24\x00\x00\x00WEBP"),
        ("image/webp", b"R\x00\x00\x00\x24\x00\x00\x00WEBP"),
        // `WEBP` straight after `RIFF`, where the size belongs.
        ("image/webp", b"RIFFWEBP\x00\x00\x00\x00"),
        ("IMAGE/WEBP", b"RIFF\x24\x00\x00\x00WAVE"),
        ("Image/PNG; charset=binary", b"\x89PNG\r\n\x1a\x0b"),
        // The right signature, but not at the start: the bytes begin with something else.
        ("image/png", b"\x00\x89PNG\r\n\x1a\n"),
        ("image/jpeg", b"\x00\xff\xd8\xff\xe0"),
        ("image/jpeg", b"<html>\xff\xd8\xff"),
        ("image/gif", b" GIF89a\x01\x00\x01\x00"),
        ("image/gif", b"\x00GIF87a"),
        ("image/webp", b"\x00RIFF\x00\x00\x00\x00WEBP"),
        // `RIFF` one byte late, so that `WEBP` still sits where a RIFF file puts it.
        ("image/webp", b"\x00RIFF\x00\x00\x00WEBP"),
        // `RIFF` at the start, but `WEBP` one byte late.
        ("image/webp", b"RIFF\x24\x00\x00\x00\x00WEBP"),
    ];
    for (served, bytes) in cases {
        check_served("images-signature-wrong.html", served, bytes, false).await?;
    }
    Ok(())
}

/// The served type is read by its essence: spaces around it and a parameter after it do not
/// stop a PNG's bytes from being shown as an image.
#[tokio::test]
async fn a_served_type_with_spaces_and_a_parameter_is_read_by_its_essence() -> TestResult {
    check_served(
        "images-type-spaces.html",
        " image/png ; charset=binary ",
        &PNG_SIGNATURE,
        true,
    )
    .await
}

/// Bytes that end before the signature does are described: one byte short for each type, and
/// no bytes at all.
#[tokio::test]
async fn bytes_shorter_than_a_signature_are_described() -> TestResult {
    let cases: [(&str, &[u8]); 9] = [
        ("image/png", b"\x89PNG\r\n\x1a"),
        ("image/jpeg", b"\xff\xd8"),
        ("image/gif", b"GIF89"),
        ("image/webp", b"RIFF\x24\x00\x00\x00WEB"),
        ("image/webp", b"RIFF"),
        ("image/png", b""),
        ("image/jpeg", b""),
        ("image/gif", b""),
        ("image/webp", b""),
    ];
    for (served, bytes) in cases {
        check_served("images-signature-short.html", served, bytes, false).await?;
    }
    Ok(())
}

/// The type is never guessed from the bytes: a real image of one raster type served as another
/// is described under the type it claims, not shown as the type it is.
#[tokio::test]
async fn an_image_of_one_type_served_as_another_is_described_not_shown_as_what_it_is() -> TestResult
{
    let cases: [(&str, &[u8]); 8] = [
        ("image/jpeg", &PNG_SIGNATURE),
        ("image/gif", &PNG_SIGNATURE),
        ("image/webp", &PNG_SIGNATURE),
        ("image/png", b"\xff\xd8\xff\xe0\x00\x10JFIF"),
        ("image/png", b"GIF89a\x01\x00\x01\x00"),
        ("image/png", b"RIFF\x24\x00\x00\x00WEBP"),
        ("image/webp", b"GIF89a\x01\x00\x01\x00"),
        ("image/gif", b"RIFF\x24\x00\x00\x00WEBP"),
    ];
    for (served, bytes) in cases {
        check_served("images-signature-other.html", served, bytes, false).await?;
    }
    Ok(())
}

/// The read declares a PNG and the application serves the bytes as `image/jpeg`: they are a
/// PNG, so they are not a JPEG, and no block is made, of either type.
#[tokio::test]
async fn a_png_served_as_jpeg_is_described() -> TestResult {
    let world = reader();
    let image = digest_of('c');
    world.application.answer_with(
        "read_item",
        image_read_response(
            WORKSPACE,
            "multimodal",
            &[media(&image, "image/png", Some("Chart"), &on_page(4))],
        ),
    )?;
    world.application.serve_object(
        &image,
        Ok(object_block(&image, "image/jpeg", PNG, 8, false)),
    )?;
    let result = show(
        &world,
        "images-png-as-jpeg.html",
        image_read_input("multimodal", 1024, 1),
    )
    .await?;
    assert_eq!(result.content.len(), 1, "{result:?}");
    assert_eq!(
        text_of(&result)?,
        described(&[format!(
            "[Image not inlined: picture \"Chart\", page 4, object {image}: \
             the content is not the type it claims (image/jpeg)]"
        )])
    );
    Ok(())
}

/// The type string says PNG, declared and served, and the bytes are an SVG document: the
/// raster rule on the type strings passes, and the bytes do not.
#[tokio::test]
async fn an_svg_served_as_png_is_described() -> TestResult {
    let world = reader();
    let image = digest_of('c');
    let document = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>";
    world.application.answer_with(
        "read_item",
        image_read_response(
            WORKSPACE,
            "multimodal",
            &[media(&image, "image/png", Some("Logo"), &on_page(1))],
        ),
    )?;
    world.application.serve_object(
        &image,
        Ok(object_block(
            &image,
            "image/png",
            &base64_of(document),
            document.len(),
            false,
        )),
    )?;
    let result = show(
        &world,
        "images-svg-as-png.html",
        image_read_input("multimodal", 1024, 1),
    )
    .await?;
    assert_eq!(result.content.len(), 1, "{result:?}");
    assert_eq!(
        text_of(&result)?,
        described(&[format!(
            "[Image not inlined: picture \"Logo\", page 1, object {image}: \
             the content is not the type it claims (image/png)]"
        )])
    );
    Ok(())
}

/// An image that is not its type takes nothing from the image budget: the next fetch asks for
/// all that was left before it, and an image that fits is still inlined.
#[tokio::test]
async fn an_image_that_is_not_its_type_takes_nothing_from_the_image_budget() -> TestResult {
    let world = reader();
    let (wrong, large, small) = (digest_of('c'), digest_of('d'), digest_of('e'));
    world.application.answer_with(
        "read_item",
        image_read_response(
            WORKSPACE,
            "multimodal",
            &[
                media(&wrong, "image/png", None, &on_page(1)),
                media(&large, "image/png", None, &on_page(2)),
                media(&small, "image/png", None, &on_page(3)),
            ],
        ),
    )?;
    world.application.serve_object(
        &wrong,
        Ok(object_block(
            &wrong,
            "image/png",
            &zeros_base64(500),
            500,
            false,
        )),
    )?;
    let whole = png_base64(1000);
    world.application.serve_object(
        &large,
        Ok(object_block(&large, "image/png", &whole, 1000, false)),
    )?;
    world
        .application
        .serve_object(&small, Ok(object_block(&small, "image/png", PNG, 8, false)))?;
    let result = show(
        &world,
        "images-signature-budget.html",
        image_read_input("multimodal", 1024, 3),
    )
    .await?;
    let images = images_of(&result)?;
    assert_eq!(
        images
            .iter()
            .map(|block| block.data.as_str())
            .collect::<Vec<_>>(),
        vec![whole.as_str(), PNG]
    );
    assert_eq!(
        text_of(&result)?,
        described(&[format!(
            "[Image not inlined: picture, page 1, object {wrong}: \
             the content is not the type it claims (image/png)]"
        )])
    );
    assert_eq!(
        world.application.requests("get_object")?,
        vec![
            object_request(WORKSPACE, &wrong, 1024),
            object_request(WORKSPACE, &large, 1024),
            object_request(WORKSPACE, &small, 24),
        ]
    );
    Ok(())
}

/// Each fetch runs on the reader's own route (the review's mutant O1). The read cites a
/// workspace dispatch refuses, so every fetch's refusal is recorded; its actor must be the
/// reader on the reader's own route, never a copy of the reader on a route of the server's
/// choosing. The fake access decides by subject and workspace only, so the recorded actor is
/// what tells the routes apart.
#[tokio::test]
async fn each_fetch_is_dispatched_on_the_reads_own_route() -> TestResult {
    let world = World::new(
        FakeAccess::default()
            .granting("alice", &[Permission::Read])
            .refusing(OTHER_WORKSPACE),
    );
    let (first, second) = (digest_of('c'), digest_of('d'));
    world.application.answer_with(
        "read_item",
        image_read_response(
            OTHER_WORKSPACE,
            "multimodal",
            &[
                media(&first, "image/png", None, &on_page(1)),
                media(&second, "image/png", None, &on_page(2)),
            ],
        ),
    )?;
    for image in [&first, &second] {
        world
            .application
            .serve_object(image, Ok(object_block(image, "image/png", PNG, 8, false)))?;
    }
    let result = show(
        &world,
        "images-route-refused.html",
        image_read_input("multimodal", 1024, 2),
    )
    .await?;
    assert_eq!(images_of(&result)?.len(), 0);
    let appended = world.events.appended()?;
    assert_eq!(appended.len(), 2, "one refusal for each fetch");
    for (_, event) in &appended {
        assert_eq!(event.kind, EventKind::PermissionDenied);
        assert_eq!(event.operation, Some(OperationName::GetObject));
        let actor = some(event.actor.as_ref(), "the refused actor")?;
        assert_eq!(
            (
                actor.subject.as_str(),
                &actor.route,
                actor.client_id.as_deref()
            ),
            ("alice", &AccessRoute::McpDelegation, Some("client-test"))
        );
    }
    Ok(())
}

/// A served fetch reaches the application with the read's whole principal: the subject, the
/// tenant, the route, the client and the delegation ceiling the read itself ran with.
#[tokio::test]
async fn each_served_fetch_carries_the_reads_whole_principal() -> TestResult {
    let world = reader();
    let mut alice = agent("alice")?;
    alice.delegation = Some(DelegationCeiling {
        permissions: vec![Permission::Read],
        workspace_ids: None,
    });
    let (first, second) = (digest_of('c'), digest_of('d'));
    world.application.answer_with(
        "read_item",
        image_read_response(
            WORKSPACE,
            "multimodal",
            &[
                media(&first, "image/png", None, &on_page(1)),
                media(&second, "image/png", None, &on_page(2)),
            ],
        ),
    )?;
    for image in [&first, &second] {
        world
            .application
            .serve_object(image, Ok(object_block(image, "image/png", PNG, 8, false)))?;
    }
    let server = McpServer::new(
        world.ports.clone(),
        bundle("images-route-served.html", "<p>app</p>")?,
    )?;
    let result = server
        .call_tool_as(
            Some(&alice),
            "show",
            Some(arguments(image_read_input("multimodal", 1024, 2))?),
        )
        .await?;
    assert_eq!(result.is_error, Some(false), "{result:?}");
    assert_eq!(images_of(&result)?.len(), 2);
    let expected = serde_json::to_value(&alice)?;
    assert_eq!(expected.get("route"), Some(&json!("mcp_delegation")));
    assert_eq!(
        world.application.callers("read_item")?,
        vec![expected.clone()]
    );
    assert_eq!(
        world.application.callers("get_object")?,
        vec![expected.clone(), expected]
    );
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
