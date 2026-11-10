// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the tag surface — `TagResource`, `ListTagsForResource`
//! and `UntagResource` — on every resource kind Network Firewall tags.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.
//!
//! The tests are tables: each row is one input in the class the rule covers, and
//! every assertion names its row and the actual value.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_networkfirewall_networkfirewall_20201112::state;

use crate::harness::{error_code, error_message, Harness, ACCOUNT, NOT_FOUND_MESSAGE, REGION};

/// One tag write in a sequence: the operation and the tags or keys it names.
#[derive(Debug)]
enum TagWrite {
    /// `TagResource` with these key:value pairs.
    Add(Vec<(String, String)>),
    /// `UntagResource` with these keys.
    Remove(Vec<String>),
}

/// A resource this service tags: how to create one, and where its own reader
/// reports the tags it carries.
#[derive(Debug)]
pub struct TaggableKind {
    /// Names the row in every assertion.
    pub label: &'static str,
    /// The `X-Amz-Target` of the resource's own describe operation.
    pub describe: &'static str,
    /// The request that describe takes.
    pub describe_request: Value,
    /// Where the described body carries the resource's `Tags`.
    pub tags_path: &'static [&'static str],
    /// The resource's ARN, which every tag operation names it by.
    pub arn: String,
}

/// The four resource kinds this emulator tags, each created empty: a firewall, a
/// firewall policy and a rule group of each half of the service
/// ("firewalls, firewall policies, and rule groups", `TagResource`).
pub async fn taggable_kinds(harness: &Harness, suffix: &str) -> Vec<TaggableKind> {
    let policy_name = format!("tagged-policy-{suffix}");
    let policy_arn = harness.shared_policy(&policy_name).await;
    let firewall_name = format!("tagged-firewall-{suffix}");
    let created = harness
        .ok(
            "CreateFirewall",
            &json!({
                "FirewallName": firewall_name,
                "FirewallPolicyArn": policy_arn,
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": "subnet-0123abcd" }],
            }),
        )
        .await;
    let firewall_arn = created["Firewall"]["FirewallArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a FirewallArn in {created:?}"))
        .to_string();

    let stateless_name = format!("tagged-stateless-{suffix}");
    let stateless_arn = harness
        .shared_rule_group(&stateless_name, "STATELESS", 10)
        .await;
    let stateful_name = format!("tagged-stateful-{suffix}");
    let stateful_arn = harness
        .shared_rule_group(&stateful_name, "STATEFUL", 10)
        .await;

    vec![
        TaggableKind {
            label: "firewall",
            describe: "DescribeFirewall",
            describe_request: json!({ "FirewallName": firewall_name }),
            tags_path: &["Firewall", "Tags"],
            arn: firewall_arn,
        },
        TaggableKind {
            label: "firewall-policy",
            describe: "DescribeFirewallPolicy",
            describe_request: json!({ "FirewallPolicyName": policy_name }),
            tags_path: &["FirewallPolicyResponse", "Tags"],
            arn: policy_arn,
        },
        TaggableKind {
            label: "stateless-rule-group",
            describe: "DescribeRuleGroup",
            describe_request: json!({ "RuleGroupName": stateless_name, "Type": "STATELESS" }),
            tags_path: &["RuleGroupResponse", "Tags"],
            arn: stateless_arn,
        },
        TaggableKind {
            label: "stateful-rule-group",
            describe: "DescribeRuleGroup",
            describe_request: json!({ "RuleGroupName": stateful_name, "Type": "STATEFUL" }),
            tags_path: &["RuleGroupResponse", "Tags"],
            arn: stateful_arn,
        },
    ]
}

/// Every tag on `arn`, drained page by page through `ListTagsForResource`, in the
/// order the listing reports them.
async fn listed_tags(harness: &Harness, arn: &str, page_size: i64) -> Vec<(String, String)> {
    assert!(
        page_size >= 1,
        "case drain {arn}: a drain needs a page that holds at least one tag, got {page_size}"
    );
    let mut collected: Vec<(String, String)> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut request = json!({ "ResourceArn": arn, "MaxResults": page_size });
        if let Some(token) = &cursor {
            request["NextToken"] = json!(token);
        }
        let body = harness.ok("ListTagsForResource", &request).await;
        let page = body["Tags"]
            .as_array()
            .unwrap_or_else(|| panic!("case drain {arn}: expected Tags in {body:?}"))
            .clone();
        assert!(
            i64::try_from(page.len()).unwrap_or(i64::MAX) <= page_size,
            "case drain {arn}: a page may not exceed MaxResults={page_size}, got {page:?}"
        );
        collected.extend(page.iter().map(tag_pair));
        match body.get("NextToken").and_then(Value::as_str) {
            Some(token) => cursor = Some(token.to_string()),
            None => break,
        }
    }
    collected
}

