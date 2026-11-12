// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the rule that a block's `Geometry$Polygon` is the FINE-GRAINED
//! outline of the marks its `BoundingBox` coarsely bounds, measured off the submitted
//! page.
//!
//! The model draws the distinction the rule follows: `BoundingBox` is "an axis-aligned
//! COARSE representation of the location of the recognized item", while `Polygon` is,
//! "within the bounding box, a FINE-GRAINED polygon around the recognized item". So a
//! polygon is taken from the marks themselves — its four points are the topmost, the
//! rightmost, the bottommost and the leftmost mark the block was read from — which puts it
//! inside that box, on each of the box's edges, and makes it follow the item's own shape
//! rather than restate the four corners of the rectangle around it. The points run
//! clockwise from the corner the item's characters were READ from, so a page submitted
//! turned reports the outline of each item in the frame it was read in.
//!
//! The rows of the table differ along what the rule generalises over: the size the text is
//! drawn at, the polarity of the ink, the stroke weight, whether a rule is drawn across
//! the page, the quarter turn the page is submitted at, how many lines and words it
//! carries, whether it carries a labelled line or a table, and which synchronous read path
//! it is read through.
//!
//! Every page here is DRAWN in this test, so the expected outline is computed from the
//! pixels that were put on the page — never from a recorded answer.

use image::{GrayImage, ImageFormat};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::block_geometry::{bbox_of, polygon_of};
use crate::document_extraction::{
    assert_block_graph, block_id, block_type, blocks_of, call, drawn_lines, harness, of_type,
};
use crate::raster_documents::{raster_document, render, Paper};

/// One row of the table: the text drawn on the page, how it is drawn, the quarter turns it
/// is submitted at, and the read path it is read through.
struct Case {
    name: &'static str,
    lines: Vec<String>,
    paper: Paper,
    /// Clockwise quarter turns applied to the drawn page before it is submitted.
    turns: u8,
    operation: &'static str,
    /// The `FeatureTypes` of an `AnalyzeDocument` row; empty for text detection.
    features: Vec<&'static str>,
}

fn sample_lines() -> Vec<String> {
    ["SIMPARITY LEDGER", "Account Number: 48213", "Total Due 1250.00"]
        .iter()
        .map(|line| (*line).to_string())
        .collect()
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a plain page of printed lines",
            lines: sample_lines(),
            paper: Paper::plain(),
            turns: 0,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "the same page drawn large",
            lines: sample_lines(),
            paper: Paper {
                scale: 4,
                margin: 24,
                ..Paper::plain()
            },
            turns: 0,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "light text on a dark ground",
            lines: sample_lines(),
            paper: Paper {
                background: 20,
                ink: 240,
                ..Paper::plain()
            },
            turns: 0,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "heavier strokes and a rule drawn across the page",
            lines: sample_lines(),
            paper: Paper {
                thicken: true,
                rule: true,
                ..Paper::plain()
            },
            turns: 0,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "a single word, which no space divides",
            lines: vec!["SIMPARITY".to_string()],
            paper: Paper::plain(),
            turns: 0,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "a page submitted a quarter turn clockwise",
            lines: sample_lines(),
            paper: Paper::plain(),
            turns: 1,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "a page submitted a half turn",
            lines: sample_lines(),
            paper: Paper::plain(),
            turns: 2,
            operation: "DetectDocumentText",
            features: Vec::new(),
        },
        Case {
            name: "a page submitted three quarters of a turn",
            lines: sample_lines(),
            paper: Paper::plain(),
            turns: 3,
            operation: "AnalyzeDocument",
            features: vec!["FORMS"],
        },
        Case {
            name: "the analysis path over a labelled page",
            lines: sample_lines(),
            paper: Paper::plain(),
            turns: 0,
            operation: "AnalyzeDocument",
            features: vec!["FORMS"],
        },
        Case {
            name: "the analysis path over a page carrying a table",
            lines: drawn_lines("Item\tPrice\nPen\t2.50\nPad\t11.75"),
            paper: Paper::plain(),
            turns: 0,
            operation: "AnalyzeDocument",
            features: vec!["TABLES"],
        },
    ]
}

