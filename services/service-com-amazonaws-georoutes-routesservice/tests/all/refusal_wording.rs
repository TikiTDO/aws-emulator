// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a request it refuses.
//!
//! A refusal is part of the contract: a client branches on the status and the error code,
//! and points its user at the member the `fieldList` names. The rules under test are
//! therefore not "this recorded request gives this recorded sentence" but invariants over
//! classes of refused requests:
//!
//! * a rule between members a model cannot express (at least one of a mutually-sufficient
//!   pair; an id that must not repeat) is stated as fixed text naming those members, with
//!   no request value quoted into it;
//! * a cap that depends on a mode states the cap and the mode it applies under;
//! * a value outside the domain or the range its member allows is stated by the bound it
//!   broke;
//! * whatever the rule, `message` and `fieldList[0].message` carry the SAME sentence, and
//!   `fieldList[0].name` names the offending member — down to the element's position and
//!   the member within it where the offender is an element of a list.
//!
//! Every table spans what its rule generalises over: which member carries the offending
//! value, where in a list it sits, which bound was broken, which operation is being
//! called, and the sizes and values on and past each published bound.

use serde_json::{json, Value};

use crate::routing::{post, routes_service, ROUTES, SNAP};

/// The paths of the operations these refusals come from. `routing.rs` exports the two this
/// crate's other suites share; the rest are named here.
const MATRIX: &str = "/v2/route-matrix";
const OPTIMIZE: &str = "/v2/optimize-waypoints";

/// A position well inside the WGS 84 domain and on this service's network.
const GOOD: [f64; 2] = [-123.1, 49.28];

/// What a refusal must carry: the member its `fieldList` names and the sentence it
/// states. `None` where the request must be SERVED instead.
type Expect = Option<(&'static str, &'static str)>;

