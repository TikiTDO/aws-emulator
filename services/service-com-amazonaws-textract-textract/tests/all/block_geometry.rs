// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the rule that a block's `Geometry` and its `Confidence` are
//! MEASUREMENTS of the part of the submitted document that block was read from — never a
//! page template.
//!
//! The rule, over a table of documents rather than one recorded page:
//!
//! * a `LINE`/`WORD` block's `BoundingBox` is where that text sits in the document and as
//!   big as it is there (for a raster: the extent of the ink it was recognised from, which
//!   this suite knows because it DREW the page), its `Polygon` is inscribed in that box and
//!   touches each of its edges, and a `PAGE` block's box ENCLOSES the blocks of its page
//!   rather than claiming the whole page;
//! * so the geometry varies with the document: between the lines of one page, between the
//!   words of one line, and not at all between two renderings of the same page at two
//!   sizes (the boxes are page-relative fractions);
//! * a block's `Confidence` is the confidence of the characters IT was read from: the
//!   mean score of its own glyphs, so a `LINE` reports the character-weighted mean of its
//!   words.
//!
//! Both synchronous read paths are driven here (`DetectDocumentText` and
//! `AnalyzeDocument`), because they answer over one document model and must report one
//! geometry for it.
//!
//! Every raster page is drawn in this test from known text at known pixel positions with
//! the same public-domain 8x8 font the recogniser holds templates for, so the assertion
//! compares the geometry the service reports with WHERE THE INK WAS PUT, not with a
//! recorded answer.

use std::collections::BTreeSet;

use image::ImageFormat;
use serde_json::{json, Value};

use crate::document_extraction::{
    assert_block_graph, block_id, block_type, blocks_of, call, harness, of_type, text_document,
};
use crate::raster_documents::{raster_document, render, Paper, CELL, PITCH};

/// A `BoundingBox` as it came back over the wire.
///
/// `pub(crate)` here and on the two readers below: `crate::block_outlines` asserts the
/// polygon half of the same `Geometry` and reads it through these rather than growing a
/// second copy of them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub(crate) left: f64,
    pub(crate) top: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

impl Rect {
    pub(crate) fn right(&self) -> f64 {
        self.left + self.width
    }

    pub(crate) fn bottom(&self) -> f64 {
        self.top + self.height
    }

    /// Whether this box lies inside `outer`, with `tol` of slack on every side.
    fn within(&self, outer: &Rect, tol: f64) -> bool {
        self.left >= outer.left - tol
            && self.top >= outer.top - tol
            && self.right() <= outer.right() + tol
            && self.bottom() <= outer.bottom() + tol
    }
}

fn number(value: &Value, member: &str) -> f64 {
    value.get(member).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

pub(crate) fn bbox_of(case: &str, block: &Value) -> Rect {
    let bbox = block
        .get("Geometry")
        .and_then(|geometry| geometry.get("BoundingBox"))
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: block {} carries no BoundingBox, got {block}",
                block_id(block)
            )
        });
    Rect {
        left: number(bbox, "Left"),
        top: number(bbox, "Top"),
        width: number(bbox, "Width"),
        height: number(bbox, "Height"),
    }
}

pub(crate) fn polygon_of(case: &str, block: &Value) -> Vec<(f64, f64)> {
    block
        .get("Geometry")
        .and_then(|geometry| geometry.get("Polygon"))
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: block {} carries no Polygon, got {block}",
                block_id(block)
            )
        })
        .iter()
        .map(|point| (number(point, "X"), number(point, "Y")))
        .collect()
}

