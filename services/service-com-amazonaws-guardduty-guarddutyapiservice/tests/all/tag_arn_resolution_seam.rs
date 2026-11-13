// SPDX-License-Identifier: CC0-1.0

//! The ONE `ResourceArn`-to-tag-map resolution the whole tagging surface shares,
//! driven end to end over the wire through an operation of each borrow kind:
//! `ListTagsForResource` (which only reports tags) and `TagResource`/`UntagResource`
//! (which write them).
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together. The resources come from the `resource_tag_removal` fixture and the write
//! helpers from the suites beside it, so this suite cannot disagree with them about
//! which ARN names which resource.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. A report and a write resolve the same ARN to the SAME tag map — the one kept on
//!    the resource's own record — for every class a tag ARN names: the detector, and
//!    each sub-resource segment `state::TAGGED_SUB_RESOURCE_SEGMENTS` names. So a key
//!    `TagResource` writes is the key the report hands back and the key the resource's
//!    own read reports, and a key `UntagResource` removes stops being reported by both.
//! 2. They agree about which ARNs resolve NOTHING: an ARN whose claimed partition,
//!    Region or account is not the one the endpoint serves, and a well-formed id of any
//!    class that nothing minted, are refused by the report and by both writes alike —
//!    HTTP 400 under the `InvalidInputException` class, under the one fixed sentence the
//!    tagging surface answers, which echoes nothing of the caller's and persists
//!    nothing.
//!
//! The one place the two halves may differ — a sub-resource whose delete has already
//! been accepted, which is still REPORTED but takes no write — is the
//! `deleted_resource_tag_writes` suite's subject, and is deliberately not re-asserted
//! here.

use serde_json::Value;

use crate::detector_lifecycle::{call_in, error_code, new_router, Router, ABSENT_DETECTOR_ID};
use crate::resource_tag_addition::tag_in;
use crate::resource_tag_removal::{
    arn_of, tag_map, tagged_resources, tags_of, untag_in, uri_label, Tagged,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::service::refusals::INVALID_RESOURCE_ARN_MESSAGE;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::TAGGED_SUB_RESOURCE_SEGMENTS;

/// The Region every request in this suite is signed for, unless a case names another.
const REGION: &str = "us-east-1";

/// A second Region, so a row can call one endpoint with another endpoint's ARN.
const OTHER_REGION: &str = "eu-west-1";

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// A well-formed minted id (32 hexadecimal characters) that no create ever minted.
const ABSENT_ID: &str = "0123456789abcdef0123456789abcdef";

/// A `FilterName` no create ever used, in the alphabet a filter name admits.
const ABSENT_FILTER_NAME: &str = "absent-filter";

/// The tags every resource in this suite is created with, so a write is observable as
/// a change to a map that already has entries, and a refusal as "this map did not
/// move". The value of one of them is the empty string a `TagValue` (`len 0..256` —
/// `smithy_query(shape="TagMap")`) may be.
const BASE_TAGS: [(&str, &str); 2] = [("Env", "prod"), ("Reviewed", "")];

/// `BASE_TAGS` as a request/response `TagMap`.
fn base_tags() -> Value {
    tag_map(&BASE_TAGS)
}

/// Report the tags of the resource `arn` names, through `region`'s endpoint.
///
/// `ListTagsForResource` is bound to `GET /tags/{ResourceArn}` with a NON-GREEDY URI
/// label (`smithy_query(operation="ListTagsForResource")`), the same binding the two
/// writers carry their ARN in.
async fn list_in(router: &Router, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/tags/{}", uri_label(arn)),
        None,
    )
    .await
}

/// The `tags` map the report hands back for `arn`, asserting it answered 200 and
/// reported the member at all.
async fn listed(router: &Router, region: &str, arn: &str, case: &str) -> Value {
    let (status, body) = list_in(router, region, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the report to answer 200, got {status} {body:?}"
    );
    let tags = body.get("tags").cloned().unwrap_or(Value::Null);
    assert!(
        tags.is_object(),
        "case {case:?}: the tags member is always reported, got {tags:?}"
    );
    tags
}

