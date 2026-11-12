// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE resolution of a tagging operation's `ResourceArn` to the
//! stored tag set it names, driven over real HTTP through the same `MultiServiceRouter`
//! the emulator boots, so what is asserted is the wire behaviour.
//!
//! The rule under test: a well-formed MediaPackage resource ARN resolves to a stored tag
//! set only when the request's own Region holds a resource of that type under that id AND
//! that stored record carries exactly the ARN the caller sent — so an ARN naming another
//! Region, another account, another partition, another resource type or another id
//! resolves to nothing and is `NotFoundException` (404). The REPORTING path
//! (`ListTagsForResource`) and the WRITING path (`TagResource`, `UntagResource`) resolve
//! it identically, because they are the same rule: every row below asserts the three
//! operations agree about whether the ARN resolves, and that a request which does not
//! resolve leaves the resource's tag set exactly as it found it.
//!
//! Each row differs along what the rule generalises over: which ARN field is wrong
//! (partition, Region, account, resource type, id), by how much it is wrong (a different
//! value, or one that merely shares the stored value's prefix), which Region the request
//! itself is served in, and — every row on both — which of the two resource types this
//! service tags. An ARN that is not a MediaPackage channel or origin-endpoint ARN at all
//! fails BEFORE this resolution and is covered by `tests/all/resource_tags.rs`.

use serde_json::{json, Value};

// The crate's shared test client. `call_in` is the region-parameterised round trip, which
// this suite needs because the Region a request is served in is half of the rule.
use crate::channel_lifecycle::{
    call_in, create_channel, emulator, error_code, ACCOUNT, OTHER_ACCOUNT, REGION,
};
// The tagging request forms (the percent-encoded ARN path segment, the `tagKeys` query)
// and the per-resource-type fixtures live once, in the tagging suite.
use crate::resource_tags::{
    create_with_tags, described_tags, tags_path, untag_query, Kind, PARENT_CHANNEL,
};

/// The ARN resource-type segment of each type this service tags, as `state::channel_arn`
/// and `state::origin_endpoint_arn` mint it.
fn resource_type_of(kind: &Kind) -> &'static str {
    match kind {
        Kind::Channel => "channels",
        Kind::OriginEndpoint => "origin_endpoints",
    }
}

/// One resolution case: the ARN the tagging operations are addressed with, the Region the
/// request is served in, and whether that pair resolves to the stored resource.
///
/// Spelled as the ARN's FIELDS rather than as a whole string, so a row names exactly the
/// one thing it varies and the rest is the stored resource's own ARN.
struct Resolution {
    case: &'static str,
    partition: &'static str,
    /// The Region field of the ARN.
    arn_region: &'static str,
    account: &'static str,
    /// The ARN's resource type, or `None` for the type of the resource under test.
    resource_type: Option<&'static str>,
    /// Appended to the stored resource's id, so `""` is the resource's own id.
    id_suffix: &'static str,
    /// The Region the REQUEST is served in (its SigV4 credential scope).
    serving_region: &'static str,
    resolves: bool,
}

/// The stored resource's own ARN, requested in its own Region: the one row that resolves,
/// and the base every other row varies one field of.
const OWN: Resolution = Resolution {
    case: "the resource's own ARN in its own Region",
    partition: "aws",
    arn_region: REGION,
    account: ACCOUNT,
    resource_type: None,
    id_suffix: "",
    serving_region: REGION,
    resolves: true,
};

fn resolution_cases() -> Vec<Resolution> {
    vec![
        OWN,
        Resolution {
            case: "an ARN naming another Region",
            arn_region: "us-east-1",
            resolves: false,
            ..OWN
        },
        Resolution {
            case: "an ARN whose Region merely shares the stored Region's prefix",
            arn_region: "eu-west-2",
            resolves: false,
            ..OWN
        },
        Resolution {
            case: "an ARN naming another account",
            account: OTHER_ACCOUNT,
            resolves: false,
            ..OWN
        },
        Resolution {
            case: "an ARN of the China partition",
            partition: "aws-cn",
            resolves: false,
            ..OWN
        },
        Resolution {
            case: "an ARN of the GovCloud partition",
            partition: "aws-us-gov",
            resolves: false,
            ..OWN
        },
        Resolution {
            case: "an ARN naming the OTHER taggable resource type under this id",
            resource_type: Some("other"),
            resolves: false,
            ..OWN
        },
        Resolution {
            case: "an ARN naming an id this service does not hold",
            id_suffix: "-no-such-resource",
            resolves: false,
            ..OWN
        },
        Resolution {
            case: "an ARN whose id merely has the stored id as a prefix",
            id_suffix: "x",
            resolves: false,
            ..OWN
        },
        Resolution {
            case: "the resource's own ARN, requested in another Region",
            serving_region: "us-east-1",
            resolves: false,
            ..OWN
        },
        Resolution {
            case: "the resource's own ARN, requested in a Region sharing its prefix",
            serving_region: "eu-west-2",
            resolves: false,
            ..OWN
        },
        Resolution {
            case: "an ARN naming the serving Region, which holds no such resource",
            arn_region: "us-east-1",
            serving_region: "us-east-1",
            resolves: false,
            ..OWN
        },
    ]
}

