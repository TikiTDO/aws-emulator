// SPDX-License-Identifier: CC0-1.0

//! How `TagResource` STATES the refusal of a write that would leave a resource
//! carrying more tag pairs than the model's `TagMap` maximum.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The harness
//! (`router`/`call`/…) is the one `monitor_and_probe` owns, and the tagging
//! requests are the ones `resource_tags` owns.
//!
//! THE RULE: the per-resource tag maximum is a MODELLED `@length` bound the
//! handler applies itself — to the MERGED set the write would store rather than
//! to the request's own map — so a write past it is refused with HTTP 400
//! `ValidationException` stated in AWS's constraint-violation construction, like
//! every other refusal this service authors (see `authored_refusal_messages`):
//! the envelope, the member path the caller sent the map at, the collection's
//! entry count as the value a clause echoes for a map, and the published
//! requirement for a maximum length. A write whose merged set is at or under the
//! bound is accepted, so the rule is about the bound and not about refusing.
//!
//! WHERE the count is judged decides which refusal answers: a REQUEST carrying
//! more pairs than the maximum breaks a modelled constraint on the body, which
//! this service refuses at its front door (`BadRequestException`, see
//! `monitor_lifecycle_and_front_door`), while the MERGED set is a rule only the
//! handler can apply and therefore the one it states itself. The last case pins
//! that split.
//!
//! The table spans the class the rule covers: how far past the bound the merged
//! set lands (one pair, two, many), which resource kind the model's `resourceArn`
//! references (a monitor and a probe), and whether the request mixes keys the
//! resource already carries with new ones.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{error_code, error_message, router};
use super::resource_tags::{
    fixture, list_tags, tag_resource, Target, MAX_TAGS, TAG_MUTATION_STATUS,
};

/// The envelope AWS wraps a single constraint clause in.
const ONE_ERROR: &str = "1 validation error detected: ";

/// The error code this service's front door names for a request BODY that
/// breaks a modelled constraint, and the one sentence it states — the behaviour
/// `monitor_lifecycle_and_front_door` pins for every modelled constraint on a
/// body value.
const FRONT_DOOR_CODE: &str = "BadRequestException";
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// The requirement AWS publishes for a value past a `@length` maximum, which is
/// the one the framework's own clause builder renders for that constraint
/// (`aws_smithy_schema_server::constraints`).
fn max_length_requirement(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// How a clause echoes a MAP whose entry count broke a bound: the framework's
/// own rendering for a map (`<n> entries`, beside a list's `<n> items`), which is
/// what a caller reads back for a map member — no caller sent this text, because
/// a map has no scalar spelling.
fn entries(count: usize) -> String {
    format!("{count} entries")
}

/// The whole message a refused tag count is stated with.
///
/// `pub(crate)`, because the ceiling is one rule over every operation that
/// composes a resource's tag map out of more than one map, so the suite that
/// pins it for `CreateProbe`'s union (`create_probe_tag_union_ceiling`) reads
/// the sentence from here rather than keeping a copy that could drift.
pub(crate) fn one_error(count: usize) -> String {
    format!(
        "{ONE_ERROR}Value '{}' at 'tags' failed to satisfy constraint: {}",
        entries(count),
        max_length_requirement(MAX_TAGS)
    )
}

/// `count` distinct pairs whose keys are the ones a fixture's stored tags use,
/// so a request built from this map OVERWRITES rather than adds.
fn stored_keys(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("key-{index:03}")).collect()
}

/// `count` pairs no fixture stores, so each one a request names is a key the
/// resource does not carry yet.
fn new_pairs(count: usize) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(format!("new-{index:03}"), json!("v"));
    }
    Value::Object(map)
}

/// A JSON map of `count` stored pairs — what a fixture is created carrying.
fn stored_pairs(count: usize) -> Value {
    let mut map = Map::new();
    for key in stored_keys(count) {
        map.insert(key, json!("v"));
    }
    Value::Object(map)
}

/// One row: what it exercises, how many pairs the target already carries, which
/// resource kind the call addresses, and the pairs it writes.
struct Case {
    case: &'static str,
    stored: usize,
    target: Target,
    written: Value,
}