// ---------------------------------------------------------------------------
// Rule 1: one map, reached by both halves, for every class a tag ARN names.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_write_through_one_half_is_what_the_other_half_reports() {
    // How many classes a tag ARN names: the detector, plus one per sub-resource segment
    // the service resolves.
    let class_count = TAGGED_SUB_RESOURCE_SEGMENTS.len() + 1;

    for index in 0..class_count {
        // One store per class, so what each half reports is this class's own writes.
        let router = new_router();
        let (_, resources) = tagged_resources(&router, REGION, &base_tags()).await;
        assert_eq!(
            resources.len(),
            class_count,
            "every class a tag ARN names must be created here, got {}",
            resources.len()
        );
        let resource = &resources[index];
        let case = resource.case;
        let arn = resource.arn(REGION);

        // The report, before any write of this suite's: what the create stored.
        let expected = base_tags();
        let actual = listed(&router, REGION, &arn, case).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the created tags {expected:?}, got {actual:?}"
        );

        // The write half: a fresh key, and a new value for a key the resource carries.
        let written = tag_map(&[("Owner", "payments"), ("Env", "staging")]);
        let (status, body) = tag_in(&router, REGION, &arn, &written).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected the tag to answer 204, got {status} {body:?}"
        );

        // The read half reports exactly that, and so does the resource's own read:
        // both halves reached the one map kept on the record.
        let after_tag = tag_map(&[("Env", "staging"), ("Reviewed", ""), ("Owner", "payments")]);
        let reported = listed(&router, REGION, &arn, case).await;
        assert_eq!(
            reported, after_tag,
            "case {case:?}: expected the written tags {after_tag:?}, got {reported:?}"
        );
        let own = tags_of(&router, REGION, resource, case).await;
        assert_eq!(
            own, reported,
            "case {case:?}: the report and the resource's own read must agree, expected \
             {reported:?}, got {own:?}"
        );

        // And a removal through the write half stops being reported by the read half.
        let (status, body) = untag_in(&router, REGION, &arn, &["Owner", "Env"]).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected the untag to answer 204, got {status} {body:?}"
        );
        let after_untag = tag_map(&[("Reviewed", "")]);
        let reported = listed(&router, REGION, &arn, case).await;
        assert_eq!(
            reported, after_untag,
            "case {case:?}: expected the remaining tags {after_untag:?}, got {reported:?}"
        );
        let own = tags_of(&router, REGION, resource, case).await;
        assert_eq!(
            own, reported,
            "case {case:?}: the report and the resource's own read must agree, expected \
             {reported:?}, got {own:?}"
        );

        // No sibling's map moved: the ARN resolved ONE resource for both halves.
        let untouched = base_tags();
        for (other_index, other) in resources.iter().enumerate() {
            if other_index == index {
                continue;
            }
            let other_case = format!("{} while {case} is written", other.case);
            let actual = listed(&router, REGION, &other.arn(REGION), &other_case).await;
            assert_eq!(
                actual, untouched,
                "case {other_case:?}: a write elsewhere must change nothing here, expected \
                 {untouched:?}, got {actual:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an ARN one half resolves nothing for is refused by both.
// ---------------------------------------------------------------------------

/// One ARN the endpoint resolves no taggable resource for: the case, the Region whose
/// endpoint is called, and the ARN.
struct Unresolvable {
    case: String,
    region: &'static str,
    arn: String,
}

/// Every way an ARN can resolve nothing, as rows. They differ along what the rule
/// generalises over: each part of the scope the ARN claims (partition, Region, account),
/// which endpoint is called, and — for every class a tag ARN names — a well-formed id
/// of that class nothing minted.
fn unresolvable_rows(detector: &str) -> Vec<Unresolvable> {
    let row = |case: String, region: &'static str, arn: String| Unresolvable { case, region, arn };
    let mut rows = vec![
        row(
            "another account's detector".to_string(),
            REGION,
            format!("arn:aws:guardduty:{REGION}:111122223333:detector/{detector}"),
        ),
        row(
            "another partition's detector".to_string(),
            REGION,
            format!("arn:aws-cn:guardduty:{REGION}:000000000000:detector/{detector}"),
        ),
        row(
            "an ARN claiming another Region".to_string(),
            REGION,
            arn_of(OTHER_REGION, &format!("detector/{detector}")),
        ),
        row(
            "this Region's ARN at another Region's endpoint".to_string(),
            OTHER_REGION,
            arn_of(REGION, &format!("detector/{detector}")),
        ),
        row(
            "a detectorId no create minted".to_string(),
            REGION,
            arn_of(REGION, &format!("detector/{ABSENT_DETECTOR_ID}")),
        ),
        row(
            "a sub-resource type GuardDuty does not tag".to_string(),
            REGION,
            arn_of(REGION, &format!("detector/{detector}/widget/{ABSENT_ID}")),
        ),
    ];

    // One row per sub-resource class, naming an id of that class the detector does not
    // hold. The catalogue is `state::TAGGED_SUB_RESOURCE_SEGMENTS`, so a seventh
    // taggable class is covered here the moment it is resolved at all.
    for (segment, _) in TAGGED_SUB_RESOURCE_SEGMENTS {
        // A filter is named by its `FilterName` rather than by a minted id.
        let id = if segment == "filter" {
            ABSENT_FILTER_NAME
        } else {
            ABSENT_ID
        };
        rows.push(row(
            format!("a {segment:?} id the detector does not hold"),
            REGION,
            arn_of(REGION, &format!("detector/{detector}/{segment}/{id}")),
        ));
    }
    rows
}

/// Assert one answer is the refusal a well-formed ARN this endpoint resolves nothing
/// for gets, echoing nothing of the caller's.
fn assert_refused(case: &str, arn: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body:?}"
    );
    let class = error_code(body);
    assert_eq!(
        class, INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, INVALID_RESOURCE_ARN_MESSAGE,
        "case {case:?}: expected {INVALID_RESOURCE_ARN_MESSAGE:?}, got {message:?}"
    );
    assert!(
        !message.contains(arn),
        "case {case:?}: the refusal must not echo {arn:?}, got {message:?}"
    );
}