/// A point of a `Polygon` as it came back over the wire.
type Point = (f64, f64);

fn rotation_of(block: &Value) -> Option<f64> {
    block
        .get("Geometry")
        .and_then(|geometry| geometry.get("RotationAngle"))
        .and_then(Value::as_f64)
}

/// The four extreme marks of the ink DRAWN on `page` — the topmost, the rightmost, the
/// bottommost and the leftmost — clockwise from the one nearest the page's top-left
/// corner, as the page-relative points `Polygon` is expressed in.
///
/// A mark is a pixel the renderer put the ink level on, so this is where the ink really
/// is: the page is lossless (PNG), and the recogniser decides ink by taking the smaller of
/// the page's two tone classes, which is the drawn text either way round.
fn drawn_outline(page: &GrayImage, ink: u8) -> Vec<Point> {
    let (width, height) = (page.width() as usize, page.height() as usize);
    let mut top: Option<(usize, usize)> = None;
    let mut right: Option<(usize, usize)> = None;
    let mut bottom: Option<(usize, usize)> = None;
    let mut left: Option<(usize, usize)> = None;
    for y in 0..height {
        for x in 0..width {
            if page.get_pixel(x as u32, y as u32).0[0] != ink {
                continue;
            }
            if top.is_none_or(|(tx, ty)| y < ty || (y == ty && x < tx)) {
                top = Some((x, y));
            }
            if right.is_none_or(|(rx, ry)| x > rx || (x == rx && y < ry)) {
                right = Some((x, y));
            }
            if bottom.is_none_or(|(bx, by)| y > by || (y == by && x > bx)) {
                bottom = Some((x, y));
            }
            if left.is_none_or(|(lx, ly)| x < lx || (x == lx && y > ly)) {
                left = Some((x, y));
            }
        }
    }
    let marks = [top, right, bottom, left].map(|mark| mark.expect("the page carries ink"));
    // A pixel covers the cell `x..x+1` by `y..y+1` of the page, so each extreme mark is
    // taken at the corner of its own pixel that faces away from the others.
    let point = |(x, y): (usize, usize), rightwards: bool, downwards: bool| {
        (
            (x + usize::from(rightwards)) as f64 / width as f64,
            (y + usize::from(downwards)) as f64 / height as f64,
        )
    };
    vec![
        point(marks[0], false, false),
        point(marks[1], true, false),
        point(marks[2], true, true),
        point(marks[3], false, true),
    ]
}

/// `outline` started at the corner a page whose characters are turned by `rotation` was
/// read from: an upright page is read from its top-left corner, and each quarter turn
/// moves the corner the reading started at one step clockwise.
fn read_from(outline: &[Point], rotation: f64) -> Vec<Point> {
    let first = (rotation / 90.0).round() as usize % outline.len();
    (0..outline.len())
        .map(|step| outline[(step + first) % outline.len()])
        .collect()
}

fn submitted(page: &GrayImage, turns: u8) -> GrayImage {
    match turns % 4 {
        1 => image::imageops::rotate90(page),
        2 => image::imageops::rotate180(page),
        3 => image::imageops::rotate270(page),
        _ => page.clone(),
    }
}

async fn read_page(router: &MultiServiceRouter, case: &Case, page: &GrayImage) -> Vec<Value> {
    let document = raster_document(page, ImageFormat::Png);
    let request = if case.features.is_empty() {
        json!({ "Document": document })
    } else {
        json!({ "Document": document, "FeatureTypes": case.features })
    };
    let (status, body) = call(router, case.operation, &request).await;
    assert_eq!(
        status, 200,
        "case {:?}: a readable page must answer 200, got {status} with {body}",
        case.name
    );
    let blocks = blocks_of(&body);
    assert_block_graph(case.name, &body, &blocks, None);
    blocks
}