/// As [`Expect`], where the sentence is built per row because it quotes the value the
/// request set.
type ExpectOwned = Option<(&'static str, String)>;

/// One row of a table that posts a body to an operation's path.
type PathRow = (&'static str, &'static str, Value, Expect);

/// As [`PathRow`], for a sentence built per row.
type CoordinateRow = (&'static str, &'static str, Value, ExpectOwned);

/// One row of the matrix-cap table: the case, how many origins and destinations it posts,
/// the routing boundary, and what the refusal must carry.
type MatrixRow = (&'static str, usize, usize, Value, Expect);

/// What a refusal said: the error code, the message, the reason, and the first
/// `fieldList` entry's name and message.
#[derive(Debug)]
struct Refusal {
    code: String,
    message: String,
    reason: String,
    field_name: String,
    field_message: String,
}

/// Read a refusal off a response body, asserting the parts every refusal of this service
/// carries: the modeled `ValidationException` code, a `reason` the model's enum declares,
/// exactly one `fieldList` entry, and the same sentence in `message` and that entry.
fn refusal(case: &str, status: u16, answer: &Value) -> Refusal {
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 refusal, got {status} with {answer:?}"
    );
    let text = |member: &str| -> String {
        answer[member]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: {member} is missing from {answer:?}"))
            .to_string()
    };
    let fields = answer["fieldList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: fieldList is required, got {answer:?}"));
    assert_eq!(
        fields.len(),
        1,
        "case {case:?}: one broken rule is one fieldList entry, got {fields:?}"
    );
    let read = Refusal {
        code: text("__type"),
        message: text("message"),
        reason: text("reason"),
        field_name: fields[0]["name"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: fieldList[0].name is required, got {fields:?}"))
            .to_string(),
        field_message: fields[0]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: fieldList[0].message is required, got {fields:?}"))
            .to_string(),
    };
    assert_eq!(
        read.code, "ValidationException",
        "case {case:?}: a refused request is the modeled ValidationException, got {:?}",
        read.code
    );
    assert_eq!(
        read.reason, "FieldValidationFailed",
        "case {case:?}: a broken field rule is reported as FieldValidationFailed, got {:?}",
        read.reason
    );
    assert_eq!(
        read.field_message, read.message,
        "case {case:?}: the fieldList entry restates the message, got {:?} against {:?}",
        read.field_message, read.message
    );
    read
}

/// `Origin` alone says where a journey starts but not where it goes, so a request that
/// sets neither of the two members that do is refused, while any one of them is enough.
/// The sentence names the mutually-sufficient members and the `fieldList` names the first
/// of them.
#[tokio::test]
async fn optimize_waypoints_requires_a_destination_or_a_waypoint() {
    let router = routes_service();
    // The one sentence under test: fixed text whose only variable part is the member list.
    let expected = "At least one of the following fields must be set: Destination, Waypoints.";
    // Rows differ along what the rule generalises over: which of the two members is set,
    // how many entries the list carries, and whether the list is present but empty.
    let cases: [(&str, Value, u16); 6] = [
        ("origin only", json!({ "Origin": GOOD }), 400),
        (
            "origin and an empty waypoint list",
            json!({ "Origin": GOOD, "Waypoints": [] }),
            400,
        ),
        (
            "origin and a destination",
            json!({ "Origin": GOOD, "Destination": [-123.12, 49.26] }),
            200,
        ),
        (
            "origin and one waypoint",
            json!({ "Origin": GOOD, "Waypoints": [{ "Id": "a", "Position": [-123.11, 49.27] }] }),
            200,
        ),
        (
            "origin and three waypoints",
            json!({
                "Origin": GOOD,
                "Waypoints": [
                    { "Id": "a", "Position": [-123.11, 49.27] },
                    { "Id": "b", "Position": [-123.05, 49.25] },
                    { "Id": "c", "Position": [-123.2, 49.3] }
                ]
            }),
            200,
        ),
        (
            "origin, a destination and a waypoint",
            json!({
                "Origin": GOOD,
                "Destination": [-123.12, 49.26],
                "Waypoints": [{ "Id": "a", "Position": [-123.11, 49.27] }]
            }),
            200,
        ),
    ];

    for (case, body, status_expected) in cases {
        let (status, answer, _) = post(&router, OPTIMIZE, &body).await;
        if status_expected == 200 {
            assert_eq!(
                status, 200,
                "case {case:?}: somewhere to go was set, so the request must be served, got {status} with {answer:?}"
            );
            continue;
        }
        let read = refusal(case, status, &answer);
        assert_eq!(
            read.message, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            read.message
        );
        assert_eq!(
            read.field_name, "Destination",
            "case {case:?}: the refusal names the first of the mutually-sufficient members, got {:?}",
            read.field_name
        );
    }
}

/// Two stops cannot share one id. The sentence names the `Id` member and the members an id
/// can come from, quoting no id out of the request, and the `fieldList` names the offending
/// stop's own `Id` — with the element's position where the duplicate sits in a list.
#[tokio::test]
async fn a_repeated_stop_id_names_the_offending_element() {
    let router = routes_service();
    let expected = "'Id' must be unique across 'OriginOptions', 'Waypoints' and 'DestinationOptions'.";
    let position = |index: usize| -> Value {
        json!([-123.1 - (index as f64) * 0.01, 49.28 + (index as f64) * 0.01])
    };
    // Rows differ along where the duplicate sits: which member carries it, which position
    // in the list, and how many stops the request has.
    let cases: [(&str, Value, Option<&str>); 6] = [
        (
            "the second waypoint repeats the first",
            json!({
                "Origin": GOOD,
                "Waypoints": [
                    { "Id": "stop", "Position": position(1) },
                    { "Id": "stop", "Position": position(2) }
                ]
            }),
            Some("Waypoints[1].Id"),
        ),
        (
            "the third waypoint repeats the first",
            json!({
                "Origin": GOOD,
                "Waypoints": [
                    { "Id": "stop", "Position": position(1) },
                    { "Id": "other", "Position": position(2) },
                    { "Id": "stop", "Position": position(3) }
                ]
            }),
            Some("Waypoints[2].Id"),
        ),
        (
            "the first waypoint repeats the origin id",
            json!({
                "Origin": GOOD,
                "OriginOptions": { "Id": "start" },
                "Waypoints": [{ "Id": "start", "Position": position(1) }]
            }),
            Some("Waypoints[0].Id"),
        ),
        (
            "the destination repeats a waypoint id",
            json!({
                "Origin": GOOD,
                "Destination": [-123.12, 49.26],
                "DestinationOptions": { "Id": "stop" },
                "Waypoints": [{ "Id": "stop", "Position": position(1) }]
            }),
            Some("DestinationOptions.Id"),
        ),
        (
            "the destination repeats the origin id",
            json!({
                "Origin": GOOD,
                "OriginOptions": { "Id": "start" },
                "Destination": [-123.12, 49.26],
                "DestinationOptions": { "Id": "start" }
            }),
            Some("DestinationOptions.Id"),
        ),
        (
            "distinct ids throughout",
            json!({
                "Origin": GOOD,
                "OriginOptions": { "Id": "start" },
                "Destination": [-123.12, 49.26],
                "DestinationOptions": { "Id": "finish" },
                "Waypoints": [
                    { "Id": "one", "Position": position(1) },
                    { "Id": "two", "Position": position(2) }
                ]
            }),
            None,
        ),
    ];

    for (case, body, offender) in cases {
        let (status, answer, _) = post(&router, OPTIMIZE, &body).await;
        let Some(offender) = offender else {
            assert_eq!(
                status, 200,
                "case {case:?}: ids that do not repeat must be served, got {status} with {answer:?}"
            );
            continue;
        };
        let read = refusal(case, status, &answer);
        assert_eq!(
            read.message, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            read.message
        );
        assert_eq!(
            read.field_name, offender,
            "case {case:?}: expected the offending stop's Id member {offender:?}, got {:?}",
            read.field_name
        );
    }
}

/// The matrix caps depend on the routing boundary, so the refusal states the cap AND the
/// boundary mode it applies under, and never quotes the counts back. A request at a cap is
/// served.
#[tokio::test]
async fn a_matrix_past_a_cap_states_the_cap_and_the_boundary_mode() {
    let router = routes_service();
    let points = |count: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|index| json!({ "Position": [-123.1 - (index as f64) * 0.001, 49.28] }))
                .collect(),
        )
    };
    let unbounded = json!({ "Unbounded": true });
    // The documented limits: an unbounded boundary allows 15 origins, 100 destinations and
    // 100 pairs; a geometry boundary allows 500, 500 and 160,000. Rows sit on each cap, one
    // past it, and under both boundary modes.
    let geometry = json!({
        "Geometry": { "BoundingBox": [-130.0, 40.0, -120.0, 55.0] }
    });
    let cases: [MatrixRow; 8] = [
        (
            "unbounded at the pair cap",
            10,
            10,
            unbounded.clone(),
            None,
        ),
        (
            "unbounded past the pair cap",
            11,
            10,
            unbounded.clone(),
            Some((
                "Origins",
                "The total number of routes to calculate must be less than or equal to 100 \
                 when 'RoutingBoundary.Unbounded' has value true.",
            )),
        ),
        (
            "unbounded at the origin cap",
            15,
            1,
            unbounded.clone(),
            None,
        ),
        (
            "unbounded past the origin cap",
            16,
            1,
            unbounded.clone(),
            Some((
                "Origins",
                "The number of origins must be less than or equal to 15 when \
                 'RoutingBoundary.Unbounded' has value true.",
            )),
        ),
        (
            "unbounded at the destination cap",
            1,
            100,
            unbounded.clone(),
            None,
        ),
        (
            "unbounded past the destination cap",
            1,
            101,
            unbounded.clone(),
            Some((
                "Destinations",
                "The number of destinations must be less than or equal to 100 when \
                 'RoutingBoundary.Unbounded' has value true.",
            )),
        ),
        (
            "a geometry boundary past the unbounded origin cap",
            16,
            1,
            geometry.clone(),
            None,
        ),
        (
            "a geometry boundary past its own origin cap",
            501,
            1,
            geometry.clone(),
            Some((
                "Origins",
                "The number of origins must be less than or equal to 500 when \
                 'RoutingBoundary.Geometry' is set.",
            )),
        ),
    ];

    for (case, origins, destinations, boundary, refused) in cases {
        let body = json!({
            "Origins": points(origins),
            "Destinations": points(destinations),
            "RoutingBoundary": boundary,
        });
        let (status, answer, _) = post(&router, MATRIX, &body).await;
        let Some((member, expected)) = refused else {
            assert_eq!(
                status, 200,
                "case {case:?}: a matrix within its caps must be served, got {status} with {answer:?}"
            );
            continue;
        };
        let read = refusal(case, status, &answer);
        assert_eq!(
            read.message, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            read.message
        );
        assert_eq!(
            read.field_name, member,
            "case {case:?}: expected the capped member {member:?}, got {:?}",
            read.field_name
        );
    }
}

