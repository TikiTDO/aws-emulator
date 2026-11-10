// SPDX-License-Identifier: CC0-1.0

//! `DetectLabels` end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: RekognitionService.DetectLabels`), dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts.
//!
//! THE RULE UNDER TEST: `DetectLabels` reports what the SUBMITTED IMAGE carries and only
//! what was measured in it — one label per entity measured in the pixels, with the ancestor
//! label the taxonomy puts above it and an `Instance` box per measured instance, and, where
//! the request asks for image properties, the measured quality and dominant colours of the
//! pixels themselves — so an image carrying no measurable entity is answered with an empty
//! label list rather than a refusal, a member nothing measured is absent, `Features` decides
//! which of the two analyses runs, and `MaxLabels`, `MinConfidence` and
//! `Settings.GeneralLabels` only NARROW what was measured. The image's bytes are the
//! request's own `Bytes` or the object this emulator's Amazon S3 holds at the `S3Object`
//! named, read by their own content and never by the name they are stored under, and an
//! image this service cannot read at all is refused with the modelled error for why.
//! Nothing is persisted: a second identical call answers identically.
//!
//! Every image below is DRAWN by the test from known tones, distances and colours — a
//! portrait from the detection suite's own drawing kit, a banded grey frame, a two-colour
//! frame — so each assertion compares the service's answer against what the test PUT in the
//! image rather than against a recorded answer. The tables span what the rule generalises
//! over: how many entities an image carries (none, one, two), the encoded format, how the
//! image was submitted (inline, from an S3 object), which `Features` are asked for, every
//! published bound of `MaxLabels` and `MinConfidence` and one past each, which filters the
//! settings name, how many dominant colours are asked for, and which part of an S3 address
//! does not resolve.

use std::io::Cursor;

use image::{DynamicImage, GrayImage, ImageFormat, Luma, Rgb, RgbImage};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::face_collections::{bytes_image, call, harness, Reply};
use crate::face_detection::{encode, harness_with_object_store, s3, DrawnFace, Portrait, BUCKET};

/// The label a measured face is reported as, and its one ancestor.
const FACE: &str = "Face";
const PERSON: &str = "Person";

/// The version of the label detection model this emulator answers with.
const LABEL_MODEL_VERSION: &str = "3.0";

/// The confidence floor a request naming no `MinConfidence` is narrowed to: "if
/// MinConfidence is not specified, the operation returns labels with a confidence values
/// greater than or equal to 55 percent".
const DEFAULT_MIN_CONFIDENCE: f64 = 55.0;

/// `MaxLabels`'s published ceiling: "the default and upper limit is 1000 labels".
const MAX_LABELS_LIMIT: i64 = 1_000;

/// `MinConfidence` is `Percent`, `range 0..100`
/// (`smithy_query(query="operation",operation="DetectLabels")`).
const PERCENT_MIN: f64 = 0.0;
const PERCENT_MAX: f64 = 100.0;

/// How far a measured percentage may sit from the value the test computed off the tones it
/// drew, in percentage points: the two are the same arithmetic on the same pixels.
const MEASURED_TOLERANCE: f64 = 0.5;

/// The members of a reported `Label` that come from a published taxonomy this emulator does
/// not hold, and so may never appear (`smithy_query(query="shape",shape="Label")`).
const UNPUBLISHED_MEMBERS: [&str; 2] = ["Categories", "Aliases"];

/// The twelve simplified colour names a measured colour is reported under — "One of 12
/// simplified color names"
/// (`smithy_query(query="doc",shape="DominantColor",member="SimplifiedColor")`) — which is
/// also the vocabulary a colour LABEL is named from, capitalised.
const SIMPLIFIED_COLORS: [&str; 12] = [
    "black", "grey", "white", "red", "orange", "yellow", "green", "cyan", "blue", "purple",
    "pink", "brown",
];

/// The label name of a simplified colour name.
fn colour_label(name: &str) -> String {
    let mut capitalised = name.to_string();
    capitalised[..1].make_ascii_uppercase();
    capitalised
}

/// The label a frame of essentially one colour carries, and how much of it one colour must
/// cover for that.
const MONOCHROME: &str = "Monochrome";
const MONOCHROME_SHARE: f64 = 95.0;