fn text_of(block: &Value) -> String {
    block
        .get("Text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn confidence_of(block: &Value) -> Option<f64> {
    block.get("Confidence").and_then(Value::as_f64)
}

/// The blocks a block names as its children, in the order it names them.
fn children<'a>(blocks: &'a [Value], parent: &Value) -> Vec<&'a Value> {
    parent
        .get("Relationships")
        .and_then(Value::as_array)
        .map(|relationships| {
            relationships
                .iter()
                .filter(|relationship| {
                    relationship.get("Type").and_then(Value::as_str) == Some("CHILD")
                })
                .flat_map(|relationship| {
                    relationship
                        .get("Ids")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|id| {
                    let id = id.as_str().unwrap_or_default().to_string();
                    blocks.iter().find(|block| block_id(block) == id)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn union(rects: &[Rect]) -> Rect {
    let left = rects.iter().map(|rect| rect.left).fold(f64::MAX, f64::min);
    let top = rects.iter().map(|rect| rect.top).fold(f64::MAX, f64::min);
    let right = rects.iter().map(Rect::right).fold(f64::MIN, f64::max);
    let bottom = rects.iter().map(Rect::bottom).fold(f64::MIN, f64::max);
    Rect {
        left,
        top,
        width: right - left,
        height: bottom - top,
    }
}

/// The character columns of each whitespace-separated word of a line: its first column,
/// its last column, and its text.
fn word_columns(line: &str) -> Vec<(usize, usize, String)> {
    let mut out: Vec<(usize, usize, String)> = Vec::new();
    for (column, ch) in line.chars().enumerate() {
        match (ch.is_whitespace(), out.last_mut()) {
            (false, Some(word)) if word.1 + 1 == column => {
                word.1 = column;
                word.2.push(ch);
            }
            (false, _) => out.push((column, column, ch.to_string())),
            (true, _) => {}
        }
    }
    out
}

/// Where the renderer DREW characters `first..=last` of row `row`, as a fraction of the
/// page it drew them on.
///
/// The renderer puts character `c` of row `r` in the cell
/// `margin + c * CELL * scale` by `margin + r * PITCH * scale`, `CELL * scale` pixels on
/// a side, so this is the rectangle the word's ink must be found inside.
fn drawn(paper: Paper, width: u32, height: u32, row: usize, first: usize, last: usize) -> Rect {
    let page_width = f64::from(width);
    let page_height = f64::from(height);
    let left = (paper.margin + first * CELL * paper.scale) as f64 / page_width;
    let right = (paper.margin + (last + 1) * CELL * paper.scale) as f64 / page_width;
    let top = (paper.margin + row * PITCH * paper.scale) as f64 / page_height;
    let bottom = (paper.margin + (row * PITCH + CELL) * paper.scale) as f64 / page_height;
    Rect {
        left,
        top,
        width: right - left,
        height: bottom - top,
    }
}

/// Assert that every block whose children are WORDs spans exactly the words it was read
/// from — its box encloses them and nothing more — and reports the confidence of their
/// characters. Returns the block types it checked, and records every word confidence it
/// saw in `seen`.
///
/// One helper for both documents a block can come from: the rule is the same whether the
/// words' boxes were measured off pixels or laid out from characters.
fn assert_spans_its_words(
    case: &str,
    blocks: &[Value],
    tol: f64,
    seen: &mut BTreeSet<String>,
) -> BTreeSet<String> {
    let mut checked = BTreeSet::new();
    for parent in blocks {
        let words = children(blocks, parent);
        if words.is_empty() || words.iter().any(|word| block_type(word) != "WORD") {
            continue;
        }
        let mut total = 0.0f64;
        let mut characters = 0usize;
        let mut boxes = Vec::new();
        for word in &words {
            let text = text_of(word);
            let confidence = confidence_of(word).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: WORD {text:?} must report the confidence it was read \
                     with, got {word}"
                )
            });
            seen.insert(format!("{confidence:.4}"));
            total += confidence * text.chars().count() as f64;
            characters += text.chars().count();
            boxes.push(bbox_of(case, word));
        }
        checked.insert(block_type(parent).to_string());

        let expected = total / characters.max(1) as f64;
        let reported = confidence_of(parent).unwrap_or_default();
        assert!(
            (reported - expected).abs() < 0.05,
            "case {case:?}: {} block {} must report the confidence of the characters it was \
             read from ({expected}), got {reported}",
            block_type(parent),
            block_id(parent)
        );

        let spanned = union(&boxes);
        let reported = bbox_of(case, parent);
        for (member, expected, got) in [
            ("Left", spanned.left, reported.left),
            ("Top", spanned.top, reported.top),
            ("Width", spanned.width, reported.width),
            ("Height", spanned.height, reported.height),
        ] {
            assert!(
                (expected - got).abs() < tol,
                "case {case:?}: {} block {} must span exactly the words it was read from, so \
                 BoundingBox.{member} is {expected}, got {got}",
                block_type(parent),
                block_id(parent)
            );
        }
    }
    checked
}

/// One row of the geometry table: a page, and the read path it is read through.
struct GeometryCase {
    name: &'static str,
    lines: Vec<&'static str>,
    paper: Paper,
    format: ImageFormat,
    operation: &'static str,
    /// The `FeatureTypes` of an `AnalyzeDocument` row; empty for text detection.
    features: Vec<&'static str>,
}

/// Mixed case, digits, a labelled field and punctuation: more than one glyph class, and
/// words of several lengths so one line's words cannot share a box.
fn sample_lines() -> Vec<&'static str> {
    vec![
        "SIMPARITY LEDGER",
        "Account Number: 48213",
        "Total Due 1250.00",
    ]
}

fn geometry_cases() -> Vec<GeometryCase> {
    vec![
        GeometryCase {
            name: "a PNG at twice the font's size",
            lines: sample_lines(),
            paper: Paper {
                scale: 2,
                margin: 6,
                ..Paper::plain()
            },
            format: ImageFormat::Png,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        GeometryCase {
            name: "a PNG whose text sits far from the page's corner",
            lines: sample_lines(),
            paper: Paper {
                scale: 5,
                margin: 140,
                ..Paper::plain()
            },
            format: ImageFormat::Png,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        GeometryCase {
            name: "a JPEG of six short lines",
            lines: vec!["One", "Two", "Three", "Four", "Five", "Six"],
            paper: Paper {
                scale: 3,
                margin: 10,
                ..Paper::plain()
            },
            // `Document$Bytes` carries "PNG or JPEG format"; a TIFF is a document only an
            // Amazon S3 object may hold, and `crate::s3_sourced_documents` reads one.
            format: ImageFormat::Jpeg,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        GeometryCase {
            name: "light ink on a dark page, read as one line",
            lines: vec!["SOLO 1250.00"],
            paper: Paper {
                scale: 4,
                margin: 9,
                background: 12,
                ink: 243,
                ..Paper::plain()
            },
            format: ImageFormat::Png,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        GeometryCase {
            name: "the analysis path over the same kind of page",
            lines: sample_lines(),
            paper: Paper {
                scale: 4,
                margin: 20,
                ..Paper::plain()
            },
            format: ImageFormat::Png,
            operation: "AnalyzeDocument",
            features: vec!["FORMS", "TABLES"],
        },
        GeometryCase {
            name: "the analysis path over a page whose columns make a table",
            lines: vec![
                "Item        Qty",
                "Widget        12",
                "Gadget         7",
            ],
            paper: Paper {
                scale: 3,
                margin: 8,
                ..Paper::plain()
            },
            format: ImageFormat::Png,
            operation: "AnalyzeDocument",
            features: vec!["TABLES"],
        },
        GeometryCase {
            name: "the analysis path over a JPEG, whose coding is lossy",
            lines: sample_lines(),
            paper: Paper {
                scale: 6,
                margin: 12,
                ..Paper::plain()
            },
            format: ImageFormat::Jpeg,
            operation: "AnalyzeDocument",
            features: vec!["FORMS"],
        },
    ]
}

fn request_for(case: &GeometryCase, document: &Value) -> Value {
    if case.features.is_empty() {
        json!({ "Document": document })
    } else {
        json!({ "Document": document, "FeatureTypes": case.features })
    }
}

#[tokio::test]
async fn a_recognised_pages_geometry_is_measured_where_the_ink_was_drawn() {
    let router = harness();

    for case in geometry_cases() {
        let page = render(&case.lines, case.paper);
        let (width, height) = (page.width(), page.height());
        let document = raster_document(&page, case.format);
        let (status, body) = call(&router, case.operation, &request_for(&case, &document)).await;
        assert_eq!(
            status, 200,
            "case {:?}: a readable raster must answer 200, got {status} with {body}",
            case.name
        );
        let blocks = blocks_of(&body);
        assert_block_graph(case.name, &body, &blocks, None);

        // Two pixels of slack per font pixel: a lossy coding and a binarisation both
        // decide an edge pixel, and the assertion is about WHERE the ink is, not about
        // which of its edge pixels was called ink.
        let tol = 2.0 * case.paper.scale as f64 / f64::from(width.min(height));

        let lines = of_type(&blocks, "LINE");
        assert_eq!(
            lines.len(),
            case.lines.len(),
            "case {:?}: expected one LINE per line drawn, got {:?}",
            case.name,
            lines.iter().map(|line| text_of(line)).collect::<Vec<_>>()
        );

        let page_box = bbox_of(case.name, of_type(&blocks, "PAGE")[0]);
        let mut line_boxes: Vec<Rect> = Vec::new();

        for (row, line) in lines.iter().enumerate() {
            let drawn_line = drawn(
                case.paper,
                width,
                height,
                row,
                0,
                case.lines[row].chars().count().saturating_sub(1),
            );
            let line_box = bbox_of(case.name, line);
            line_boxes.push(line_box);
            assert!(
                line_box.within(&drawn_line, tol),
                "case {:?}: LINE {row} was drawn at {drawn_line:?}, but reports {line_box:?}",
                case.name
            );
            assert!(
                line_box.width >= 0.3 * drawn_line.width && line_box.height >= 0.3 * drawn_line.height,
                "case {:?}: LINE {row} must cover the ink it was read from (drawn \
                 {drawn_line:?}), got {line_box:?}",
                case.name
            );

            let words = children(&blocks, line);
            let drawn_words = word_columns(case.lines[row]);
            assert_eq!(
                words.iter().map(|word| text_of(word)).collect::<Vec<_>>(),
                drawn_words
                    .iter()
                    .map(|(_, _, text)| text.clone())
                    .collect::<Vec<_>>(),
                "case {:?}: LINE {row} must report the words that were drawn on it",
                case.name
            );
            for (word, (first, last, text)) in words.iter().zip(drawn_words.iter()) {
                let drawn_word = drawn(case.paper, width, height, row, *first, *last);
                let word_box = bbox_of(case.name, word);
                assert!(
                    word_box.within(&drawn_word, tol),
                    "case {:?}: WORD {text:?} was drawn at {drawn_word:?}, but reports \
                     {word_box:?}",
                    case.name
                );
                assert!(
                    word_box.width >= 0.3 * drawn_word.width,
                    "case {:?}: WORD {text:?} must cover the ink it was read from (drawn \
                     {drawn_word:?}), got {word_box:?}",
                    case.name
                );
                assert!(
                    word_box.within(&line_box, tol),
                    "case {:?}: WORD {text:?} at {word_box:?} must sit inside its LINE \
                     {line_box:?}",
                    case.name
                );
            }

            // A word of clearly more characters covers clearly more of the page: the
            // boxes are measurements, not one size reported for every word.
            for (left, (_, _, left_text)) in words.iter().zip(drawn_words.iter()) {
                for (right, (_, _, right_text)) in words.iter().zip(drawn_words.iter()) {
                    if right_text.chars().count() < left_text.chars().count() + 2 {
                        continue;
                    }
                    assert!(
                        bbox_of(case.name, right).width > bbox_of(case.name, left).width,
                        "case {:?}: WORD {right_text:?} is longer than {left_text:?}, so it \
                         must cover more of the page, got {:?} against {:?}",
                        case.name,
                        bbox_of(case.name, right),
                        bbox_of(case.name, left)
                    );
                }
            }
        }

        // Lines down the page: each one lower than the one before, and no two sharing a
        // box.
        for pair in line_boxes.windows(2) {
            assert!(
                pair[1].top > pair[0].top,
                "case {:?}: the lines of a page must be reported down it, got {:?} then {:?}",
                case.name,
                pair[0],
                pair[1]
            );
        }

        // The PAGE bounds what was read on the page.
        let enclosing = union(&line_boxes);
        assert!(
            (page_box.left - enclosing.left).abs() < tol
                && (page_box.top - enclosing.top).abs() < tol
                && (page_box.right() - enclosing.right()).abs() < tol
                && (page_box.bottom() - enclosing.bottom()).abs() < tol,
            "case {:?}: the PAGE box must enclose the page's blocks ({enclosing:?}), got \
             {page_box:?}",
            case.name
        );
        assert!(
            page_box.left > 0.0
                && page_box.top > 0.0
                && page_box.right() < 1.0
                && page_box.bottom() < 1.0,
            "case {:?}: the text was drawn inside a margin, so the PAGE box must sit inside \
             the page rather than claim all of it, got {page_box:?}",
            case.name
        );
        for block in &blocks {
            if block_type(block) == "PAGE" {
                continue;
            }
            let block_box = bbox_of(case.name, block);
            assert!(
                block_box.within(&page_box, tol),
                "case {:?}: {} block {} at {block_box:?} must sit inside its PAGE \
                 {page_box:?}",
                case.name,
                block_type(block),
                block_id(block)
            );
        }

        // Every Polygon is the fine-grained outline of the marks its own box coarsely
        // bounds: four points inside that box, together touching each of its edges — so no
        // block reports the page's corners for a region inside the page. That assertion,
        // and the exact outline a drawn page must report, live in `crate::block_outlines`.
        for block in &blocks {
            crate::block_outlines::assert_inscribed(case.name, block);
            let polygon = polygon_of(case.name, block);
            assert!(
                polygon
                    .iter()
                    .any(|(x, y)| (*x > 0.0 && *x < 1.0) || (*y > 0.0 && *y < 1.0)),
                "case {:?}: block {} Polygon must name the interior points it was measured \
                 at, got {polygon:?}",
                case.name,
                block_id(block)
            );
        }
    }
}

#[tokio::test]
async fn the_same_page_drawn_larger_reports_the_same_geometry() {
    let router = harness();

    // `BoundingBox` and `Point` are page-relative fractions, so doubling every pixel of
    // the page — the text, the margins and the page itself — must not move a block: the
    // geometry describes the document's layout, not the raster's resolution.
    for scale in [2usize, 3, 4] {
        let case = format!("scale {scale} against scale {}", 2 * scale);
        let small = render(
            &sample_lines(),
            Paper {
                scale,
                margin: 4 * scale,
                ..Paper::plain()
            },
        );
        let large = render(
            &sample_lines(),
            Paper {
                scale: 2 * scale,
                margin: 8 * scale,
                ..Paper::plain()
            },
        );
        let mut answers = Vec::new();
        for page in [&small, &large] {
            let request = json!({ "Document": raster_document(page, ImageFormat::Png) });
            let (status, body) = call(&router, "DetectDocumentText", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: both renderings must be read, got {status} with {body}"
            );
            answers.push(blocks_of(&body));
        }
        let (small_blocks, large_blocks) = (&answers[0], &answers[1]);
        assert_eq!(
            small_blocks.len(),
            large_blocks.len(),
            "case {case:?}: the same page must answer with the same blocks, got {} and {}",
            small_blocks.len(),
            large_blocks.len()
        );
        for (a, b) in small_blocks.iter().zip(large_blocks.iter()) {
            // The same blocks over the same text. Not the same CHARACTERS necessarily:
            // resampling the ink can make a recogniser read one glyph as another, which
            // is a property of recognition, while where that ink sits on the page is not.
            assert_eq!(
                (block_type(a), text_of(a).chars().count()),
                (block_type(b), text_of(b).chars().count()),
                "case {case:?}: the same page must be read as the same blocks, got {:?} and \
                 {:?}",
                text_of(a),
                text_of(b)
            );
            let (first, second) = (bbox_of(&case, a), bbox_of(&case, b));
            for (member, left, right) in [
                ("Left", first.left, second.left),
                ("Top", first.top, second.top),
                ("Width", first.width, second.width),
                ("Height", first.height, second.height),
            ] {
                assert!(
                    (left - right).abs() < 0.02,
                    "case {case:?}: {} block {:?} BoundingBox.{member} must not depend on the \
                     page's resolution, got {left} and {right}",
                    block_type(a),
                    text_of(a)
                );
            }
        }
    }
}

#[tokio::test]
async fn a_blocks_confidence_is_measured_over_the_characters_it_was_read_from() {
    let router = harness();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut checked: BTreeSet<String> = BTreeSet::new();

    for case in geometry_cases() {
        let page = render(&case.lines, case.paper);
        let document = raster_document(&page, case.format);
        let (status, body) = call(&router, case.operation, &request_for(&case, &document)).await;
        assert_eq!(
            status, 200,
            "case {:?}: a readable raster must answer 200, got {status} with {body}",
            case.name
        );
        let blocks = blocks_of(&body);
        checked.extend(assert_spans_its_words(case.name, &blocks, 1e-6, &mut seen));
    }

    // Not one value for every block of every page: a confidence a recogniser measured
    // over different ink differs, which is what makes a confidence threshold mean
    // anything.
    assert!(
        seen.len() > 1,
        "a recognised WORD must report its own measured confidence, got the single value \
         {seen:?}"
    );
    // The rule was really exercised beyond the LINE blocks: the analysis features read
    // the same words into blocks of their own.
    for kind in ["LINE", "KEY_VALUE_SET"] {
        assert!(
            checked.contains(kind),
            "the table must exercise the confidence rule on a {kind} block, checked \
             {checked:?}"
        );
    }
}

/// Lines of several lengths and labels, and a tab-separated table whose cells hold more
/// than one word: a document whose every line is DRAWN onto the page, so its blocks are
/// laid out where its own ink is.
const TEXT_DOCUMENT: &str = "Statement of Account\n\
                             Invoice Number: INV-7781\n\
                             Prepared for: Simparity Logistics Incorporated\n\
                             Item Code\tQty\tUnit Price\n\
                             Widget Mk2\t12\t1250.00\n\
                             Total Due: 1250.00\n";

#[tokio::test]
async fn a_documents_blocks_are_laid_out_from_the_text_it_holds() {
    let router = harness();
    let document = text_document(TEXT_DOCUMENT);
    let drawn = crate::document_extraction::drawn_lines(TEXT_DOCUMENT);
    let drawn_lines: Vec<&str> = drawn.iter().map(String::as_str).collect();

    for (case, request) in [
        (
            "DetectDocumentText",
            json!({ "Document": document.clone() }),
        ),
        (
            "AnalyzeDocument FORMS TABLES",
            json!({ "Document": document.clone(), "FeatureTypes": ["FORMS", "TABLES"] }),
        ),
    ] {
        let operation = case.split_whitespace().next().unwrap_or_default();
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a readable document must answer 200, got {status} with {body}"
        );
        let blocks = blocks_of(&body);
        // Each block reports the confidence measured over its own characters, so only
        // `Percent`'s range holds for all of them.
        assert_block_graph(case, &body, &blocks, None);

        // Every block the document's words were read into spans exactly those words.
        let checked = assert_spans_its_words(case, &blocks, 1e-6, &mut BTreeSet::new());
        let expected_kinds: &[&str] = if case.contains("TABLES") {
            &["LINE", "KEY_VALUE_SET", "CELL"]
        } else {
            &["LINE"]
        };
        for kind in expected_kinds {
            assert!(
                checked.contains(*kind),
                "case {case:?}: the rule must be exercised on a {kind} block, checked \
                 {checked:?}"
            );
        }

        let lines = of_type(&blocks, "LINE");
        assert_eq!(
            lines.len(),
            drawn_lines.len(),
            "case {case:?}: expected one LINE per line of the document, got {:?}",
            lines.iter().map(|line| text_of(line)).collect::<Vec<_>>()
        );
        let line_boxes: Vec<Rect> = lines.iter().map(|line| bbox_of(case, line)).collect();
        for pair in line_boxes.windows(2) {
            assert!(
                pair[1].top > pair[0].top,
                "case {case:?}: the lines of a page must be reported down it, got {:?} then \
                 {:?}",
                pair[0],
                pair[1]
            );
        }

        // A line's box is as wide as its own text: the longest line of the document
        // covers the most of the page, the shortest the least.
        let widest = drawn_lines
            .iter()
            .enumerate()
            .max_by_key(|(_, line)| line.chars().count())
            .map(|(index, _)| index)
            .unwrap_or_default();
        let narrowest = drawn_lines
            .iter()
            .enumerate()
            .min_by_key(|(_, line)| line.chars().count())
            .map(|(index, _)| index)
            .unwrap_or_default();
        assert!(
            line_boxes[widest].width > line_boxes[narrowest].width,
            "case {case:?}: LINE {:?} is longer than {:?}, so it must cover more of the \
             page, got {:?} against {:?}",
            drawn_lines[widest],
            drawn_lines[narrowest],
            line_boxes[widest],
            line_boxes[narrowest]
        );

        // Words across a line: each one further right than the one before, and sized by
        // its own characters.
        for (index, line) in lines.iter().enumerate() {
            let words = children(&blocks, line);
            let drawn_words = word_columns(drawn_lines[index]);
            assert_eq!(
                words.iter().map(|word| text_of(word)).collect::<Vec<_>>(),
                drawn_words
                    .iter()
                    .map(|(_, _, text)| text.clone())
                    .collect::<Vec<_>>(),
                "case {case:?}: LINE {index} must report the words it holds"
            );
            let mut previous: Option<Rect> = None;
            for (word, (_, _, text)) in words.iter().zip(drawn_words.iter()) {
                let word_box = bbox_of(case, word);
                assert!(
                    word_box.within(&bbox_of(case, line), 1e-6),
                    "case {case:?}: WORD {text:?} at {word_box:?} must sit inside its LINE"
                );
                if let Some(before) = previous {
                    assert!(
                        word_box.left > before.left,
                        "case {case:?}: WORD {text:?} comes later in the line, so it must be \
                         reported further right, got {word_box:?} after {before:?}"
                    );
                }
                previous = Some(word_box);
            }
            // Two words of different lengths on one line never share a width.
            for (left, (_, _, left_text)) in words.iter().zip(drawn_words.iter()) {
                for (right, (_, _, right_text)) in words.iter().zip(drawn_words.iter()) {
                    if right_text.chars().count() <= left_text.chars().count() {
                        continue;
                    }
                    assert!(
                        bbox_of(case, right).width > bbox_of(case, left).width,
                        "case {case:?}: WORD {right_text:?} is longer than {left_text:?}, so \
                         it must cover more of the page, got {:?} against {:?}",
                        bbox_of(case, right),
                        bbox_of(case, left)
                    );
                }
            }
        }

        // The PAGE bounds the text that was read, not the whole page.
        let page_box = bbox_of(case, of_type(&blocks, "PAGE")[0]);
        let enclosing = union(&line_boxes);
        assert!(
            (page_box.left - enclosing.left).abs() < 1e-6
                && (page_box.top - enclosing.top).abs() < 1e-6
                && (page_box.right() - enclosing.right()).abs() < 1e-6
                && (page_box.bottom() - enclosing.bottom()).abs() < 1e-6,
            "case {case:?}: the PAGE box must enclose the page's blocks ({enclosing:?}), got \
             {page_box:?}"
        );
        assert!(
            page_box.left > 0.0
                && page_box.top > 0.0
                && page_box.right() < 1.0
                && page_box.bottom() < 1.0,
            "case {case:?}: the PAGE box must sit inside the page rather than claim all of \
             it, got {page_box:?}"
        );
    }
}

/// One row of the table-cell geometry table: the row as the document holds it.
struct TableRowCase {
    name: &'static str,
    row: &'static str,
}

/// The `CELL` blocks of one row of the page's one table, in column order.
fn row_cells<'a>(case: &str, blocks: &'a [Value], row_index: i64) -> Vec<&'a Value> {
    let mut cells: Vec<&Value> = of_type(blocks, "CELL")
        .into_iter()
        .filter(|cell| cell.get("RowIndex").and_then(Value::as_i64) == Some(row_index))
        .collect();
    cells.sort_by_key(|cell| cell.get("ColumnIndex").and_then(Value::as_i64).unwrap_or(0));
    assert!(
        !cells.is_empty(),
        "case {case:?}: row {row_index} must report CELL blocks, got {blocks:?}"
    );
    cells
}

#[tokio::test]
async fn a_table_cells_box_spans_the_words_read_in_it() {
    let router = harness();

    // A cell's box is the extent of the words read IN it, which are the children it names,
    // and every cell of a row sits inside the TABLE the row belongs to. The rows differ
    // along the shape of the table: how many columns the row has, and how many words stand
    // in one cell.
    //
    // A cell holding NO words is not among them, and cannot be: a page shows a column
    // break as a gap in its ink, and two gaps running together are one gap, so no page
    // draws an empty cell between two populated ones.
    let cases = [
        TableRowCase {
            name: "two columns",
            row: "Widget Mk2\t1250.00",
        },
        TableRowCase {
            name: "three columns",
            row: "Widget Mk2\t12\t1250.00",
        },
        TableRowCase {
            name: "a cell of several words",
            row: "Widget Mk2 Blue\t1250.00 USD",
        },
    ];

    for case in &cases {
        let text = format!("Item Code\tQty\tUnit Price\n{}\n", case.row);
        let (status, body) = call(
            &router,
            "AnalyzeDocument",
            &json!({ "Document": text_document(&text), "FeatureTypes": ["TABLES"] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: a readable table must answer 200, got {status} with {body}",
            case.name
        );
        let blocks = blocks_of(&body);
        let cells = row_cells(case.name, &blocks, 2);
        assert_eq!(
            cells.len(),
            case.row.split('\t').count(),
            "case {:?}: expected one CELL per tab-separated cell of {:?}, got {:?}",
            case.name,
            case.row,
            cells
                .iter()
                .map(|cell| text_of(cell))
                .collect::<Vec<String>>()
        );

        let boxes: Vec<Rect> = cells.iter().map(|cell| bbox_of(case.name, cell)).collect();
        let table = of_type(&blocks, "TABLE");
        let table_box = bbox_of(case.name, table[0]);
        // Every cell of the row spans the words read IN it, which are the children it
        // names, and the table encloses every cell.
        for (index, cell) in cells.iter().enumerate() {
            let column = index + 1;
            let own = boxes[index];
            let words = children(&blocks, cell);
            assert!(
                !words.is_empty(),
                "case {:?}: the cell at column {column} must name the words read in it, got \
                 {cell}",
                case.name
            );
            let enclosing = union(
                &words
                    .iter()
                    .map(|word| bbox_of(case.name, word))
                    .collect::<Vec<Rect>>(),
            );
            assert!(
                (own.left - enclosing.left).abs() < 1e-6
                    && (own.right() - enclosing.right()).abs() < 1e-6,
                "case {:?}: the cell at column {column} must span its own words \
                 ({enclosing:?}), got {own:?}",
                case.name
            );
            // Cells are reported across the row: each one begins after the one before it.
            if index > 0 {
                assert!(
                    own.left > boxes[index - 1].left,
                    "case {:?}: the cell at column {column} comes later in the row, so it \
                     must be reported further right, got {own:?} after {:?}",
                    case.name,
                    boxes[index - 1]
                );
            }
        }
        for (index, own) in boxes.iter().enumerate() {
            assert!(
                own.within(&table_box, 1e-6),
                "case {:?}: the cell at column {} must sit inside its TABLE ({table_box:?}), \
                 got {own:?}",
                case.name,
                index + 1
            );
        }
    }
}