/// Every block's polygon lies inside its own box and reaches each of that box's edges, so
/// the two describe the same marks.
///
/// `pub(crate)`: `crate::block_geometry` holds every block to this same invariant over its
/// own table of documents rather than restating it.
pub(crate) fn assert_inscribed(case: &str, block: &Value) {
    let rect = bbox_of(case, block);
    let (left, top, right, bottom) = (rect.left, rect.top, rect.right(), rect.bottom());
    let polygon = polygon_of(case, block);
    assert_eq!(
        polygon.len(),
        4,
        "case {case:?}: {} block {} must outline its marks with four points, got \
         {polygon:?}",
        block_type(block),
        block_id(block)
    );
    for point in &polygon {
        assert!(
            point.0 >= left - 1e-6
                && point.0 <= right + 1e-6
                && point.1 >= top - 1e-6
                && point.1 <= bottom + 1e-6,
            "case {case:?}: {} block {} Polygon must lie within its own BoundingBox \
             ({left}, {top})..({right}, {bottom}), got {polygon:?}",
            block_type(block),
            block_id(block)
        );
    }
    let edges = [
        ("Left", left, polygon.iter().map(|p| p.0).fold(f64::MAX, f64::min)),
        ("Right", right, polygon.iter().map(|p| p.0).fold(f64::MIN, f64::max)),
        ("Top", top, polygon.iter().map(|p| p.1).fold(f64::MAX, f64::min)),
        (
            "Bottom",
            bottom,
            polygon.iter().map(|p| p.1).fold(f64::MIN, f64::max),
        ),
    ];
    for (edge, expected, actual) in edges {
        assert!(
            (expected - actual).abs() < 1e-6,
            "case {case:?}: {} block {} Polygon must reach the {edge} edge of its own \
             BoundingBox ({expected}), got {actual} in {polygon:?}",
            block_type(block),
            block_id(block)
        );
    }
}

/// Whether `polygon` is nothing but the four corners of the box around it, which is what a
/// polygon measured over no marks of its own would be.
fn is_box_corners(polygon: &[Point]) -> bool {
    let left = polygon.iter().map(|p| p.0).fold(f64::MAX, f64::min);
    let right = polygon.iter().map(|p| p.0).fold(f64::MIN, f64::max);
    let top = polygon.iter().map(|p| p.1).fold(f64::MAX, f64::min);
    let bottom = polygon.iter().map(|p| p.1).fold(f64::MIN, f64::max);
    polygon.iter().all(|(x, y)| {
        (*x == left || *x == right) && (*y == top || *y == bottom)
    })
}