/// Create a fixture whose addressed resource carries `stored` pairs, and answer
/// the ARN of the resource the row addresses beside the tags it carries.
async fn fixture_for(
    router: &MultiServiceRouter,
    name: &str,
    stored: usize,
    target: Target,
) -> (String, Value) {
    let carried = stored_pairs(stored);
    let (monitor_arn, probe_arn) = match target {
        Target::Monitor => fixture(router, name, &carried, &json!({"role": "edge"})).await,
        Target::Probe => fixture(router, name, &json!({"team": "net"}), &carried).await,
    };
    let arn = match target {
        Target::Monitor => monitor_arn,
        Target::Probe => probe_arn,
    };
    (arn, carried)
}

/// The tags `ListTagsForResource` reports for one ARN.
async fn stored_tags(router: &MultiServiceRouter, resource_arn: &str) -> Value {
    let (status, listed) = list_tags(router, resource_arn).await;
    assert_eq!(
        status, 200,
        "expected ListTagsForResource 200 for {resource_arn}, got {status} {listed:?}"
    );
    listed.get("tags").cloned().unwrap_or(Value::Null)
}

/// How many pairs `ListTagsForResource` reports for one ARN.
async fn stored_count(router: &MultiServiceRouter, resource_arn: &str) -> usize {
    stored_tags(router, resource_arn)
        .await
        .as_object()
        .map_or(0, Map::len)
}

/// A write whose merged set would pass the maximum is refused with the
/// constraint clause for that bound, naming the member the caller sent and the
/// count the resource would have been left with — and stores nothing.
#[tokio::test]
async fn a_write_past_the_tag_map_maximum_is_stated_as_the_bound_it_broke() {
    // Each row's merged count is `stored` plus the keys it names that the
    // resource does not carry yet.
    let cases: Vec<Case> = vec![
        Case {
            case: "the reported case: at the bound on a monitor, one new key",
            stored: MAX_TAGS,
            target: Target::Monitor,
            written: new_pairs(1),
        },
        Case {
            case: "at the bound on a probe, one new key",
            stored: MAX_TAGS,
            target: Target::Probe,
            written: new_pairs(1),
        },
        Case {
            case: "one under the bound, two new keys",
            stored: MAX_TAGS - 1,
            target: Target::Monitor,
            written: new_pairs(2),
        },
        Case {
            case: "well under the bound, a request of many new keys",
            stored: MAX_TAGS - 40,
            target: Target::Probe,
            written: new_pairs(60),
        },
        Case {
            case: "two under the bound, three new keys beside one already stored",
            stored: MAX_TAGS - 2,
            target: Target::Monitor,
            written: {
                let mut map = new_pairs(3);
                if let Value::Object(pairs) = &mut map {
                    pairs.insert(stored_keys(1)[0].clone(), json!("overwritten"));
                }
                map
            },
        },
        Case {
            case: "half the maximum stored, a request of half the maximum again",
            stored: MAX_TAGS / 2 + 1,
            target: Target::Probe,
            written: new_pairs(MAX_TAGS / 2),
        },
    ];

    let router = router();
    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        let (arn, carried) =
            fixture_for(&router, &format!("tag-ceiling-{index}"), row.stored, row.target).await;
        let requested = row
            .written
            .as_object()
            .expect("a row writes a tag map")
            .clone();
        let already = carried.as_object().expect("a fixture stores a tag map");
        let merged = row.stored
            + requested
                .keys()
                .filter(|key| !already.contains_key(*key))
                .count();
        assert!(
            merged > MAX_TAGS,
            "case {case:?}: the row must land past the maximum, got a merged count of {merged}"
        );

        let (status, refused) = tag_resource(&router, &arn, &row.written).await;
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
        // A refused write stores none of the pairs it carried: the resource is
        // left with exactly the tags it was created with.
        let listed = stored_tags(&router, &arn).await;
        assert_eq!(
            listed, carried,
            "case {case:?}: the refused write must persist nothing, got {listed:?}"
        );
    }
}

