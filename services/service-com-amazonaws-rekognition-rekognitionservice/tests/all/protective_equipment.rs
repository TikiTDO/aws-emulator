// SPDX-License-Identifier: CC0-1.0

//! `DetectProtectiveEquipment` end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: RekognitionService.DetectProtectiveEquipment`), dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts.
//!
//! THE RULE UNDER TEST: the answer reports the PERSONS the submitted image carries — one per
//! person the detector measured in the pixels, numbered by its position in this call's own
//! answer, at most the documented fifteen, each carrying the confidence measured for it and
//! the one body part this detector measures — and a `Summary` only for a request that asked
//! for one, covering the persons whose measured confidence reaches the request's own floor
//! (never under 50%) and placing each of them on the side the operation keeps for a person
//! whose PPE adornment could not be determined, since nothing in this process detects an item
//! of equipment. An image carrying no person is answered with an empty list rather than
//! refused; an image this service cannot read at all is refused with the modelled error for
//! why; and nothing is persisted, so a later caller sees exactly what it would have seen
//! without the call.
//!
//! Every portrait here is DRAWN from known tones and distances by the fixtures of the
//! detection suite, so each assertion compares the service's answer against the picture the
//! test drew rather than against a recorded answer. The tables span what the rule generalises
//! over: how many persons an image carries (none, one, two of different sizes, three, a crowd
//! past the cap), where in the frame they sit, their size, tilt and tone, the frame's size,
//! how the image is submitted (inline, from an S3 object), which equipment types a request
//! requires, the confidence floor it asks the summary for — including both published bounds of
//! that member's range and one past each — and how an image fails to be readable at all.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The router harness, the dispatch helper and the reduced reply come from the
// collection-lifecycle suite, and the drawn portraits and the object-store harness from the
// detection suite, rather than being built a second time here.
use crate::face_collections::{bytes_image, call, harness, Reply};
use crate::face_detection::{harness_with_object_store, number, s3, DrawnFace, Portrait, BUCKET};

/// The equipment types a request can require
/// (`smithy_query(query="shape",shape="ProtectiveEquipmentType")`: `FACE_COVER`, `HAND_COVER`,
/// `HEAD_COVER`).
const EQUIPMENT_TYPES: [&str; 3] = ["FACE_COVER", "HAND_COVER", "HEAD_COVER"];

/// The published bounds of the `MinConfidence` member's range
/// (`smithy_query(query="shape",shape="Percent")`: `range 0..100`).
const PERCENT_MIN: f64 = 0.0;
const PERCENT_MAX: f64 = 100.0;

/// The floor a summary never goes below, whatever a request asks for: "if you specify a value
/// that is less than 50%, the results are the same specifying a value of 50%"
/// (`smithy_query(query="doc",shape="ProtectiveEquipmentSummarizationAttributes",member="MinConfidence")`).
const SUMMARY_FLOOR: f64 = 50.0;

/// The most persons one answer covers: "DetectProtectiveEquipment detects PPE worn by up to 15
/// persons detected in an image" (the operation's own documentation).
const MAX_PERSONS: usize = 15;

/// The body part this emulator's detector measures, as the model's `BodyPart` enum names it
/// (`smithy_query(query="shape",shape="BodyPart")`).
const MEASURED_BODY_PART: &str = "FACE";

/// The PPE model version every answer reports.
const MODEL_VERSION: &str = "1.0";

/// The two sides of the summary nothing here can place a person on, since both are claims
/// about equipment items no detector in this process looks for.
const UNDETERMINED_SIDES: [&str; 2] = [
    "PersonsWithRequiredEquipment",
    "PersonsWithoutRequiredEquipment",
];

/// One `DetectProtectiveEquipment` call, with the summarization attributes a request carries
/// or none at all.
async fn detect_equipment(
    router: &MultiServiceRouter,
    image: Value,
    summarization: Option<Value>,
) -> Reply {
    let mut body = json!({ "Image": image });
    if let Some(attributes) = summarization {
        body["SummarizationAttributes"] = attributes;
    }
    call(router, "DetectProtectiveEquipment", body).await
}