#[tokio::test]
async fn a_blocks_polygon_outlines_the_marks_its_bounding_box_bounds() {
    let router = harness();

    for case in cases() {
        let lines: Vec<&str> = case.lines.iter().map(String::as_str).collect();
        let page = submitted(&render(&lines, case.paper), case.turns);
        let blocks = read_page(&router, &case, &page).await;

        // Every block of the answer, whatever its type.
        for block in &blocks {
            assert_inscribed(case.name, block);
        }

        // The PAGE block spans the page's content, so its polygon is the outline of every
        // mark that was drawn on the page — read from the corner the characters were read
        // from, which is what the WORD blocks report as their RotationAngle.
        let rotation = of_type(&blocks, "WORD")
            .first()
            .and_then(|word| rotation_of(word))
            .unwrap_or_else(|| panic!("case {:?}: a WORD must report its rotation", case.name));
        let expected = read_from(&drawn_outline(&page, case.paper.ink), rotation);
        let reported = polygon_of(case.name, of_type(&blocks, "PAGE")[0]);
        for (index, (point, want)) in reported.iter().zip(expected.iter()).enumerate() {
            assert!(
                (point.0 - want.0).abs() < 1e-5 && (point.1 - want.1).abs() < 1e-5,
                "case {:?}: the PAGE Polygon must outline the marks drawn on the page \
                 ({expected:?}), got {reported:?} differing at point {index}",
                case.name
            );
        }

        // Which corner the list starts at, stated without the index arithmetic above: an
        // upright page is read from its own top-left corner, so the first point is the
        // extreme mark nearest it, and a page read upside down starts at the diagonally
        // opposite corner instead.
        let diagonal = |point: &Point| point.0 + point.1;
        let nearest_top_left = reported
            .iter()
            .map(diagonal)
            .fold(f64::MAX, f64::min);
        let furthest = reported.iter().map(diagonal).fold(f64::MIN, f64::max);
        match rotation as i64 {
            0 => assert!(
                (diagonal(&reported[0]) - nearest_top_left).abs() < 1e-5,
                "case {:?}: a page read as it was submitted outlines each item from the \
                 corner nearest the page's top-left one, got {reported:?}",
                case.name
            ),
            180 => assert!(
                (diagonal(&reported[0]) - furthest).abs() < 1e-5,
                "case {:?}: a page read upside down outlines each item from the corner \
                 furthest from the page's top-left one, got {reported:?}",
                case.name
            ),
            _ => {}
        }

        // The outline is FINER than the box: a page of printed characters has marks that
        // do not reach the corners of the rectangle around them, so at least one block
        // reports a polygon the box does not already state.
        assert!(
            blocks
                .iter()
                .any(|block| !is_box_corners(&polygon_of(case.name, block))),
            "case {:?}: a polygon must be the fine-grained outline of its block's marks, \
             but every block restated the corners of its own box: {:?}",
            case.name,
            blocks
                .iter()
                .map(|block| polygon_of(case.name, block))
                .collect::<Vec<_>>()
        );
    }
}

#[tokio::test]
async fn one_pages_outline_does_not_change_when_it_is_drawn_larger() {
    let router = harness();

    // `Point` is a ratio of the page's size, so drawing the same page — its text, its
    // margins and the page itself — at twice the size must not move an outline.
    for scale in [1usize, 2, 3] {
        let case = Case {
            name: "the same page drawn at two sizes",
            lines: sample_lines(),
            paper: Paper {
                scale,
                margin: 4 * scale,
                ..Paper::plain()
            },
            turns: 0,
            operation: "DetectDocumentText",
            features: Vec::new(),
        };
        let larger = Case {
            name: case.name,
            lines: sample_lines(),
            paper: Paper {
                scale: 2 * scale,
                margin: 8 * scale,
                ..case.paper
            },
            turns: case.turns,
            operation: case.operation,
            features: Vec::new(),
        };
        let small = render(
            &case.lines.iter().map(String::as_str).collect::<Vec<_>>(),
            case.paper,
        );
        let large = render(
            &larger.lines.iter().map(String::as_str).collect::<Vec<_>>(),
            larger.paper,
        );
        let small_blocks = read_page(&router, &case, &small).await;
        let large_blocks = read_page(&router, &larger, &large).await;
        assert_eq!(
            small_blocks.len(),
            large_blocks.len(),
            "case {:?}: both renderings carry the same text, so both must answer the same \
             blocks",
            case.name
        );
        let described = format!("scale {scale} against scale {}", 2 * scale);
        // Two pixels of the SMALLER rendering, per axis: both pages are drawn exactly
        // proportionally, so an outline may move by the edge pixel a binarisation decides
        // and no more.
        let tol = (
            2.0 / f64::from(small.width()),
            2.0 / f64::from(small.height()),
        );
        for (small_block, large_block) in small_blocks.iter().zip(large_blocks.iter()) {
            let small_polygon = polygon_of(&described, small_block);
            let large_polygon = polygon_of(&described, large_block);
            for (index, (point, other)) in small_polygon.iter().zip(large_polygon.iter()).enumerate()
            {
                assert!(
                    (point.0 - other.0).abs() < tol.0 && (point.1 - other.1).abs() < tol.1,
                    "case {described:?}: the {} block {} outline must not depend on the \
                     raster's resolution, got {small_polygon:?} against {large_polygon:?} \
                     differing at point {index} by more than {tol:?}",
                    block_type(small_block),
                    block_id(small_block)
                );
            }
        }
    }
}
