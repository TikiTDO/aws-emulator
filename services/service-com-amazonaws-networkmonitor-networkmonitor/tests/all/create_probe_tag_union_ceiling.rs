// SPDX-License-Identifier: CC0-1.0

//! What tag map `CreateProbe` leaves on the probe it mints, when the request
//! assigns tags through BOTH members that carry them.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The harness
//! (`router`/`call`/…) is the one `monitor_and_probe` owns, the tagging requests
//! are `resource_tags`', and the probe read is `probe_teardown`'s.
//!
//! THE RULE: the per-resource tag maximum bounds the ONE tag map a write leaves
//! on a resource, so an operation that composes that map out of more than one
//! map judges the MERGED set against the maximum — not each map on its own. Here
//! the two maps are both the request's (`tags` and `probe.tags`), each inside the
//! modelled `TagMap` `@length(min: 0, max: 200)` that the front door applies to
//! a member, so their union is a count only the handler can judge: a union past
//! the maximum is refused with HTTP 400 `ValidationException` stated in AWS's
//! constraint-violation construction (the same clause the merged set is refused
//! with through `TagResource`, see `tag_count_ceiling_refusal`) and mints no
//! probe, while a union at or under it is stored whole and is what every later
//! read of the probe reports.
//!
//! Two consequences of the stored map obeying the bound are pinned beside it: no
//! read can report more pairs than the modelled `TagMap` admits, and a probe the
//! service created is still taggable — a later `TagResource` that only
//! overwrites keys the probe already carries adds nothing and is accepted.
//!
//! The table spans the class the rule covers: how the union is divided between
//! the two members (all in one, split evenly, lopsided, neither), whether the
//! halves share keys (an overlapping key is one pair, not two), and where the
//! union lands relative to the maximum (far under, exactly at it, one past, far
//! past).

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{
    create_monitor, create_probe, error_code, error_message, get_monitor, probes_of, router,
    source_arn, string_at,
};
use super::probe_teardown::get_probe;
use super::resource_tags::{list_tags, tag_resource, MAX_TAGS, TAG_MUTATION_STATUS};
use super::tag_count_ceiling_refusal::one_error;

/// `count` distinct pairs whose keys carry `prefix`, so two maps built with
/// different prefixes share no key and two built with the same prefix overlap.
fn pairs(prefix: &str, count: usize) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(format!("{prefix}-{index:03}"), json!(format!("{prefix}-v{index:03}")));
    }
    Value::Object(map)
}

/// A JSON map from explicit pairs, for a row whose keys are chosen one by one.
fn exact(entries: &[(&str, &str)]) -> Value {
    Value::Object(
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(*value)))
            .collect(),
    )
}

/// The pairs of a row's map, or none for a row whose member is absent.
fn entries_of(map: &Value) -> Map<String, Value> {
    map.as_object().cloned().unwrap_or_default()
}

/// The map the probe ends up carrying if the request is accepted: the union of
/// the two the request sent, with the one nested in `probe` winning a key both
/// name.
fn union_of(request_tags: &Value, probe_tags: &Value) -> Value {
    let mut merged = entries_of(request_tags);
    merged.extend(entries_of(probe_tags));
    Value::Object(merged)
}

/// The `CreateProbe` body: a valid probe, plus whichever of the two tag members
/// the row carries (`Value::Null` means the member is absent).
fn create_probe_body(request_tags: &Value, probe_tags: &Value) -> String {
    let mut probe = Map::new();
    probe.insert("sourceArn".to_string(), json!(source_arn()));
    probe.insert("destination".to_string(), json!("10.0.0.9"));
    probe.insert("protocol".to_string(), json!("ICMP"));
    probe.insert("packetSize".to_string(), json!(56));
    if !probe_tags.is_null() {
        probe.insert("tags".to_string(), probe_tags.clone());
    }
    let mut body = Map::new();
    body.insert("probe".to_string(), Value::Object(probe));
    if !request_tags.is_null() {
        body.insert("tags".to_string(), request_tags.clone());
    }
    Value::Object(body).to_string()
}