/// Every label of an answer that is not one of the measured entities is a colour of the
/// frame or the composition those colours add up to.
fn assert_colour_vocabulary(case: &str, labels: &[Value]) {
    let vocabulary: Vec<String> = SIMPLIFIED_COLORS.iter().map(|name| colour_label(name)).collect();
    for name in names(labels) {
        if name == FACE || name == PERSON || name == MONOCHROME {
            continue;
        }
        assert!(
            vocabulary.contains(&name),
            "case {case:?}: a colour label is named from the simplified colour names, \
             got {name:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The images the tests draw
// ---------------------------------------------------------------------------

/// A portrait carrying two faces of different sizes, which the detection suite pins as two.
fn two_face_portrait() -> Portrait {
    Portrait::one_face().with_size(480, 320).with_faces(vec![
        DrawnFace::new(130.0, 150.0, 56.0),
        DrawnFace::new(350.0, 140.0, 36.0),
    ])
}

/// A grey frame of `height` rows whose top `band` rows carry `top` and the rest `bottom`.
fn banded(width: u32, height: u32, band: u32, top: u8, bottom: u8) -> GrayImage {
    GrayImage::from_fn(width, height, |_, y| {
        Luma([if y < band { top } else { bottom }])
    })
}

/// A grey frame divided into `tones.len()` equal horizontal bands, top to bottom.
fn banded_tones(width: u32, height: u32, tones: &[u8]) -> GrayImage {
    let bands = tones.len() as u32;
    GrayImage::from_fn(width, height, |_, y| {
        Luma([tones[((y * bands / height).min(bands - 1)) as usize]])
    })
}

/// A colour frame whose leftmost `left_columns` columns carry `left` and the rest `right`.
fn two_colour(width: u32, height: u32, left_columns: u32, left: [u8; 3], right: [u8; 3]) -> RgbImage {
    RgbImage::from_fn(width, height, |x, _| {
        Rgb(if x < left_columns { left } else { right })
    })
}

/// A colour frame drawn entirely in one colour.
fn flat_colour(width: u32, height: u32, colour: [u8; 3]) -> RgbImage {
    RgbImage::from_pixel(width, height, Rgb(colour))
}

/// A colour frame as the PNG bytes a client submits.
fn colour_png(image: &RgbImage) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image.clone())
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("the frame encodes");
    bytes.into_inner()
}

/// The mean of a frame's intensities, as the percentage of full scale `Brightness` carries.
fn drawn_brightness(image: &GrayImage) -> f64 {
    let total: f64 = image.pixels().map(|pixel| f64::from(pixel.0[0])).sum();
    total / image.pixels().len() as f64 / 255.0 * 100.0
}

/// The standard deviation of a frame's intensities, as the percentage of the largest
/// deviation an 8-bit population can have (half the scale) that `Contrast` carries.
fn drawn_contrast(image: &GrayImage) -> f64 {
    let count = image.pixels().len() as f64;
    let mean: f64 = image.pixels().map(|pixel| f64::from(pixel.0[0])).sum::<f64>() / count;
    let variance: f64 = image
        .pixels()
        .map(|pixel| (f64::from(pixel.0[0]) - mean).powi(2))
        .sum::<f64>()
        / count;
    variance.sqrt() / 127.5 * 100.0
}

// ---------------------------------------------------------------------------
// Reading the answers
// ---------------------------------------------------------------------------

/// One `DetectLabels` call.
async fn detect(router: &MultiServiceRouter, body: Value) -> Reply {
    call(router, "DetectLabels", body).await
}

/// The `Labels` of a successful answer, with what every reported label must and must not
/// carry already checked.
fn labels_of(case: &str, reply: &Reply) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: DetectLabels must answer, got {} with {:?}",
        reply.status, reply.body
    );
    assert!(
        reply.field("OrientationCorrection").is_none(),
        "case {case:?}: OrientationCorrection is always null, got {:?}",
        reply.body
    );
    let labels = reply
        .field("Labels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: Labels must be present, got {:?}", reply.body));
    for label in &labels {
        assert!(
            label.get("Name").and_then(Value::as_str).is_some(),
            "case {case:?}: a reported label names an entity, got {label:?}"
        );
        assert!(
            label.get("Confidence").and_then(Value::as_f64).is_some(),
            "case {case:?}: a reported label carries the confidence measured, got {label:?}"
        );
        for member in UNPUBLISHED_MEMBERS {
            assert!(
                label.get(member).is_none_or(Value::is_null),
                "case {case:?}: no taxonomy is held here, so {member} must be absent, \
                 got {label:?}"
            );
        }
    }
    labels
}

/// The label of `name` in an answer, if it is there.
fn label_named<'a>(labels: &'a [Value], name: &str) -> Option<&'a Value> {
    labels
        .iter()
        .find(|label| label.get("Name").and_then(Value::as_str) == Some(name))
}