/// One `Tag` on the wire as a `(Key, Value)` pair.
fn tag_pair(tag: &Value) -> (String, String) {
    (
        tag["Key"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a Key in {tag:?}"))
            .to_string(),
        tag["Value"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a Value in {tag:?}"))
            .to_string(),
    )
}

/// `Tags` as a key -> value map, so a projection is compared by content whatever
/// order it carries.
pub fn tag_map(tags: &Value) -> BTreeMap<String, String> {
    tags.as_array()
        .unwrap_or_else(|| panic!("expected a Tags list, got {tags:?}"))
        .iter()
        .map(tag_pair)
        .collect()
}

/// The `TagResource` body for `tags`.
pub fn tag_request(arn: &str, tags: &[(String, String)]) -> Value {
    json!({
        "ResourceArn": arn,
        "Tags": tags
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect::<Vec<Value>>(),
    })
}

/// `count` distinct tags, `prefix`-keyed and zero-padded so their key order is
/// their creation order.
fn generated_tags(prefix: &str, count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("{prefix}-{index:03}"), format!("value-{index}")))
        .collect()
}

/// RULE: a tag write stores the request's key:value pairs on the resource its ARN
/// names — a key the resource already carries takes the new value and a key it
/// does not carry is added — `UntagResource` removes exactly the keys it names,
/// and `ListTagsForResource` reports the tags the writes left behind, for every
/// resource kind the service tags.
///
/// The rows are the four taggable resource kinds, each driven through the same
/// sequence of writes, whose own rows differ along what the rule generalises over:
/// how many tags one call carries, the order the keys arrive in, the value form
/// (empty, long, non-ASCII), overwriting a key that is already there, removing a
/// key that is not, and removing everything. The expected tag set is recomputed in
/// the test from the rule, so each step is checked against the rule rather than
/// against a recorded list — through the listing AND through the resource's own
/// describe, which must agree.
#[tokio::test]
async fn tag_writes_and_the_tag_listing_round_trip_on_every_taggable_kind() {
    let harness = Harness::new();
    // The modelled bounds of `Tag$Key` (length 1..128) and `Tag$Value`
    // (length 0..256), from
    // `smithy_query(query="shape",shape="Tag")`.
    let longest_key = format!("long-key-{}", "k".repeat(119));
    let longest_value = "v".repeat(256);

    for kind in taggable_kinds(&harness, "round-trip").await {
        let arn = kind.arn.clone();
        let label = kind.label;
        assert_eq!(
            listed_tags(&harness, &arn, 5).await,
            Vec::new(),
            "case {label}: a resource created with no Tags carries none"
        );

        let writes: Vec<(&str, TagWrite)> = vec![
            (
                "one-tag-with-an-empty-value",
                TagWrite::Add(vec![("solo".to_string(), String::new())]),
            ),
            (
                "three-tags-with-keys-out-of-order",
                TagWrite::Add(vec![
                    ("zone".to_string(), "edge".to_string()),
                    ("env".to_string(), "prod".to_string()),
                    ("owner".to_string(), "netops".to_string()),
                ]),
            ),
            (
                "the-longest-modelled-key-and-value",
                TagWrite::Add(vec![(longest_key.clone(), longest_value.clone())]),
            ),
            (
                "a-non-ascii-key-and-value",
                TagWrite::Add(vec![("équipe".to_string(), "réseau".to_string())]),
            ),
            (
                "overwrite-one-key-and-add-another",
                TagWrite::Add(vec![
                    ("env".to_string(), "staging".to_string()),
                    ("tier".to_string(), "two".to_string()),
                ]),
            ),
            (
                "a-key-repeated-within-one-request",
                TagWrite::Add(vec![
                    ("repeated".to_string(), "first".to_string()),
                    ("repeated".to_string(), "last".to_string()),
                ]),
            ),
            (
                "remove-two-keys",
                TagWrite::Remove(vec!["solo".to_string(), "zone".to_string()]),
            ),
            (
                "remove-a-key-the-resource-does-not-carry",
                TagWrite::Remove(vec!["never-applied".to_string()]),
            ),
            (
                "remove-everything-that-is-left",
                TagWrite::Remove(vec![
                    "env".to_string(),
                    "owner".to_string(),
                    "tier".to_string(),
                    "repeated".to_string(),
                    longest_key.clone(),
                    "équipe".to_string(),
                ]),
            ),
        ];

        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        for (step, write) in &writes {
            let case = format!("{label}/{step}");
            match write {
                TagWrite::Add(tags) => {
                    harness
                        .ok("TagResource", &tag_request(&arn, tags))
                        .await;
                    for (key, value) in tags {
                        expected.insert(key.clone(), value.clone());
                    }
                }
                TagWrite::Remove(keys) => {
                    harness
                        .ok(
                            "UntagResource",
                            &json!({ "ResourceArn": arn, "TagKeys": keys }),
                        )
                        .await;
                    for key in keys {
                        expected.remove(key);
                    }
                }
            }

            let listed = listed_tags(&harness, &arn, 2).await;
            let wanted: Vec<(String, String)> = expected
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            assert_eq!(
                listed, wanted,
                "case {case}: expected the tags the writes left behind, got {listed:?}"
            );

            // A listing is a read: it reports the same thing twice.
            let again = listed_tags(&harness, &arn, 2).await;
            assert_eq!(
                again, listed,
                "case {case}: a listing must not change what the next caller sees, got {again:?}"
            );

            // The resource's own reader projects the same tags, so one resource
            // never carries two tag sets.
            let described = harness.ok(kind.describe, &kind.describe_request).await;
            let mut projected = &described;
            for step in kind.tags_path {
                projected = projected.get(step).unwrap_or(&Value::Null);
            }
            let projected = if projected.is_null() {
                BTreeMap::new()
            } else {
                tag_map(projected)
            };
            assert_eq!(
                projected, expected,
                "case {case}: expected {} to project the same tags, got {projected:?}",
                kind.describe
            );
        }
    }
}