/// One row: what it exercises, the request's `tags`, the request's `probe.tags`,
/// and whether the union they compose is past the maximum.
struct Case {
    case: &'static str,
    request_tags: Value,
    probe_tags: Value,
    refused: bool,
}

/// The rows, in one table both tests walk: each is judged by whether its union
/// is past the maximum, not by which member carried what.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "the reported case: the maximum in each member, every key distinct",
            request_tags: pairs("req", MAX_TAGS),
            probe_tags: pairs("prb", MAX_TAGS),
            refused: true,
        },
        Case {
            case: "one past the maximum: the maximum in tags, one new key in probe.tags",
            request_tags: pairs("req", MAX_TAGS),
            probe_tags: pairs("prb", 1),
            refused: true,
        },
        Case {
            case: "well past: a little over half the maximum in each member",
            request_tags: pairs("req", MAX_TAGS / 2 + 20),
            probe_tags: pairs("prb", MAX_TAGS / 2 + 20),
            refused: true,
        },
        Case {
            case: "an overlapping key counts once: the maximum in tags, probe.tags \
                   overwriting one of them beside one new key",
            request_tags: pairs("key", MAX_TAGS),
            probe_tags: exact(&[("key-199", "overwritten"), ("prb-000", "new")]),
            refused: true,
        },
        Case {
            case: "exactly the maximum, split evenly across the two members",
            request_tags: pairs("req", MAX_TAGS / 2),
            probe_tags: pairs("prb", MAX_TAGS / 2),
            refused: false,
        },
        Case {
            case: "exactly the maximum: the maximum in tags, probe.tags only overwriting \
                   keys it already names",
            request_tags: pairs("key", MAX_TAGS),
            probe_tags: exact(&[
                ("key-000", "overwritten"),
                ("key-100", "overwritten"),
                ("key-199", "overwritten"),
            ]),
            refused: false,
        },
        Case {
            case: "one under the maximum in tags, one new key in probe.tags fills the last slot",
            request_tags: pairs("req", MAX_TAGS - 1),
            probe_tags: pairs("prb", 1),
            refused: false,
        },
        Case {
            case: "only probe.tags, at the maximum: tags is absent",
            request_tags: Value::Null,
            probe_tags: pairs("prb", MAX_TAGS),
            refused: false,
        },
        Case {
            case: "only tags, well under the maximum: probe.tags is absent",
            request_tags: pairs("req", 3),
            probe_tags: Value::Null,
            refused: false,
        },
        Case {
            case: "a handful in each member",
            request_tags: exact(&[("team", "net"), ("tier", "edge")]),
            probe_tags: exact(&[("owner", "probe"), ("tier", "probe-wins")]),
            refused: false,
        },
        Case {
            case: "neither member carries tags",
            request_tags: Value::Null,
            probe_tags: Value::Null,
            refused: false,
        },
    ]
}