/// One `DetectFaces` call: the faces the detector reports for the same pixels, through the
/// operation whose whole job is reporting them.
async fn detect_faces(router: &MultiServiceRouter, image: Value) -> Vec<Value> {
    let reply = call(router, "DetectFaces", json!({ "Image": image })).await;
    assert_eq!(
        reply.status, 200,
        "detecting the faces of the same pixels must succeed, got {:?}",
        reply.body
    );
    reply.list("FaceDetails")
}

/// The summarization attributes a request carries for `floor` and `types`.
fn summarize(floor: f64, types: &[&str]) -> Value {
    json!({ "MinConfidence": floor, "RequiredEquipmentTypes": types })
}

/// The `Persons` of a served answer, with what every answer must and must not carry already
/// checked: the model version, each person's id, its measured confidence, the one body part
/// the detector measures and the members nothing measured.
fn persons_of(case: &str, reply: &Reply) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the detection must be served, got {} with {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.string("ProtectiveEquipmentModelVersion").as_deref(),
        Some(MODEL_VERSION),
        "case {case:?}: the answer names the model behind it, got {:?}",
        reply.body
    );
    let persons = reply.list("Persons");
    assert!(
        reply.field("Persons").is_some(),
        "case {case:?}: the person list is answered even when it is empty, got {:?}",
        reply.body
    );
    for (index, person) in persons.iter().enumerate() {
        assert_eq!(
            person.get("Id").and_then(Value::as_i64),
            Some(i64::try_from(index).expect("the index is small")),
            "case {case:?}: a person is numbered by its position in this answer, \
             got {person:?}"
        );
        let confidence = number(person, &["Confidence"]);
        assert!(
            (SUMMARY_FLOOR..=PERCENT_MAX).contains(&confidence),
            "case {case:?}: the detector reports predictions between 50% and 100%, \
             got {confidence}"
        );
        // Where a person EXTENDS is not what a face detector measured, so no box is
        // reported for the person.
        assert!(
            person.get("BoundingBox").is_none_or(Value::is_null),
            "case {case:?}: the person's extent is not measured here, got {person:?}"
        );
        let parts = person
            .get("BodyParts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            parts.len(),
            1,
            "case {case:?}: the one measured body part is reported, got {person:?}"
        );
        let part = &parts[0];
        assert_eq!(
            part.get("Name").and_then(Value::as_str),
            Some(MEASURED_BODY_PART),
            "case {case:?}: the measured body part is the face, got {part:?}"
        );
        assert_eq!(
            number(part, &["Confidence"]),
            confidence,
            "case {case:?}: the part carries the confidence measured for it, got {part:?}"
        );
        // An item of PPE around a body part is a judgement nothing here makes, so the member
        // is absent rather than an empty list claiming the part was examined.
        assert!(
            part.get("EquipmentDetections").is_none_or(Value::is_null),
            "case {case:?}: no equipment detection is claimed, got {part:?}"
        );
    }
    persons
}

/// The ids one side of a served summary names.
fn summary_ids(case: &str, reply: &Reply, side: &str) -> Vec<i64> {
    let summary = reply
        .field("Summary")
        .unwrap_or_else(|| panic!("case {case:?}: the summary must be answered, got {:?}", reply.body));
    let ids = summary
        .get(side)
        .unwrap_or_else(|| panic!("case {case:?}: the summary carries {side}, got {summary:?}"));
    ids.as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {side} is a list of ids, got {ids:?}"))
        .iter()
        .map(|id| {
            id.as_i64()
                .unwrap_or_else(|| panic!("case {case:?}: {side} holds person ids, got {id:?}"))
        })
        .collect()
}

/// One row of the portrait table: the picture and how many persons it carries.
struct PortraitCase {
    case: &'static str,
    portrait: Portrait,
    persons: usize,
}