/// RULE: a tag operation names its resource by ARN, so an ARN that names no
/// stored resource of a taggable kind is answered `ResourceNotFoundException`
/// (HTTP 400) with the service's one fixed message, an ARN naming a kind the
/// service does not tag is answered `InvalidRequestException` (HTTP 400) quoting
/// the offending ARN, and neither stores anything.
///
/// The rows span both failures across every taggable kind's ARN form, and each is
/// driven through all three tag operations.
#[tokio::test]
async fn a_tag_operation_against_an_absent_or_untaggable_arn_is_refused() {
    let harness = Harness::new();
    // A resource that DOES exist, to prove a refused call leaves state alone.
    let present = harness.shared_policy("untouched").await;
    let keep = vec![("keep".to_string(), "me".to_string())];
    harness.ok("TagResource", &tag_request(&present, &keep)).await;

    let targets: [(&str, String, &str); 7] = [
        (
            "absent-firewall",
            format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/absent"),
            "ResourceNotFoundException",
        ),
        (
            "absent-firewall-policy",
            format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall-policy/absent"),
            "ResourceNotFoundException",
        ),
        (
            "absent-stateless-rule-group",
            format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:stateless-rulegroup/absent"),
            "ResourceNotFoundException",
        ),
        (
            "absent-stateful-rule-group",
            format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:stateful-rulegroup/absent"),
            "ResourceNotFoundException",
        ),
        (
            "another-service-entirely",
            "arn:aws:s3:::a-bucket".to_string(),
            "InvalidRequestException",
        ),
        (
            "a-kind-this-service-does-not-tag",
            format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:vpc-endpoint-association/x"),
            "InvalidRequestException",
        ),
        (
            "no-resource-path-at-all",
            format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall"),
            "InvalidRequestException",
        ),
    ];

    for (label, arn, expected_code) in targets {
        let requests: [(&str, Value); 3] = [
            (
                "TagResource",
                tag_request(&arn, &[("env".to_string(), "prod".to_string())]),
            ),
            (
                "UntagResource",
                json!({ "ResourceArn": arn, "TagKeys": ["env"] }),
            ),
            ("ListTagsForResource", json!({ "ResourceArn": arn })),
        ];
        for (operation, request) in requests {
            let case = format!("{label}/{operation}");
            let (status, body) = harness.call(operation, &request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, expected_code),
                "case {case}: expected {expected_code}, got {status} {body:?}"
            );
            let message = error_message(&body);
            if expected_code == "ResourceNotFoundException" {
                assert_eq!(
                    message, NOT_FOUND_MESSAGE,
                    "case {case}: expected the service's fixed message, got {message:?}"
                );
            } else {
                assert!(
                    message.contains(&arn)
                        && message
                            .contains("does not name a firewall, firewall policy or rule group"),
                    "case {case}: expected a message naming the offending ARN, got {message:?}"
                );
            }
        }
    }

    let listed = listed_tags(&harness, &present, 10).await;
    assert_eq!(
        listed, keep,
        "case untouched: a refused tag call must leave another resource's tags alone, got \
         {listed:?}"
    );
}