/// A monitor of this row's own, so no row is judged on the probes another left
/// behind.
async fn monitor_for(router: &MultiServiceRouter, name: &str) {
    let (status, created) =
        create_monitor(router, &format!(r#"{{"monitorName":"{name}"}}"#)).await;
    assert_eq!(
        status, 200,
        "setup CreateMonitor {name:?}: expected 200, got {status} {created:?}"
    );
}

/// The tags `ListTagsForResource` reports for one ARN.
async fn listed_tags(router: &MultiServiceRouter, resource_arn: &str) -> Value {
    let (status, listed) = list_tags(router, resource_arn).await;
    assert_eq!(
        status, 200,
        "expected ListTagsForResource 200 for {resource_arn}, got {status} {listed:?}"
    );
    listed.get("tags").cloned().unwrap_or(Value::Null)
}

/// A request whose two tag maps compose a union past the maximum is refused with
/// the clause for that bound, naming the merged count the probe would have been
/// left with — and mints no probe at all.
#[tokio::test]
async fn a_tag_union_past_the_maximum_is_refused_and_mints_nothing() {
    let router = router();
    for (index, row) in cases().iter().filter(|row| row.refused).enumerate() {
        let case = row.case;
        let merged = entries_of(&union_of(&row.request_tags, &row.probe_tags)).len();
        assert!(
            merged > MAX_TAGS,
            "case {case:?}: the row must land past the maximum, got a union of {merged} pairs"
        );
        let name = format!("union-past-{index}");
        monitor_for(&router, &name).await;

        let (status, refused) = create_probe(
            &router,
            &name,
            &create_probe_body(&row.request_tags, &row.probe_tags),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
        let expected = one_error(merged);
        let message = error_message(&refused);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains('"'),
            "case {case:?}: a refusal must carry no Rust Debug quoting, got {message:?}"
        );

        // The refused create minted nothing: the monitor holds no probe, and it
        // still accepts one whose union is inside the maximum.
        let (status, got) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup GetMonitor should succeed, got {status} {got:?}"
        );
        let held = probes_of(&got);
        assert!(
            held.is_empty(),
            "case {case:?}: the refused create must mint no probe, got {held:?}"
        );
        let accepted = create_probe_body(&pairs("req", 1), &pairs("prb", 1));
        let (status, created) = create_probe(&router, &name, &accepted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a later create inside the maximum must be accepted, got {status} \
             {created:?}"
        );
    }
}

/// A request whose union is at or under the maximum stores that union whole: it
/// is what the create answers, what every later read of the probe reports, and a
/// map no read has to report more pairs of than the model admits — so the probe
/// is still taggable by a write that only overwrites keys it carries.
#[tokio::test]
async fn a_tag_union_within_the_maximum_is_stored_whole_and_read_back() {
    let router = router();
    for (index, row) in cases().iter().filter(|row| !row.refused).enumerate() {
        let case = row.case;
        let expected = union_of(&row.request_tags, &row.probe_tags);
        let merged = entries_of(&expected).len();
        assert!(
            merged <= MAX_TAGS,
            "case {case:?}: the row must stay inside the maximum, got a union of {merged} pairs"
        );
        let name = format!("union-within-{index}");
        monitor_for(&router, &name).await;

        let (status, created) = create_probe(
            &router,
            &name,
            &create_probe_body(&row.request_tags, &row.probe_tags),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateProbe 200, got {status} {created:?}"
        );
        assert_eq!(
            created.get("tags"),
            Some(&expected),
            "case {case:?}: expected the created probe to carry {expected:?}, got {created:?}"
        );
        let probe_id = string_at(&created, "probeId");
        let probe_arn = string_at(&created, "probeArn");

        // The same stored map answers every read path.
        let (status, got) = get_probe(&router, &name, &probe_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetProbe 200, got {status} {got:?}"
        );
        assert_eq!(
            got.get("tags"),
            Some(&expected),
            "case {case:?}: GetProbe must report the stored union, got {got:?}"
        );
        let listed = listed_tags(&router, &probe_arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: ListTagsForResource must report the stored union, got {listed:?}"
        );

        // Whatever the request sent, no read reports more pairs than the
        // modelled `TagMap` admits.
        assert!(
            merged <= MAX_TAGS,
            "case {case:?}: a read reported {merged} pairs, past the modelled maximum"
        );

        // And the probe the service created is still taggable: a write that only
        // overwrites a key it already carries adds nothing, so it is accepted and
        // leaves the count where it was.
        if let Some(key) = entries_of(&expected).keys().next().cloned() {
            let overwrite = exact(&[(key.as_str(), "overwritten")]);
            let (status, body) = tag_resource(&router, &probe_arn, &overwrite).await;
            assert_eq!(
                status, TAG_MUTATION_STATUS,
                "case {case:?}: expected an overwrite of {key:?} to answer \
                 {TAG_MUTATION_STATUS}, got {status} {body:?}"
            );
            let after = entries_of(&listed_tags(&router, &probe_arn).await);
            assert_eq!(
                after.len(),
                merged,
                "case {case:?}: expected the overwrite to leave {merged} pairs, got {}",
                after.len()
            );
            assert_eq!(
                after.get(&key),
                Some(&json!("overwritten")),
                "case {case:?}: expected {key:?} to carry the overwritten value, got {after:?}"
            );
        }
    }
}