#[tokio::test]
async fn both_halves_refuse_the_same_unresolvable_arns() {
    let router = new_router();
    let (detector, resources) = tagged_resources(&router, REGION, &base_tags()).await;

    for Unresolvable { case, region, arn } in unresolvable_rows(&detector) {
        // The reporting half.
        let (status, body) = list_in(&router, region, &arn).await;
        assert_refused(&format!("listing {case}"), &arn, status, &body);

        // The writing half, through both of its operations.
        let (status, body) = tag_in(&router, region, &arn, &tag_map(&[("Owner", "me")])).await;
        assert_refused(&format!("tagging {case}"), &arn, status, &body);

        let (status, body) = untag_in(&router, region, &arn, &["Env"]).await;
        assert_refused(&format!("untagging {case}"), &arn, status, &body);
    }

    // None of them persisted anything, and every live resource still resolves for both
    // halves.
    let untouched = base_tags();
    for resource in &resources {
        let case = resource.case;
        let reported = listed(&router, REGION, &resource.arn(REGION), case).await;
        assert_eq!(
            reported, untouched,
            "case {case:?}: a refused call must change nothing, expected {untouched:?}, got \
             {reported:?}"
        );
        let own = tags_of(&router, REGION, resource, case).await;
        assert_eq!(
            own, untouched,
            "case {case:?}: expected {untouched:?}, got {own:?}"
        );
    }
}

/// Every class a tag ARN names is driven above: the fixture builds one resource per
/// `state::TAGGED_SUB_RESOURCE_SEGMENTS` entry plus the detector itself, and the
/// unresolvable table carries a row for each of those segments — so a seventh taggable
/// class fails here rather than passing silently.
#[tokio::test]
async fn every_tagged_class_is_driven_through_both_halves() {
    let router = new_router();
    let (detector, resources) = tagged_resources(&router, REGION, &base_tags()).await;

    assert_eq!(
        resources.len(),
        TAGGED_SUB_RESOURCE_SEGMENTS.len() + 1,
        "every class a tag ARN names must be created here, got {} for {} segments",
        resources.len(),
        TAGGED_SUB_RESOURCE_SEGMENTS.len()
    );

    let rows = unresolvable_rows(&detector);
    for (segment, _) in TAGGED_SUB_RESOURCE_SEGMENTS {
        let created: Vec<&Tagged> = resources
            .iter()
            .filter(|resource| resource.path.contains(&format!("/{segment}/")))
            .collect();
        assert_eq!(
            created.len(),
            1,
            "case {segment:?}: exactly one resource must be named through this segment, got {}",
            created.len()
        );
        let absent = rows
            .iter()
            .filter(|row| row.arn.contains(&format!("/{segment}/")))
            .count();
        assert_eq!(
            absent, 1,
            "case {segment:?}: exactly one unresolvable row must name this segment, got {absent}"
        );
    }

    // The detector itself carries no sub-resource segment at all.
    let detectors = resources
        .iter()
        .filter(|resource| resource.path.split('/').count() == 2)
        .count();
    assert_eq!(
        detectors, 1,
        "exactly one created resource is the detector itself, got {detectors}"
    );
}