/// The portraits every table below draws from: none, one, two or three persons, at several
/// positions, sizes, tilts, tones and frame sizes. Each row's count is what the detection
/// suite pins for the same picture.
fn portrait_cases() -> Vec<PortraitCase> {
    vec![
        PortraitCase {
            case: "a flat light image with no person at all",
            portrait: Portrait::blank(320, 320, 235),
            persons: 0,
        },
        PortraitCase {
            case: "a flat dark image with no person at all",
            portrait: Portrait::blank(240, 240, 12),
            persons: 0,
        },
        PortraitCase {
            case: "one centred person",
            portrait: Portrait::one_face(),
            persons: 1,
        },
        PortraitCase {
            case: "two persons of different sizes",
            portrait: Portrait::one_face().with_size(480, 320).with_faces(vec![
                DrawnFace::new(130.0, 150.0, 56.0),
                DrawnFace::new(350.0, 140.0, 36.0),
            ]),
            persons: 2,
        },
        PortraitCase {
            case: "three persons across a wide frame",
            portrait: Portrait::one_face().with_size(640, 480).with_faces(vec![
                DrawnFace::new(150.0, 140.0, 56.0),
                DrawnFace::new(470.0, 140.0, 56.0),
                DrawnFace::new(310.0, 360.0, 56.0),
            ]),
            persons: 3,
        },
        PortraitCase {
            case: "a person whose head is tilted",
            portrait: Portrait::one_face()
                .with_faces(vec![DrawnFace::new(160.0, 150.0, 56.0).rolled(10.0)]),
            persons: 1,
        },
        PortraitCase {
            case: "a small person in a large frame",
            portrait: Portrait::one_face()
                .with_size(640, 480)
                .with_faces(vec![DrawnFace::new(300.0, 220.0, 56.0)]),
            persons: 1,
        },
        PortraitCase {
            case: "a close-up person larger than the frame",
            portrait: Portrait::one_face()
                .with_faces(vec![DrawnFace::new(160.0, 140.0, 110.0)]),
            persons: 1,
        },
        PortraitCase {
            case: "bright skin on a dark background",
            portrait: Portrait::one_face().with_tones(60, 220, 20),
            persons: 1,
        },
        PortraitCase {
            case: "a dim portrait",
            portrait: Portrait::one_face().with_tones(20, 90, 10),
            persons: 1,
        },
    ]
}

/// A crowd of `count` drawn faces on one frame, laid out four to a row and spaced wider than
/// the head the fixture draws (2.2 eye spans across, 3.0 tall), so no two of them touch. The
/// frame grows with the rows, so every drawn face is inside it.
fn crowd(count: usize) -> Portrait {
    let span = 48.0;
    let columns = 4;
    let rows = count.div_ceil(columns);
    let faces = (0..count)
        .map(|index| {
            let column = (index % columns) as f32;
            let row = (index / columns) as f32;
            DrawnFace::new(120.0 + column * 200.0, 150.0 + row * 250.0, span)
        })
        .collect();
    Portrait::one_face()
        .with_size(840, 250 * u32::try_from(rows).expect("the row count is small") + 150)
        .with_faces(faces)
}

/// Every person the image carries is reported once, and reported as the detector measured it:
/// the count is the count `DetectFaces` reports for the same pixels, and each person's
/// confidence is the confidence measured for the face that located it.
#[tokio::test]
async fn every_person_the_image_carries_is_reported_once() {
    let router = harness();
    for PortraitCase {
        case,
        portrait,
        persons: drawn,
    } in portrait_cases()
    {
        let bytes = portrait.png();
        let reply = detect_equipment(&router, bytes_image(&bytes), None).await;
        let persons = persons_of(case, &reply);
        assert_eq!(
            persons.len(),
            drawn,
            "case {case:?}: the {drawn} drawn person(s) must each be reported once, \
             got {persons:?}"
        );
        let faces = detect_faces(&router, bytes_image(&bytes)).await;
        assert_eq!(
            persons.len(),
            faces.len().min(MAX_PERSONS),
            "case {case:?}: a person is reported for each face the detector measured, \
             got {persons:?} against {faces:?}"
        );
        for (index, (person, face)) in persons.iter().zip(faces.iter()).enumerate() {
            assert_eq!(
                number(person, &["Confidence"]),
                number(face, &["Confidence"]),
                "case {case:?}: person {index} carries the confidence the detector \
                 measured, got {person:?} against {face:?}"
            );
        }
    }
}