/// A coordinate outside the WGS 84 domain is refused with the component, its two bounds
/// and the value the request set — wherever the position appears, and whichever component
/// is out. A coordinate ON a bound is inside the domain.
#[tokio::test]
async fn a_coordinate_outside_its_domain_is_refused_with_its_bounds() {
    let router = routes_service();
    let longitude = |value: f64| format!("Longitude must be between -180 and 180 degrees, but was set to {value}.");
    let latitude = |value: f64| format!("Latitude must be between -90 and 90 degrees, but was set to {value}.");
    // Rows span the operations that read a position, the member paths within them, both
    // components, both ends of each domain, and a position on each published bound.
    let cases: [CoordinateRow; 10] = [
        (
            "a route origin past the longitude maximum",
            ROUTES,
            json!({ "Origin": [200.0, 49.0], "Destination": GOOD }),
            Some(("Origin", longitude(200.0))),
        ),
        (
            "a route destination past the longitude minimum",
            ROUTES,
            json!({ "Origin": GOOD, "Destination": [-180.5, 49.0] }),
            Some(("Destination", longitude(-180.5))),
        ),
        (
            "a route waypoint past the latitude maximum",
            ROUTES,
            json!({ "Origin": GOOD, "Destination": GOOD, "Waypoints": [{ "Position": [-123.1, 91.0] }] }),
            Some(("Waypoints[0].Position", latitude(91.0))),
        ),
        (
            "a route on both longitude bounds",
            ROUTES,
            json!({ "Origin": [-180.0, 49.0], "Destination": [180.0, 49.0] }),
            None,
        ),
        (
            "a trace point past the longitude maximum",
            SNAP,
            json!({ "TracePoints": [{ "Position": GOOD }, { "Position": [200.0, 49.0] }] }),
            Some(("TracePoints[1].Position", longitude(200.0))),
        ),
        (
            "a trace point past the latitude minimum",
            SNAP,
            json!({ "TracePoints": [{ "Position": [-123.1, -90.25] }, { "Position": GOOD }] }),
            Some(("TracePoints[0].Position", latitude(-90.25))),
        ),
        (
            "a matrix destination past the longitude minimum",
            MATRIX,
            json!({
                "Origins": [{ "Position": GOOD }],
                "Destinations": [{ "Position": GOOD }, { "Position": [-181.0, 49.0] }]
            }),
            Some(("Destinations[1].Position", longitude(-181.0))),
        ),
        (
            "a matrix boundary circle centre past the latitude maximum",
            MATRIX,
            json!({
                "Origins": [{ "Position": GOOD }],
                "Destinations": [{ "Position": GOOD }],
                "RoutingBoundary": { "Geometry": { "Circle": { "Center": [-123.1, 90.5], "Radius": 1000.0 } } }
            }),
            Some(("RoutingBoundary.Geometry.Circle.Center", latitude(90.5))),
        ),
        (
            "an optimization waypoint past the longitude maximum",
            OPTIMIZE,
            json!({ "Origin": GOOD, "Waypoints": [{ "Id": "a", "Position": [180.75, 49.0] }] }),
            Some(("Waypoints[0].Position", longitude(180.75))),
        ),
        (
            "an optimization on both latitude bounds",
            OPTIMIZE,
            json!({ "Origin": [-123.1, -90.0], "Destination": [-123.1, 90.0] }),
            None,
        ),
    ];

    for (case, path, body, refused) in cases {
        let (status, answer, _) = post(&router, path, &body).await;
        let Some((member, expected)) = refused else {
            assert_eq!(
                status, 200,
                "case {case:?}: a coordinate on a bound is inside the domain, got {status} with {answer:?}"
            );
            continue;
        };
        let read = refusal(case, status, &answer);
        assert_eq!(
            read.message, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            read.message
        );
        assert_eq!(
            read.field_name, member,
            "case {case:?}: expected the member carrying the coordinate {member:?}, got {:?}",
            read.field_name
        );
    }
}