/// RULE: a resource carries at most the documented number of tags
/// (`state::MAX_TAGS_PER_RESOURCE`, "up to 50 tags for a resource"); a write that
/// would take it past that ceiling is refused with `InvalidRequestException`
/// (HTTP 400) and stores nothing, while a write that only overwrites keys the
/// resource already carries is accepted at the ceiling.
#[tokio::test]
async fn the_documented_tag_ceiling_is_enforced_and_a_refused_write_stores_nothing() {
    let harness = Harness::new();
    let ceiling = state::MAX_TAGS_PER_RESOURCE;
    let arn = harness.shared_policy("tag-ceiling").await;

    // Right up to the ceiling, in one call and in several.
    let first_half = generated_tags("cap", ceiling / 2);
    let second_half: Vec<(String, String)> = generated_tags("cap", ceiling)
        .into_iter()
        .skip(ceiling / 2)
        .collect();
    for (label, batch) in [("first-half", &first_half), ("second-half", &second_half)] {
        harness.ok("TagResource", &tag_request(&arn, batch)).await;
        let listed = listed_tags(&harness, &arn, 100).await;
        assert!(
            listed.len() <= ceiling,
            "case {label}: a resource may not exceed {ceiling} tags, got {}",
            listed.len()
        );
    }
    let at_ceiling = listed_tags(&harness, &arn, 100).await;
    assert_eq!(
        at_ceiling.len(),
        ceiling,
        "case at-the-ceiling: expected {ceiling} tags, got {at_ceiling:?}"
    );

    // One more key is refused, and nothing about the resource changes.
    let overflowing = vec![("one-too-many".to_string(), "x".to_string())];
    let (status, body) = harness
        .call("TagResource", &tag_request(&arn, &overflowing))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidRequestException"),
        "case one-too-many: expected InvalidRequestException, got {status} {body:?}"
    );
    let message = error_message(&body);
    assert!(
        message.contains(&ceiling.to_string()) && message.contains(&(ceiling + 1).to_string()),
        "case one-too-many: expected a message naming the ceiling and the count, got {message:?}"
    );
    assert_eq!(
        listed_tags(&harness, &arn, 100).await,
        at_ceiling,
        "case one-too-many: a refused write must store nothing"
    );

    // Overwriting keys the resource already carries does not grow it, so it is
    // accepted at the ceiling.
    let overwrites: Vec<(String, String)> = at_ceiling
        .iter()
        .take(3)
        .map(|(key, _)| (key.clone(), "rewritten".to_string()))
        .collect();
    harness
        .ok("TagResource", &tag_request(&arn, &overwrites))
        .await;
    let rewritten = listed_tags(&harness, &arn, 100).await;
    assert_eq!(
        rewritten.len(),
        ceiling,
        "case overwrite-at-the-ceiling: expected {ceiling} tags, got {rewritten:?}"
    );
    for (key, value) in &overwrites {
        assert!(
            rewritten.contains(&(key.clone(), value.clone())),
            "case overwrite-at-the-ceiling: expected {key} to carry the new value, got \
             {rewritten:?}"
        );
    }

    // Freeing one slot makes room for exactly one new key.
    let evicted = rewritten[0].0.clone();
    harness
        .ok(
            "UntagResource",
            &json!({ "ResourceArn": arn, "TagKeys": [evicted] }),
        )
        .await;
    harness
        .ok("TagResource", &tag_request(&arn, &overflowing))
        .await;
    let refilled = listed_tags(&harness, &arn, 100).await;
    assert_eq!(
        refilled.len(),
        ceiling,
        "case refill: expected {ceiling} tags, got {refilled:?}"
    );
    assert!(
        refilled.contains(&overflowing[0]),
        "case refill: expected the freed slot to take the new tag, got {refilled:?}"
    );

    // A single call past the ceiling is refused on a resource that has no tags
    // at all, so the check is about the resulting count and not about the
    // resource already being full.
    let fresh = harness.shared_policy("tag-ceiling-fresh").await;
    let too_many = generated_tags("bulk", ceiling + 1);
    let (status, body) = harness
        .call("TagResource", &tag_request(&fresh, &too_many))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidRequestException"),
        "case bulk-past-the-ceiling: expected InvalidRequestException, got {status} {body:?}"
    );
    assert_eq!(
        listed_tags(&harness, &fresh, 100).await,
        Vec::new(),
        "case bulk-past-the-ceiling: a refused write must store nothing"
    );
    // The same call one tag shorter is accepted, which is where the ceiling sits.
    harness
        .ok(
            "TagResource",
            &tag_request(&fresh, &generated_tags("bulk", ceiling)),
        )
        .await;
    assert_eq!(
        listed_tags(&harness, &fresh, 100).await.len(),
        ceiling,
        "case bulk-at-the-ceiling: expected exactly {ceiling} tags"
    );
}