/// One answer covers at most the documented fifteen persons, the ones located by the largest
/// faces, while an image with fewer carries them all.
#[tokio::test]
async fn an_answer_covers_at_most_the_documented_fifteen_persons() {
    let router = harness();
    for count in [MAX_PERSONS - 1, MAX_PERSONS, MAX_PERSONS + 1] {
        let case = format!("{count} drawn faces");
        let bytes = crowd(count).png();
        let faces = detect_faces(&router, bytes_image(&bytes)).await;
        assert_eq!(
            faces.len(),
            count,
            "case {case:?}: the crowd must carry {count} measured faces, got {}",
            faces.len()
        );
        let persons = persons_of(&case, &detect_equipment(&router, bytes_image(&bytes), None).await);
        assert_eq!(
            persons.len(),
            count.min(MAX_PERSONS),
            "case {case:?}: at most {MAX_PERSONS} persons are reported, got {}",
            persons.len()
        );
        // The persons kept are the ones the detector reported first, which are the largest
        // faces: each kept person carries the confidence of the face at its own position.
        for (index, (person, face)) in persons.iter().zip(faces.iter()).enumerate() {
            assert_eq!(
                number(person, &["Confidence"]),
                number(face, &["Confidence"]),
                "case {case:?}: person {index} is the face at the same position, \
                 got {person:?} against {face:?}"
            );
        }
    }
}

/// The summary covers exactly the persons whose measured confidence reaches the floor the
/// request asked for — never a floor under 50% — and places each of them on the side the
/// operation keeps for a person whose PPE adornment could not be determined. The table spans
/// the required equipment types and the floor, including both published bounds of its range.
#[tokio::test]
async fn the_summary_covers_the_persons_whose_confidence_reaches_the_floor() {
    let router = harness();
    let required: Vec<Vec<&str>> = vec![
        vec![EQUIPMENT_TYPES[0]],
        vec![EQUIPMENT_TYPES[1]],
        vec![EQUIPMENT_TYPES[2]],
        EQUIPMENT_TYPES.to_vec(),
        vec![EQUIPMENT_TYPES[2], EQUIPMENT_TYPES[0]],
    ];
    for PortraitCase {
        case,
        portrait,
        persons: drawn,
    } in portrait_cases()
    {
        let bytes = portrait.png();
        // The confidences measured for this picture, which is what a floor is compared
        // against. Read from an answer rather than assumed.
        let measured: Vec<f64> = persons_of(
            case,
            &detect_equipment(&router, bytes_image(&bytes), None).await,
        )
        .iter()
        .map(|person| number(person, &["Confidence"]))
        .collect();
        assert_eq!(
            measured.len(),
            drawn,
            "case {case:?}: the picture carries {drawn} person(s), got {measured:?}"
        );
        let lowest = measured.iter().copied().fold(PERCENT_MAX, f64::min);
        let floors: Vec<f64> = vec![
            PERCENT_MIN,
            SUMMARY_FLOOR,
            (lowest - 1.0).clamp(PERCENT_MIN, PERCENT_MAX),
            (lowest + 1.0).clamp(PERCENT_MIN, PERCENT_MAX),
            PERCENT_MAX,
        ];
        for floor in floors {
            for types in &required {
                let row = format!("{case}, floor {floor}, requiring {types:?}");
                let reply = detect_equipment(
                    &router,
                    bytes_image(&bytes),
                    Some(summarize(floor, types)),
                )
                .await;
                let persons = persons_of(&row, &reply);
                assert_eq!(
                    persons.len(),
                    drawn,
                    "case {row:?}: asking for a summary does not narrow the person list, \
                     got {persons:?}"
                );
                // A floor under 50% is the same request as a floor of 50%.
                let effective = floor.max(SUMMARY_FLOOR);
                let expected: Vec<i64> = measured
                    .iter()
                    .enumerate()
                    .filter(|(_, confidence)| **confidence >= effective)
                    .map(|(index, _)| i64::try_from(index).expect("the index is small"))
                    .collect();
                assert_eq!(
                    summary_ids(&row, &reply, "PersonsIndeterminate"),
                    expected,
                    "case {row:?}: the summary covers the persons measured at or above \
                     {effective}, got {:?}",
                    reply.body
                );
                for side in UNDETERMINED_SIDES {
                    assert_eq!(
                        summary_ids(&row, &reply, side),
                        Vec::<i64>::new(),
                        "case {row:?}: {side} is a claim about undetected equipment, \
                         got {:?}",
                        reply.body
                    );
                }
            }
        }
    }
}