/// A write whose MERGED set lands at or under the maximum is accepted, whatever
/// the request's own size: the bound is on what the resource is left carrying.
///
/// The rows differ along how the merge stays inside the bound — overwriting keys
/// already stored at the bound, filling the last free slot, overwriting many
/// keys at once, and a request that fills an empty resource to exactly the
/// maximum.
#[tokio::test]
async fn a_write_whose_merged_set_is_within_the_maximum_is_stored() {
    // (case, pairs already stored, the resource kind, the pairs written, the
    // count the resource is left carrying)
    let cases: Vec<(&str, usize, Target, Value, usize)> = vec![
        (
            "at the bound, overwriting one key already stored",
            MAX_TAGS,
            Target::Monitor,
            {
                let mut map = Map::new();
                map.insert(stored_keys(1)[0].clone(), json!("overwritten"));
                Value::Object(map)
            },
            MAX_TAGS,
        ),
        (
            "at the bound on a probe, overwriting ten keys already stored",
            MAX_TAGS,
            Target::Probe,
            {
                let mut map = Map::new();
                for key in stored_keys(10) {
                    map.insert(key, json!("overwritten"));
                }
                Value::Object(map)
            },
            MAX_TAGS,
        ),
        (
            "one under the bound, one new key fills the last slot",
            MAX_TAGS - 1,
            Target::Monitor,
            new_pairs(1),
            MAX_TAGS,
        ),
        (
            "one under the bound, one new key beside an overwrite",
            MAX_TAGS - 1,
            Target::Probe,
            {
                let mut map = new_pairs(1);
                if let Value::Object(pairs) = &mut map {
                    pairs.insert(stored_keys(1)[0].clone(), json!("overwritten"));
                }
                map
            },
            MAX_TAGS,
        ),
        (
            "nothing stored, a request of exactly the maximum",
            0,
            Target::Monitor,
            new_pairs(MAX_TAGS),
            MAX_TAGS,
        ),
    ];

    let router = router();
    for (index, (case, stored, target, written, want_count)) in cases.iter().enumerate() {
        let (arn, _) = fixture_for(
            &router,
            &format!("tag-within-{index}"),
            *stored,
            *target,
        )
        .await;
        let (status, body) = tag_resource(&router, &arn, written).await;
        assert_eq!(
            status, TAG_MUTATION_STATUS,
            "case {case:?}: expected TagResource {TAG_MUTATION_STATUS}, got {status} {body:?}"
        );
        let held = stored_count(&router, &arn).await;
        assert_eq!(
            held, *want_count,
            "case {case:?}: expected the resource to be left carrying {want_count} pairs, got \
             {held}"
        );
        // The pairs the call named are what a later read reports.
        let listed = stored_tags(&router, &arn).await;
        for (key, value) in written.as_object().expect("a row writes a tag map") {
            assert_eq!(
                listed.get(key),
                Some(value),
                "case {case:?}: expected the stored tags to carry {key:?}={value:?}, got \
                 {listed:?}"
            );
        }
    }
}

/// WHERE the count is judged decides which refusal answers: the REQUEST's own
/// map is a modelled constraint on the body, which this service refuses at its
/// front door, while the MERGED set is the handler's own rule, stated as the
/// clause for the bound.
#[tokio::test]
async fn the_request_is_judged_at_the_front_door_and_the_merge_by_the_handler() {
    let router = router();

    // The body breaks the modelled `TagMap` maximum, so the front door answers
    // and the handler never runs.
    let (front_door_arn, _) = fixture_for(&router, "tag-bound-body", 0, Target::Monitor).await;
    let (status, refused) = tag_resource(&router, &front_door_arn, &new_pairs(MAX_TAGS + 1)).await;
    assert_eq!(
        status, 400,
        "expected the body refusal to be HTTP 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        FRONT_DOOR_CODE,
        "expected the body past the modelled maximum to be a front-door rejection, got \
         {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        INVALID_REQUEST_BODY,
        "expected the front door's own sentence, got {refused:?}"
    );
    // Nothing was stored by the refused call.
    assert_eq!(
        stored_count(&router, &front_door_arn).await,
        0,
        "expected the refused body to store nothing"
    );

    // The request here is one pair, well inside the modelled maximum; the set
    // the merge would store is not, which is the handler's rule to state.
    let (merged_arn, _) = fixture_for(&router, "tag-bound-merge", MAX_TAGS, Target::Monitor).await;
    let (status, authored) = tag_resource(&router, &merged_arn, &new_pairs(1)).await;
    assert_eq!(
        status, 400,
        "expected the merged refusal to be HTTP 400, got {status} {authored:?}"
    );
    assert_eq!(
        error_code(&authored),
        "ValidationException",
        "expected the handler's own refusal to be the modelled ValidationException, got \
         {authored:?}"
    );
    let message = error_message(&authored);
    assert_eq!(
        message,
        one_error(MAX_TAGS + 1),
        "expected AWS's constraint construction for the bound, got {message:?}"
    );
}