/// The names of the reported labels, in the order they were reported.
fn names(labels: &[Value]) -> Vec<String> {
    labels
        .iter()
        .filter_map(|label| label.get("Name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// A label's `Instances`.
fn instances(label: &Value) -> Vec<Value> {
    label
        .get("Instances")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A float member of a JSON object, by path.
fn number(case: &str, value: &Value, path: &[&str]) -> f64 {
    let mut current = value;
    for step in path {
        current = current
            .get(step)
            .unwrap_or_else(|| panic!("case {case:?}: {path:?} must be present in {value:?}"));
    }
    current
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: {path:?} must be a number in {value:?}"))
}

/// `measured` is `expected`, to within the tolerance the drawn tones allow.
fn assert_close(case: &str, what: &str, measured: f64, expected: f64) {
    assert!(
        (measured - expected).abs() <= MEASURED_TOLERANCE,
        "case {case:?}: {what} must be the {expected} the test drew, got {measured}"
    );
}

// ---------------------------------------------------------------------------
// What an image carries
// ---------------------------------------------------------------------------

/// One label per entity MEASURED in the image, with the ancestor it brings with it and an
/// instance per measured face, and one per colour the frame carries; an image carrying no
/// entity reports no entity label at all. The same answer whichever format the image was
/// encoded in and however it was submitted.
#[tokio::test]
async fn labels_report_the_entities_measured_in_the_image() {
    let router = harness();
    // How many faces the test DREW on each frame, which is how many instances the one
    // entity label it reports must locate.
    let cases: Vec<(&str, Portrait, usize)> = vec![
        ("one centred face", Portrait::one_face(), 1),
        ("two faces of different sizes", two_face_portrait(), 2),
        ("a flat light image", Portrait::blank(240, 240, 205), 0),
        ("a flat dark image", Portrait::blank(240, 240, 15), 0),
    ];

    for (drawing, portrait, drawn) in cases {
        for (format, label) in [(ImageFormat::Png, "PNG"), (ImageFormat::Jpeg, "JPEG")] {
            let case = format!("{drawing}, {label}");
            let bytes = encode(&portrait.render(), format);
            let reply = detect(&router, json!({ "Image": bytes_image(&bytes) })).await;
            let labels = labels_of(&case, &reply);
            assert_eq!(
                reply.string("LabelModelVersion").as_deref(),
                Some(LABEL_MODEL_VERSION),
                "case {case:?}: label detection ran, so it reports the model that ran, \
                 got {:?}",
                reply.body
            );
            assert_colour_vocabulary(&case, &labels);
            if drawn == 0 {
                for entity in [FACE, PERSON] {
                    assert!(
                        label_named(&labels, entity).is_none(),
                        "case {case:?}: no face was drawn, so no {entity} is reported, \
                         got {labels:?}"
                    );
                }
                continue;
            }
            assert_eq!(
                names(&labels)[..2],
                [FACE.to_string(), PERSON.to_string()],
                "case {case:?}: a measured face is reported with the ancestor above it, \
                 got {labels:?}"
            );
            let face = label_named(&labels, FACE).expect("the measured label is there");
            let person = label_named(&labels, PERSON).expect("the ancestor is there");
            assert_eq!(
                instances(face).len(),
                drawn,
                "case {case:?}: one instance per face drawn, got {face:?}"
            );
            assert!(
                number(&case, face, &["Confidence"]) >= DEFAULT_MIN_CONFIDENCE,
                "case {case:?}: a well drawn face is measured above the default floor, \
                 got {face:?}"
            );
            assert_eq!(
                face.get("Parents"),
                Some(&json!([{ "Name": PERSON }])),
                "case {case:?}: the ancestor of the measured label is reported on it"
            );
            assert_eq!(
                person.get("Parents").and_then(Value::as_array),
                Some(&Vec::new()),
                "case {case:?}: the root of the taxonomy reported has no ancestor, \
                 got {person:?}"
            );
            assert!(
                person.get("Instances").is_none_or(Value::is_null),
                "case {case:?}: where a person extends was not measured, got {person:?}"
            );
            for instance in instances(face) {
                let bounds = instance
                    .get("BoundingBox")
                    .unwrap_or_else(|| panic!("case {case:?}: an instance is located"));
                for (member, span) in [("Left", "Width"), ("Top", "Height")] {
                    let start = number(&case, bounds, &[member]);
                    let length = number(&case, bounds, &[span]);
                    assert!(
                        (0.0..=1.0).contains(&start) && length > 0.0 && start + length <= 1.0001,
                        "case {case:?}: a box is a fraction of the image, got {bounds:?}"
                    );
                }
                assert!(
                    number(&case, &instance, &["Confidence"]) >= DEFAULT_MIN_CONFIDENCE,
                    "case {case:?}: an instance carries the confidence measured for it, \
                     got {instance:?}"
                );
            }
        }
    }
}

/// The answer is a function of the submitted image: two identical calls agree, and the
/// labels of an image read from an S3 object are the labels of the same bytes submitted
/// inline.
#[tokio::test]
async fn one_image_answers_alike_however_it_was_submitted() {
    let router = harness_with_object_store();
    let bytes = Portrait::one_face().png();
    assert_eq!(
        s3(&router, &format!("/{BUCKET}"), &[]).await,
        200,
        "the bucket the image is staged in must be created"
    );
    assert_eq!(
        s3(&router, &format!("/{BUCKET}/one-face.png"), &bytes).await,
        200,
        "the image must be staged"
    );
    let submissions: Vec<(&str, Value)> = vec![
        ("inline bytes", bytes_image(&bytes)),
        (
            "an S3 object",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "one-face.png" } }),
        ),
    ];
    let mut answers: Vec<(&str, Vec<Value>)> = Vec::new();
    for (case, image) in submissions {
        let first = detect(&router, json!({ "Image": image })).await;
        let again = detect(&router, json!({ "Image": image })).await;
        let labels = labels_of(case, &first);
        assert_eq!(
            labels_of(case, &again),
            labels,
            "case {case:?}: nothing is persisted, so the second call answers identically"
        );
        answers.push((case, labels));
    }
    let (first_case, first) = &answers[0];
    for (case, labels) in &answers[1..] {
        assert_eq!(
            labels, first,
            "case {case:?}: the same bytes measure the same as in {first_case:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Narrowing
// ---------------------------------------------------------------------------

/// `MaxLabels` and `MinConfidence` NARROW what was measured: every label reported is above
/// the floor, no more than `MaxLabels` of them are reported, and they are the
/// highest-confidence ones — a prefix of the unnarrowed answer. The table runs each
/// published bound of both members and one past it.
#[tokio::test]
async fn max_labels_and_min_confidence_narrow_what_was_measured() {
    let router = harness();
    // A face drawn with its eye line TILTED agrees less with the level proportions the
    // detector reads than a square-on one, so it is measured below the top of the scale:
    // there is then a confidence both above and below what was measured to narrow by.
    let tilted =
        Portrait::one_face().with_faces(vec![DrawnFace::new(160.0, 150.0, 56.0).rolled(10.0)]);
    let image = bytes_image(&tilted.png());
    let unnarrowed = labels_of("no narrowing", &detect(&router, json!({ "Image": image })).await);
    let face = label_named(&unnarrowed, FACE)
        .expect("the drawn face must be measured before narrowing is tested");
    let measured = number("no narrowing", face, &["Confidence"]);
    assert!(
        (DEFAULT_MIN_CONFIDENCE..PERCENT_MAX).contains(&measured),
        "the tilted face must measure inside the scale for this table to narrow by it, \
         got {measured}"
    );
    assert!(
        unnarrowed.len() >= 3,
        "the frame must measure several labels for the caps to bite, got {unnarrowed:?}"
    );

    // (what the request narrows by, whether it must remove something)
    let cases: Vec<(&str, Value, bool)> = vec![
        ("MaxLabels at its lower bound", json!({ "MaxLabels": 0 }), true),
        ("MaxLabels of one", json!({ "MaxLabels": 1 }), true),
        ("MaxLabels of two", json!({ "MaxLabels": 2 }), true),
        (
            "MaxLabels above what was measured",
            json!({ "MaxLabels": 7 }),
            false,
        ),
        (
            "MaxLabels at its published limit",
            json!({ "MaxLabels": MAX_LABELS_LIMIT }),
            false,
        ),
        (
            "MaxLabels past its published limit",
            json!({ "MaxLabels": MAX_LABELS_LIMIT + 1 }),
            false,
        ),
        (
            "MinConfidence at its lower bound",
            json!({ "MinConfidence": PERCENT_MIN }),
            false,
        ),
        (
            "MinConfidence at its upper bound",
            json!({ "MinConfidence": PERCENT_MAX }),
            true,
        ),
        (
            "MinConfidence just below what was measured for the face",
            json!({ "MinConfidence": measured - 1.0 }),
            true,
        ),
        (
            "MinConfidence just above what was measured for the face",
            json!({ "MinConfidence": measured + 1.0 }),
            true,
        ),
        (
            "both, with the cap deciding",
            json!({ "MaxLabels": 1, "MinConfidence": PERCENT_MIN }),
            true,
        ),
        (
            "both, with the floor deciding",
            json!({ "MaxLabels": 1, "MinConfidence": PERCENT_MAX }),
            true,
        ),
    ];

    for (case, narrowing, narrows) in cases {
        let mut body = narrowing.clone();
        body.as_object_mut()
            .expect("the narrowing is an object")
            .insert("Image".to_string(), image.clone());
        let floor = narrowing
            .get("MinConfidence")
            .and_then(Value::as_f64)
            .unwrap_or(DEFAULT_MIN_CONFIDENCE);
        let cap = narrowing
            .get("MaxLabels")
            .and_then(Value::as_i64)
            .unwrap_or(MAX_LABELS_LIMIT)
            .min(MAX_LABELS_LIMIT) as usize;
        // What the rule leaves of the unnarrowed answer: the labels above the floor, the
        // highest-confidence `MaxLabels` of them. The unnarrowed answer is already ordered
        // by descending confidence.
        let expected: Vec<Value> = unnarrowed
            .iter()
            .filter(|label| number(case, label, &["Confidence"]) >= floor)
            .take(cap)
            .cloned()
            .collect();
        let reply = detect(&router, body).await;
        let labels = labels_of(case, &reply);
        assert_eq!(
            labels, expected,
            "case {case:?}: expected {expected:?}, got {labels:?}"
        );
        assert_eq!(
            labels.len() < unnarrowed.len(),
            narrows,
            "case {case:?}: this row must {} the answer, got {labels:?} of {unnarrowed:?}",
            if narrows { "narrow" } else { "leave" }
        );
        for label in &labels {
            assert!(
                number(case, label, &["Confidence"]) >= floor,
                "case {case:?}: no label below the floor {floor} is reported, got {label:?}"
            );
            assert!(
                labels.len() <= cap,
                "case {case:?}: no more than {cap} label(s) are reported, got {labels:?}"
            );
        }
    }
}

/// A value outside a member's published range is refused at the boundary, before anything
/// is measured — one step past each bound.
#[tokio::test]
async fn a_narrowing_value_outside_its_published_range_is_refused() {
    let router = harness();
    let image = bytes_image(&Portrait::one_face().png());
    let cases: Vec<(&str, Value)> = vec![
        ("MinConfidence below its range", json!({ "MinConfidence": PERCENT_MIN - 1.0 })),
        ("MinConfidence above its range", json!({ "MinConfidence": PERCENT_MAX + 1.0 })),
        ("MaxLabels below its range", json!({ "MaxLabels": -1 })),
    ];
    for (case, narrowing) in cases {
        let mut body = narrowing;
        body.as_object_mut()
            .expect("the narrowing is an object")
            .insert("Image".to_string(), image.clone());
        let reply = detect(&router, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            reply.status, reply.body
        );
    }
}

/// `Settings.GeneralLabels` selects which of the measured labels are reported: an inclusion
/// filter narrows to what it names, an exclusion filter removes what it names. No label
/// reported here carries a category, so a category inclusion filter admits none of them and
/// a category exclusion filter removes none.
#[tokio::test]
async fn the_general_label_filters_select_which_labels_are_reported() {
    let router = harness();
    let image = bytes_image(&Portrait::one_face().png());
    let unfiltered = names(&labels_of(
        "no filters",
        &detect(&router, json!({ "Image": image })).await,
    ));
    assert!(
        unfiltered.len() >= 3,
        "the frame must measure several labels for the filters to select from, \
         got {unfiltered:?}"
    );
    // The colour of the frame, which is a label the entity filters below do not name.
    let colour = unfiltered
        .iter()
        .find(|name| *name != FACE && *name != PERSON)
        .cloned()
        .expect("the frame carries a measured colour");

    // (the case, the filters, which of the unfiltered labels must survive them)
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "an inclusion filter naming the measured entity",
            json!({ "LabelInclusionFilters": [FACE] }),
            vec![FACE.to_string()],
        ),
        (
            "an inclusion filter naming the ancestor",
            json!({ "LabelInclusionFilters": [PERSON] }),
            vec![PERSON.to_string()],
        ),
        (
            "an inclusion filter naming both entities",
            json!({ "LabelInclusionFilters": [FACE, PERSON] }),
            vec![FACE.to_string(), PERSON.to_string()],
        ),
        (
            "an inclusion filter naming the colour measured",
            json!({ "LabelInclusionFilters": [colour] }),
            vec![colour.clone()],
        ),
        (
            "an inclusion filter naming an entity nothing measured",
            json!({ "LabelInclusionFilters": ["Flower"] }),
            Vec::new(),
        ),
        (
            "an exclusion filter naming the measured entity",
            json!({ "LabelExclusionFilters": [FACE] }),
            unfiltered
                .iter()
                .filter(|name| *name != FACE)
                .cloned()
                .collect(),
        ),
        (
            "an exclusion filter naming both entities",
            json!({ "LabelExclusionFilters": [FACE, PERSON] }),
            unfiltered
                .iter()
                .filter(|name| *name != FACE && *name != PERSON)
                .cloned()
                .collect(),
        ),
        (
            "an empty filter list",
            json!({ "LabelInclusionFilters": [] }),
            unfiltered.clone(),
        ),
        (
            "a category inclusion filter, with no label carrying a category",
            json!({ "LabelCategoryInclusionFilters": ["Person Description"] }),
            Vec::new(),
        ),
        (
            "a category exclusion filter, with no label carrying a category",
            json!({ "LabelCategoryExclusionFilters": ["Person Description"] }),
            unfiltered.clone(),
        ),
    ];
    for (case, filters, expected) in cases {
        let reply = detect(
            &router,
            json!({ "Image": image, "Settings": { "GeneralLabels": filters } }),
        )
        .await;
        let labels = labels_of(case, &reply);
        assert_eq!(
            names(&labels),
            expected,
            "case {case:?}: expected {expected:?}, got {labels:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Features, and the properties of the pixels
// ---------------------------------------------------------------------------

/// `Features` decides which analysis runs: labels for `GENERAL_LABELS`, which is also what a
/// request naming no feature asks for, and image properties for `IMAGE_PROPERTIES`. What one
/// feature reports does not depend on the other having been asked for.
#[tokio::test]
async fn features_decide_which_analysis_runs() {
    let router = harness();
    let image = bytes_image(&Portrait::one_face().png());
    // (the case, the Features member, labels expected, image properties expected)
    let cases: Vec<(&str, Option<Value>, bool, bool)> = vec![
        ("no Features at all", None, true, false),
        ("an empty feature list", Some(json!([])), true, false),
        (
            "GENERAL_LABELS",
            Some(json!(["GENERAL_LABELS"])),
            true,
            false,
        ),
        (
            "IMAGE_PROPERTIES",
            Some(json!(["IMAGE_PROPERTIES"])),
            false,
            true,
        ),
        (
            "both features",
            Some(json!(["GENERAL_LABELS", "IMAGE_PROPERTIES"])),
            true,
            true,
        ),
        (
            "both features, the other way round",
            Some(json!(["IMAGE_PROPERTIES", "GENERAL_LABELS"])),
            true,
            true,
        ),
    ];
    for (case, features, labelled, propertied) in cases {
        let mut body = json!({ "Image": image });
        if let Some(features) = features {
            body.as_object_mut()
                .expect("the request is an object")
                .insert("Features".to_string(), features);
        }
        let reply = detect(&router, body).await;
        let labels = labels_of(case, &reply);
        assert_eq!(
            !labels.is_empty(),
            labelled,
            "case {case:?}: labels are reported only where label detection ran, \
             got {labels:?}"
        );
        assert_eq!(
            reply.string("LabelModelVersion").is_some(),
            labelled,
            "case {case:?}: the label model is reported only where it ran, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.field("ImageProperties").is_some(),
            propertied,
            "case {case:?}: image properties are reported only where asked for, got {:?}",
            reply.body
        );
    }
}

/// The image properties reported are MEASURED off the pixels: the brightness and the
/// contrast the test drew, a sharpness that rises with the edges in the frame, the dominant
/// colours the frame actually holds, and a foreground and a background that are the two
/// sides of the frame's own intensity split.
#[tokio::test]
async fn image_properties_measure_the_pixels() {
    let router = harness();
    // A quarter of the frame dark over three quarters light: a known mean, a known
    // deviation, and a ring of border pixels that is mostly the lighter side.
    let (dark, light) = (40_u8, 200_u8);
    let drawn = banded(120, 160, 40, dark, light);
    let case = "a banded grey frame";
    let reply = detect(
        &router,
        json!({
            "Image": bytes_image(&encode(&drawn, ImageFormat::Png)),
            "Features": ["IMAGE_PROPERTIES"],
        }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: DetectLabels must answer, got {:?}",
        reply.body
    );
    let properties = reply
        .field("ImageProperties")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: image properties were asked for"));

    assert_close(
        case,
        "Quality.Brightness",
        number(case, &properties, &["Quality", "Brightness"]),
        drawn_brightness(&drawn),
    );
    assert_close(
        case,
        "Quality.Contrast",
        number(case, &properties, &["Quality", "Contrast"]),
        drawn_contrast(&drawn),
    );

    // The two tones the frame holds, most covered first, each reported as the colour of its
    // own pixels and the share of the frame they cover.
    let colours = properties
        .get("DominantColors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        colours.len(),
        2,
        "case {case:?}: the frame holds two tones, got {colours:?}"
    );
    for (position, (tone, share)) in [(light, 75.0), (dark, 25.0)].into_iter().enumerate() {
        let colour = &colours[position];
        for channel in ["Red", "Green", "Blue"] {
            assert_close(
                case,
                channel,
                number(case, colour, &[channel]),
                f64::from(tone),
            );
        }
        assert_eq!(
            colour.get("HexCode").and_then(Value::as_str),
            Some(format!("#{tone:02X}{tone:02X}{tone:02X}").as_str()),
            "case {case:?}: the hex code is the colour measured, got {colour:?}"
        );
        assert_close(
            case,
            "PixelPercent",
            number(case, colour, &["PixelPercent"]),
            share,
        );
        assert!(
            colour.get("CSSColor").is_none_or(Value::is_null),
            "case {case:?}: no CSS colour catalog is held here, so CSSColor must be absent, \
             got {colour:?}"
        );
        assert!(
            colour
                .get("SimplifiedColor")
                .and_then(Value::as_str)
                .is_some_and(|name| SIMPLIFIED_COLORS.contains(&name)),
            "case {case:?}: a measured colour is named from the simplified colour names, \
             got {colour:?}"
        );
    }

    // The split: the lighter side holds most of the frame's border, so it is the background,
    // and each side reports its own tone at the whole of its own pixels.
    for (side, tone) in [("Background", light), ("Foreground", dark)] {
        let measured = properties
            .get(side)
            .unwrap_or_else(|| panic!("case {case:?}: {side} must be reported"));
        assert_close(
            case,
            &format!("{side}.Quality.Brightness"),
            number(case, measured, &["Quality", "Brightness"]),
            f64::from(tone) / 255.0 * 100.0,
        );
        assert_close(
            case,
            &format!("{side}.Quality.Contrast"),
            number(case, measured, &["Quality", "Contrast"]),
            0.0,
        );
        let side_colours = measured
            .get("DominantColors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            side_colours.len(),
            1,
            "case {case:?}: one side of the split holds one tone, got {side_colours:?}"
        );
        assert_close(
            case,
            &format!("{side} PixelPercent"),
            number(case, &side_colours[0], &["PixelPercent"]),
            100.0,
        );
        assert_close(
            case,
            &format!("{side} tone"),
            number(case, &side_colours[0], &["Red"]),
            f64::from(tone),
        );
    }
}

/// Sharpness is a measurement too: a flat frame has no gradient to measure, one edge
/// measures less than many, and blurring the same picture lowers it.
#[tokio::test]
async fn sharpness_rises_with_the_edges_in_the_frame() {
    let router = harness();
    let frames: Vec<(&str, GrayImage)> = vec![
        ("a flat frame", banded(120, 160, 160, 128, 128)),
        ("one edge", banded(120, 160, 80, 40, 200)),
        (
            "many edges",
            banded_tones(120, 160, &[40, 200, 40, 200, 40, 200, 40, 200]),
        ),
    ];
    let mut measured: Vec<(&str, f64)> = Vec::new();
    for (case, frame) in &frames {
        let reply = detect(
            &router,
            json!({
                "Image": bytes_image(&encode(frame, ImageFormat::Png)),
                "Features": ["IMAGE_PROPERTIES"],
            }),
        )
        .await;
        let properties = reply
            .field("ImageProperties")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: image properties were asked for, got {:?}", reply.body));
        measured.push((case, number(case, &properties, &["Quality", "Sharpness"])));
    }
    assert_close(measured[0].0, "Quality.Sharpness", measured[0].1, 0.0);
    for window in measured.windows(2) {
        let ((lower, less), (higher, more)) = (window[0], window[1]);
        assert!(
            more > less,
            "case {higher:?}: more edges measure sharper than {lower:?}, got {more} and {less}"
        );
    }
}

/// The dominant colours are the colours the frame holds, in colour as much as in grey, and
/// `Settings.ImageProperties.MaxDominantColors` caps how many of them are reported —
/// defaulting to 10 where the request names none, and never more than the frame holds.
#[tokio::test]
async fn dominant_colours_report_the_colours_the_frame_holds() {
    let router = harness();
    let blue = [0_u8, 0, 255];
    let yellow = [255_u8, 255, 0];
    let frame = two_colour(100, 50, 60, blue, yellow);
    let case = "a two-colour frame";
    let reply = detect(
        &router,
        json!({
            "Image": bytes_image(&colour_png(&frame)),
            "Features": ["IMAGE_PROPERTIES"],
        }),
    )
    .await;
    let properties = reply
        .field("ImageProperties")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: image properties were asked for, got {:?}", reply.body));
    let colours = properties
        .get("DominantColors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        colours.len(),
        2,
        "case {case:?}: the frame holds two colours, got {colours:?}"
    );
    for (position, (drawn, share)) in [(blue, 60.0), (yellow, 40.0)].into_iter().enumerate() {
        let colour = &colours[position];
        for (channel, value) in ["Red", "Green", "Blue"].into_iter().zip(drawn) {
            assert_close(
                case,
                channel,
                number(case, colour, &[channel]),
                f64::from(value),
            );
        }
        assert_eq!(
            colour.get("HexCode").and_then(Value::as_str),
            Some(format!("#{:02X}{:02X}{:02X}", drawn[0], drawn[1], drawn[2]).as_str()),
            "case {case:?}: the hex code is the colour measured, got {colour:?}"
        );
        assert_close(
            case,
            "PixelPercent",
            number(case, colour, &["PixelPercent"]),
            share,
        );
    }

    // How many are reported: a frame of four tones, capped by what the request asks for.
    let four = banded_tones(80, 160, &[20, 80, 150, 220]);
    let cases: Vec<(&str, Option<i64>, usize)> = vec![
        ("no MaxDominantColors", None, 4),
        ("MaxDominantColors of one", Some(1), 1),
        ("MaxDominantColors of two", Some(2), 2),
        ("MaxDominantColors of four", Some(4), 4),
        ("MaxDominantColors above what the frame holds", Some(20), 4),
    ];
    for (case, most, expected) in cases {
        let settings = most.map_or(json!({}), |most| {
            json!({ "ImageProperties": { "MaxDominantColors": most } })
        });
        let reply = detect(
            &router,
            json!({
                "Image": bytes_image(&encode(&four, ImageFormat::Png)),
                "Features": ["IMAGE_PROPERTIES"],
                "Settings": settings,
            }),
        )
        .await;
        let properties = reply
            .field("ImageProperties")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: image properties were asked for, got {:?}", reply.body));
        let colours = properties
            .get("DominantColors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            colours.len(),
            expected,
            "case {case:?}: expected {expected} colour(s), got {colours:?}"
        );
        // Most covered first, whatever the cap.
        for window in colours.windows(2) {
            assert!(
                number(case, &window[0], &["PixelPercent"])
                    >= number(case, &window[1], &["PixelPercent"]),
                "case {case:?}: the colours are reported most covered first, got {colours:?}"
            );
        }
    }
}

/// An instance's own colours are measured off its own box, and only where the request asked
/// for image properties at all.
#[tokio::test]
async fn an_instance_reports_the_colours_of_its_own_box() {
    let router = harness();
    let image = bytes_image(&Portrait::one_face().png());
    for (case, features, coloured) in [
        ("labels alone", json!(["GENERAL_LABELS"]), false),
        (
            "labels and image properties",
            json!(["GENERAL_LABELS", "IMAGE_PROPERTIES"]),
            true,
        ),
    ] {
        let reply = detect(&router, json!({ "Image": image, "Features": features })).await;
        let labels = labels_of(case, &reply);
        let face = label_named(&labels, FACE)
            .unwrap_or_else(|| panic!("case {case:?}: the drawn face must be measured"));
        for instance in instances(face) {
            let colours = instance
                .get("DominantColors")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                !colours.is_empty(),
                coloured,
                "case {case:?}: an instance's colours are measured only where asked for, \
                 got {instance:?}"
            );
            let covered: f64 = colours
                .iter()
                .map(|colour| number(case, colour, &["PixelPercent"]))
                .sum();
            assert!(
                colours.is_empty() || (0.0..=100.01).contains(&covered),
                "case {case:?}: the shares are of the box's own pixels, got {colours:?}"
            );
        }
    }
}

/// A measured colour is named from the twelve simplified colour names, and a frame carrying
/// that colour is reported as a LABEL under the same name capitalised — one row per entry of
/// the catalog, each drawn in a colour that falls under it. A frame drawn in one colour also
/// admits no foreground/background split, so neither side is reported for it.
#[tokio::test]
async fn a_measured_colour_is_named_from_the_simplified_colour_names() {
    let router = harness();
    // One colour per entry of the catalog, drawn flat so the frame carries that one colour
    // and no entity at all.
    let cases: Vec<(&str, [u8; 3])> = vec![
        ("black", [0, 0, 0]),
        ("grey", [128, 128, 128]),
        ("white", [255, 255, 255]),
        ("red", [230, 20, 20]),
        ("orange", [255, 140, 0]),
        ("yellow", [240, 230, 40]),
        ("green", [40, 180, 60]),
        ("cyan", [40, 200, 210]),
        ("blue", [30, 60, 220]),
        ("purple", [130, 40, 180]),
        ("pink", [255, 192, 203]),
        ("brown", [120, 70, 20]),
    ];
    assert_eq!(
        cases.len(),
        SIMPLIFIED_COLORS.len(),
        "every name of the catalog must be drawn, got {cases:?}"
    );
    for (case, drawn) in cases {
        let reply = detect(
            &router,
            json!({
                "Image": bytes_image(&colour_png(&flat_colour(96, 96, drawn))),
                "Features": ["GENERAL_LABELS", "IMAGE_PROPERTIES"],
            }),
        )
        .await;
        let labels = labels_of(case, &reply);
        assert_eq!(
            names(&labels),
            vec![colour_label(case), MONOCHROME.to_string()],
            "case {case:?}: the frame's one colour is its label, and one colour is a \
             monochrome composition, got {labels:?}"
        );
        assert_close(
            case,
            "the colour label's confidence",
            number(case, &labels[0], &["Confidence"]),
            100.0,
        );
        assert!(
            number(case, &labels[1], &["Confidence"]) >= MONOCHROME_SHARE,
            "case {case:?}: the composition carries the share one colour covers, \
             got {labels:?}"
        );
        let properties = reply
            .field("ImageProperties")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: image properties were asked for"));
        let colours = properties
            .get("DominantColors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            colours.len(),
            1,
            "case {case:?}: the frame holds one colour, got {colours:?}"
        );
        assert_eq!(
            colours[0].get("SimplifiedColor").and_then(Value::as_str),
            Some(case),
            "case {case:?}: the measured colour is named for its family, got {colours:?}"
        );
        for side in ["Foreground", "Background"] {
            assert!(
                properties.get(side).is_none_or(Value::is_null),
                "case {case:?}: one colour admits no split, so {side} must be absent, \
                 got {properties:?}"
            );
        }
    }
}

/// A frame essentially ONE colour is reported as a monochrome composition, and one carrying
/// a second colour in any real quantity is not — the table runs the share at which that
/// turns over, on both sides of it.
#[tokio::test]
async fn a_frame_of_one_colour_is_a_monochrome_composition() {
    let router = harness();
    let red = [220_u8, 30, 30];
    let blue = [30_u8, 60, 220];
    // (the case, how many of the 100 columns carry the second colour, monochrome expected)
    let cases: Vec<(&str, u32, bool)> = vec![
        ("one colour alone", 0, true),
        ("a second colour on a fiftieth of the frame", 2, true),
        ("a second colour on a twentieth of the frame", 5, true),
        ("a second colour just past a twentieth", 6, false),
        ("two colours in quantity", 40, false),
    ];
    for (case, columns, monochrome) in cases {
        let frame = two_colour(100, 50, columns, blue, red);
        let reply = detect(
            &router,
            json!({ "Image": bytes_image(&colour_png(&frame)) }),
        )
        .await;
        let labels = labels_of(case, &reply);
        assert_colour_vocabulary(case, &labels);
        assert_eq!(
            label_named(&labels, MONOCHROME).is_some(),
            monochrome,
            "case {case:?}: one colour covering {}% of the frame is {}monochrome, \
             got {labels:?}",
            100 - columns,
            if monochrome { "" } else { "not " }
        );
        assert!(
            label_named(&labels, colour_label("red").as_str()).is_some(),
            "case {case:?}: the colour most of the frame carries is a label of it, \
             got {labels:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Images this service cannot read
// ---------------------------------------------------------------------------/// An image this service cannot read is refused with the modelled error for WHY, never with
/// an empty answer that would read as "nothing in this image". The format comes from the
/// bytes' own content, so the name an object is stored under decides nothing.
#[tokio::test]
async fn an_unreadable_image_is_refused_with_the_error_for_why() {
    let router = harness_with_object_store();
    let png = Portrait::one_face().png();
    let jpeg = encode(&Portrait::one_face().render(), ImageFormat::Jpeg);
    assert_eq!(
        s3(&router, &format!("/{BUCKET}"), &[]).await,
        200,
        "the bucket the objects are staged in must be created"
    );
    for (key, bytes) in [
        ("notes.jpg", b"these are notes, not an image".to_vec()),
        ("portrait.png", jpeg.clone()),
        ("portrait.jpg", png.clone()),
    ] {
        assert_eq!(
            s3(&router, &format!("/{BUCKET}/{key}"), &bytes).await,
            200,
            "the object {key} must be staged"
        );
    }

    // (the case, the Image submitted, the error it is refused with, or "" for an answer)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "bytes that are no image",
            bytes_image(b"this is not an image at all"),
            "InvalidImageFormatException",
        ),
        (
            "a truncated PNG",
            bytes_image(&png[..png.len() / 3]),
            "InvalidImageFormatException",
        ),
        (
            "an object named .jpg holding text",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "notes.jpg" } }),
            "InvalidImageFormatException",
        ),
        (
            "an object the store does not hold",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "no-such-key.png" } }),
            "InvalidS3ObjectException",
        ),
        (
            "an object in a bucket the store does not hold",
            json!({ "S3Object": { "Bucket": "no-such-bucket", "Name": "portrait.png" } }),
            "InvalidS3ObjectException",
        ),
        (
            // A version of an object in a bucket that was never versioned is a version the
            // store does not hold, so there is no more to fetch than for a missing key.
            "a version of an object in a never-versioned bucket",
            json!({
                "S3Object": { "Bucket": BUCKET, "Name": "portrait.png", "Version": "pBEGMk6cPn" },
            }),
            "InvalidS3ObjectException",
        ),
        (
            "an address with no key",
            json!({ "S3Object": { "Bucket": BUCKET } }),
            "InvalidS3ObjectException",
        ),
        (
            "an address with no bucket",
            json!({ "S3Object": { "Name": "portrait.png" } }),
            "InvalidS3ObjectException",
        ),
        (
            "an image identifying no content",
            json!({}),
            "InvalidParameterException",
        ),
        (
            // The name an object is stored under decides nothing: a JPEG named .png and a
            // PNG named .jpg are both read for what their bytes are.
            "a JPEG stored under a .png name",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "portrait.png" } }),
            "",
        ),
        (
            "a PNG stored under a .jpg name",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "portrait.jpg" } }),
            "",
        ),
    ];

    for (case, image, expected) in cases {
        let reply = detect(&router, json!({ "Image": image })).await;
        if expected.is_empty() {
            let labels = labels_of(case, &reply);
            assert!(
                !labels.is_empty(),
                "case {case:?}: the drawn face must be measured, got {labels:?}"
            );
            continue;
        }
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            expected,
            "case {case:?}: expected __type {expected}, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.string("Code").as_deref(),
            Some(expected),
            "case {case:?}: the body names the failure in Code too, got {:?}",
            reply.body
        );
        assert!(
            reply
                .string("Message")
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: the refusal states why, got {:?}",
            reply.body
        );
    }
}