/// A request that asks for no summary is answered without one: the summary is "summary
/// information for the types of PPE specified in the SummarizationAttributes input parameter",
/// and this request specified none.
#[tokio::test]
async fn a_request_that_asks_for_no_summary_is_answered_without_one() {
    let router = harness();
    for PortraitCase { case, portrait, .. } in portrait_cases() {
        let reply = detect_equipment(&router, bytes_image(&portrait.png()), None).await;
        persons_of(case, &reply);
        assert!(
            reply.field("Summary").is_none(),
            "case {case:?}: nothing was asked to be summarized, got {:?}",
            reply.body
        );
    }
}

/// A floor one past either published bound of the member's range is refused by the model
/// before the handler runs.
#[tokio::test]
async fn a_floor_outside_the_published_range_is_refused() {
    let router = harness();
    let bytes = Portrait::one_face().png();
    for (case, floor) in [
        ("below the bottom of the MinConfidence range", PERCENT_MIN - 1.0),
        ("above the top of the MinConfidence range", PERCENT_MAX + 1.0),
    ] {
        let reply = detect_equipment(
            &router,
            bytes_image(&bytes),
            Some(summarize(floor, &EQUIPMENT_TYPES)),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a floor outside the range is refused, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: the modelled range refuses it at the boundary, got {:?}",
            reply.body
        );
    }
}

/// One image answers the same whichever way it was submitted: inline bytes and the object this
/// emulator's Amazon S3 holds at the named location are read alike.
#[tokio::test]
async fn an_s3_sourced_image_answers_like_inline_bytes() {
    let router = harness_with_object_store();
    assert_eq!(
        s3(&router, &format!("/{BUCKET}"), &[]).await,
        200,
        "the image bucket must be created first"
    );
    for PortraitCase {
        case,
        portrait,
        persons: drawn,
    } in portrait_cases()
    {
        for key in ["worker.png", "nested/prefix/worker.png"] {
            let bytes = portrait.png();
            assert_eq!(
                s3(&router, &format!("/{BUCKET}/{key}"), &bytes).await,
                200,
                "case {case:?}: the image must be staged at {key}"
            );
            let attributes = Some(summarize(SUMMARY_FLOOR, &EQUIPMENT_TYPES));
            let staged = detect_equipment(
                &router,
                json!({ "S3Object": { "Bucket": BUCKET, "Name": key } }),
                attributes.clone(),
            )
            .await;
            let inline = detect_equipment(&router, bytes_image(&bytes), attributes).await;
            assert_eq!(
                persons_of(case, &staged).len(),
                drawn,
                "case {case:?}: the staged image carries {drawn} person(s), got {:?}",
                staged.body
            );
            assert_eq!(
                staged.body, inline.body,
                "case {case:?}: the object at {key} must answer exactly as the same bytes \
                 submitted inline, got {:?} against {:?}",
                staged.body, inline.body
            );
        }
    }
}