/// A value outside the range its member allows is refused by the bound it broke, naming
/// the member and that bound and nothing else; a value ON a bound is allowed.
///
/// The rows are the ranges this SERVICE enforces: a member that narrows the range of the
/// shape it targets (`SnapRadius` to 0..10000 of `DistanceMeters`, `StopDuration` to
/// ..49999 of `DurationSeconds`) and a member whose range is documented rather than modeled
/// (`Circle.Radius`, "Valid Range: Minimum value of 0. Maximum value of 200000"). A range
/// the model states on the targeted shape itself is refused by the framework's own
/// constraint walk before a handler runs, which words its own refusals.
#[tokio::test]
async fn a_value_outside_its_declared_range_is_refused_by_the_bound_it_broke() {
    let router = routes_service();
    let trace = json!([{ "Position": GOOD }, { "Position": [-123.11, 49.27] }]);
    let circle = |radius: f64| -> Value {
        json!({
            "Origins": [{ "Position": GOOD }],
            "Destinations": [{ "Position": GOOD }],
            "RoutingBoundary": { "Geometry": { "Circle": { "Center": GOOD, "Radius": radius } } }
        })
    };
    // Rows span the member, which of the two bounds was broken, an integral bound and a
    // floating-point one, and a value sitting exactly on the bound.
    let cases: [PathRow; 7] = [
        (
            "a snap radius past its maximum",
            SNAP,
            json!({ "TracePoints": trace.clone(), "SnapRadius": 10_001 }),
            Some(("SnapRadius", "'SnapRadius' must not be greater than 10000")),
        ),
        (
            "a snap radius on its maximum",
            SNAP,
            json!({ "TracePoints": trace.clone(), "SnapRadius": 10_000 }),
            None,
        ),
        (
            "a stop duration past its maximum",
            ROUTES,
            json!({
                "Origin": GOOD,
                "Destination": [-123.12, 49.26],
                "Waypoints": [{ "Position": [-123.11, 49.27], "StopDuration": 50_000 }]
            }),
            Some((
                "Waypoints[0].StopDuration",
                "'Waypoints[0].StopDuration' must not be greater than 49999",
            )),
        ),
        (
            "a stop duration on its maximum",
            ROUTES,
            json!({
                "Origin": GOOD,
                "Destination": [-123.12, 49.26],
                "Waypoints": [{ "Position": [-123.11, 49.27], "StopDuration": 49_999 }]
            }),
            None,
        ),
        (
            "a boundary circle wider than its documented maximum",
            MATRIX,
            circle(200_001.0),
            Some((
                "RoutingBoundary.Geometry.Circle.Radius",
                "'RoutingBoundary.Geometry.Circle.Radius' must not be greater than 200000",
            )),
        ),
        (
            "a boundary circle of negative radius",
            MATRIX,
            circle(-1.0),
            Some((
                "RoutingBoundary.Geometry.Circle.Radius",
                "'RoutingBoundary.Geometry.Circle.Radius' must not be less than 0",
            )),
        ),
        (
            "a boundary circle on its documented maximum",
            MATRIX,
            circle(200_000.0),
            None,
        ),
    ];

    for (case, path, body, refused) in cases {
        let (status, answer, _) = post(&router, path, &body).await;
        let Some((member, expected)) = refused else {
            assert_eq!(
                status, 200,
                "case {case:?}: a value on a bound is inside the range, got {status} with {answer:?}"
            );
            continue;
        };
        let read = refusal(case, status, &answer);
        assert_eq!(
            read.message, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            read.message
        );
        assert_eq!(
            read.field_name, member,
            "case {case:?}: expected the member whose range was broken {member:?}, got {:?}",
            read.field_name
        );
    }
}