/// RULE: the tag listing is paged — a page carries at most `MaxResults` tags, the
/// `NextToken` it hands back resumes after the last tag on the page, the pages of
/// one listing concatenate to the resource's whole key-ordered tag set with
/// nothing repeated or lost, and a `NextToken` this listing did not mint is a bad
/// request.
///
/// The rows are page sizes across the modelled range of
/// `TagsPaginationMaxResults` (`range 0..100`, from
/// `smithy_query(query="shape",shape="TagsPaginationMaxResults")`): each published
/// bound, sizes inside it, and one past each bound.
#[tokio::test]
async fn the_tag_listing_is_paged_within_the_modelled_max_results_range() {
    let harness = Harness::new();
    let arn = harness.shared_policy("tag-paging").await;
    let tags = generated_tags("page", 7);
    harness.ok("TagResource", &tag_request(&arn, &tags)).await;
    let whole: Vec<(String, String)> = tags.clone();

    for page_size in [1i64, 2, 3, 7, 100] {
        let listed = listed_tags(&harness, &arn, page_size).await;
        assert_eq!(
            listed, whole,
            "case MaxResults={page_size}: expected the pages to concatenate to the whole tag \
             set, got {listed:?}"
        );
    }

    // The lower published bound asks for no tags: a page of none, plus the cursor
    // that carries on from where this call left off.
    let (status, body) = harness
        .call(
            "ListTagsForResource",
            &json!({ "ResourceArn": arn, "MaxResults": 0 }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case MaxResults=0: a published bound is accepted, got {status} {body:?}"
    );
    assert_eq!(
        body["Tags"].as_array().map(Vec::len),
        Some(0),
        "case MaxResults=0: expected no tags on the page, got {body:?}"
    );
    let resumed = body
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case MaxResults=0: expected a NextToken in {body:?}"))
        .to_string();
    let rest = harness
        .ok(
            "ListTagsForResource",
            &json!({ "ResourceArn": arn, "MaxResults": 100, "NextToken": resumed }),
        )
        .await;
    let rest: Vec<(String, String)> = rest["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case MaxResults=0: expected Tags when resuming"))
        .iter()
        .map(tag_pair)
        .collect();
    assert_eq!(
        rest, whole,
        "case MaxResults=0: resuming the cursor must lose nothing, got {rest:?}"
    );

    for max_results in [-1i64, 101] {
        let (status, body) = harness
            .call(
                "ListTagsForResource",
                &json!({ "ResourceArn": arn, "MaxResults": max_results }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case MaxResults={max_results}: a value past the bound is refused, got {status} \
             {body:?}"
        );
    }

    for (label, token) in [
        ("not-a-token", "zzz"),
        ("a-token-for-another-listing", "ZmlyZXdhbGxzOmE="),
    ] {
        let (status, body) = harness
            .call(
                "ListTagsForResource",
                &json!({ "ResourceArn": arn, "NextToken": token }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRequestException"),
            "case {label}: expected a foreign NextToken to be refused, got {status} {body:?}"
        );
    }
}