/// The ARN a row addresses, for the resource `id` of `kind`. `Some("other")` as the row's
/// resource type means "the other type this service tags", which is how a row asks whether
/// the type segment is part of the resolution.
fn arn_under_test(kind: &Kind, row: &Resolution, id: &str) -> String {
    let own_type = resource_type_of(kind);
    let resource_type = match row.resource_type {
        None => own_type,
        Some("other") => match kind {
            Kind::Channel => resource_type_of(&Kind::OriginEndpoint),
            Kind::OriginEndpoint => resource_type_of(&Kind::Channel),
        },
        Some(explicit) => explicit,
    };
    format!(
        "arn:{}:mediapackage:{}:{}:{resource_type}/{id}{}",
        row.partition, row.arn_region, row.account, row.id_suffix
    )
}

/// The tag set a fresh fixture is created with, and the tag `TagResource` would add.
const STORED_KEY: &str = "team";
const STORED_VALUE: &str = "video";
const ADDED_KEY: &str = "env";
const ADDED_VALUE: &str = "prod";

/// The `Tags` map `ListTagsForResource` reported, with an absent or `null` member read as
/// the empty map.
fn reported_tags(body: &Value) -> Value {
    match body.get("tags") {
        None | Some(Value::Null) => json!({}),
        Some(value) => value.clone(),
    }
}

#[tokio::test]
async fn an_arn_resolves_the_same_way_for_the_reporting_and_the_writing_path() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;

    for kind in [Kind::Channel, Kind::OriginEndpoint] {
        for (index, row) in resolution_cases().into_iter().enumerate() {
            let case = format!("{}: {}", kind.label(), row.case);
            let id = format!("probe-{}-{index}", kind.label().replace(' ', "-"));
            create_with_tags(
                &router,
                &kind,
                &id,
                &json!({ STORED_KEY: STORED_VALUE }),
            )
            .await;
            let arn = arn_under_test(&kind, &row, &id);

            // The REPORTING path: the former read-only copy of the resolution.
            let (read_status, read_body) =
                call_in(&router, row.serving_region, "GET", &tags_path(&arn), None).await;
            // The WRITING path: the former read-write copy, driven by both operations
            // that reach it.
            let (tag_status, tag_body) = call_in(
                &router,
                row.serving_region,
                "POST",
                &tags_path(&arn),
                Some(&json!({ "tags": { ADDED_KEY: ADDED_VALUE } })),
            )
            .await;
            let (untag_status, untag_body) = call_in(
                &router,
                row.serving_region,
                "DELETE",
                &untag_query(&arn, &[STORED_KEY]),
                None,
            )
            .await;

            // One rule, so the three operations cannot disagree about whether this ARN
            // names a resource of this request's Region.
            let read_resolved = read_status == 200;
            let tag_resolved = tag_status == 204;
            let untag_resolved = untag_status == 204;
            assert_eq!(
                read_resolved, row.resolves,
                "case {case:?}: expected ListTagsForResource to resolve {}, got status \
                 {read_status} {read_body:?}",
                row.resolves
            );
            assert_eq!(
                tag_resolved, read_resolved,
                "case {case:?}: ListTagsForResource resolved {read_resolved} but TagResource \
                 resolved {tag_resolved}, statuses {read_status} and {tag_status} {tag_body:?}"
            );
            assert_eq!(
                untag_resolved, read_resolved,
                "case {case:?}: ListTagsForResource resolved {read_resolved} but UntagResource \
                 resolved {untag_resolved}, statuses {read_status} and {untag_status} \
                 {untag_body:?}"
            );

            if row.resolves {
                assert_eq!(
                    reported_tags(&read_body),
                    json!({ STORED_KEY: STORED_VALUE }),
                    "case {case:?}: expected the stored tag set, got {read_body:?}"
                );
            } else {
                // Every operation refuses the identical way, and the refusal is a
                // not-found about the resource type — not a malformed-member refusal.
                for (operation, status, body) in [
                    ("ListTagsForResource", read_status, &read_body),
                    ("TagResource", tag_status, &tag_body),
                    ("UntagResource", untag_status, &untag_body),
                ] {
                    assert_eq!(
                        status, 404,
                        "case {case:?}: expected {operation} to answer 404, got {status} {body:?}"
                    );
                    assert_eq!(
                        error_code(body),
                        "NotFoundException",
                        "case {case:?}: expected {operation} to answer NotFoundException, got \
                         {body:?}"
                    );
                }
            }

            // What the resolution admitted is what was written: a row that resolves has
            // been tagged and untagged, and one that does not is byte-for-byte the tag set
            // the create stored.
            let expected_after = if row.resolves {
                json!({ ADDED_KEY: ADDED_VALUE })
            } else {
                json!({ STORED_KEY: STORED_VALUE })
            };
            let (status, described) =
                call_in(&router, REGION, "GET", &kind.describe_path(&id), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the read-back expected 200, got {status} {described:?}"
            );
            assert_eq!(
                described_tags(&described),
                expected_after,
                "case {case:?}: expected the stored tag set {expected_after:?}, got {described:?}"
            );

            // And the reporting path renders that same stored map, in its own Region.
            let (status, listed) = call_in(
                &router,
                REGION,
                "GET",
                &tags_path(&arn_under_test(&kind, &OWN, &id)),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the tag read-back expected 200, got {status} {listed:?}"
            );
            assert_eq!(
                reported_tags(&listed),
                expected_after,
                "case {case:?}: ListTagsForResource and the Describe disagree, expected \
                 {expected_after:?}, got {listed:?}"
            );
        }
    }
}