/// The call is a READ of the submitted pixels: "the operation does not persist any data", so a
/// second identical call answers identically and every stored resource is exactly as it was.
#[tokio::test]
async fn detecting_protective_equipment_changes_nothing_a_later_caller_sees() {
    let router = harness();
    let collection = "protective-equipment";
    let created = call(
        &router,
        "CreateCollection",
        json!({ "CollectionId": collection }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the collection must be created, got {:?}",
        created.body
    );
    let bytes = Portrait::one_face().png();
    let indexed = call(
        &router,
        "IndexFaces",
        json!({ "CollectionId": collection, "Image": bytes_image(&bytes) }),
    )
    .await;
    assert_eq!(
        indexed.status, 200,
        "the face must be indexed, got {:?}",
        indexed.body
    );
    let before = call(
        &router,
        "DescribeCollection",
        json!({ "CollectionId": collection }),
    )
    .await;

    let attributes = Some(summarize(SUMMARY_FLOOR, &EQUIPMENT_TYPES));
    let first = detect_equipment(&router, bytes_image(&bytes), attributes.clone()).await;
    let second = detect_equipment(&router, bytes_image(&bytes), attributes).await;
    assert_eq!(
        persons_of("first call", &first).len(),
        1,
        "the drawn person is reported, got {:?}",
        first.body
    );
    assert_eq!(
        first.body, second.body,
        "case {:?}: two identical calls answer identically, got {:?} against {:?}",
        "repeated call", first.body, second.body
    );

    let after = call(
        &router,
        "DescribeCollection",
        json!({ "CollectionId": collection }),
    )
    .await;
    assert_eq!(
        before.body, after.body,
        "case {:?}: the stored collection is untouched by the call, got {:?} against {:?}",
        "described collection", after.body, before.body
    );
    let listed = call(&router, "ListCollections", json!({})).await;
    assert_eq!(
        listed.list("CollectionIds"),
        vec![Value::from(collection)],
        "case {:?}: no resource is created by the call, got {:?}",
        "listed collections",
        listed.body
    );
}

/// An image this service cannot read at all is refused with the modelled error for WHY, never
/// with an empty answer: a 200 carrying no person is reserved for an image that was read and
/// carried none.
#[tokio::test]
async fn an_unreadable_image_is_refused_with_the_modelled_error_for_why() {
    let router = harness();
    // Each row: what the request identifies as its image, and the modelled answer for it. The
    // messages are the service's own fixed wording for each class.
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "bytes that are not an image at all",
            bytes_image(b"not-an-image-at-all"),
            "InvalidImageFormatException",
            "Request has invalid image format",
        ),
        (
            // The service reads PNG and JPEG — "the image must be either a PNG or JPG
            // formatted file" — and the format comes from the bytes' own magic numbers, so a
            // GIF signature is content in a format it does not read.
            "bytes in an image format this service does not read",
            bytes_image(b"GIF89a\x01\x00\x01\x00\x80\x00\x00"),
            "InvalidImageFormatException",
            "Request has invalid image format",
        ),
        (
            "an image identifying no content at all",
            json!({}),
            "InvalidParameterException",
            "Requested image should either contain bytes or s3 object.",
        ),
        (
            "an S3 object the store does not serve",
            json!({ "S3Object": { "Bucket": "no-such-bucket", "Name": "no-such-key" } }),
            "InvalidS3ObjectException",
            "Unable to get object metadata from S3. Check object key, region and/or access \
             permissions.",
        ),
    ];

    for (case, image, expected_code, expected_message) in cases {
        for attributes in [None, Some(summarize(SUMMARY_FLOOR, &EQUIPMENT_TYPES))] {
            let reply = detect_equipment(&router, image.clone(), attributes).await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected HTTP 400, got {} with {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                expected_code,
                "case {case:?}: expected __type {expected_code}, got {:?}",
                reply.body
            );
            assert_eq!(
                reply.string("Code").as_deref(),
                Some(expected_code),
                "case {case:?}: expected Code {expected_code}, got {:?}",
                reply.body
            );
            assert_eq!(
                reply.string("Message").as_deref(),
                Some(expected_message),
                "case {case:?}: expected Message {expected_message:?}, got {:?}",
                reply.body
            );
            assert!(
                reply.field("Persons").is_none() && reply.field("Summary").is_none(),
                "case {case:?}: a refusal carries no person and no summary, got {:?}",
                reply.body
            );
        }
    }
}
