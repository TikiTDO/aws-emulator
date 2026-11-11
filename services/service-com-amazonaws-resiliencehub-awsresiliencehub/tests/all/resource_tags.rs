// SPDX-License-Identifier: CC0-1.0

//! The tags a resource of this service carries, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a
//! restJson1 request — and asserts the response BODY, not just the status.
//!
//! The rule under test: a tag assigned to a resource is part of that resource's
//! state, so every read that answers the resource's tags — `ListTagsForResource`
//! and the `tags` member of a read of the resource itself — answers the tag set
//! currently assigned to it, whatever assigned it and whichever taggable resource
//! of this service it is. `TagResource` MERGES (a key the request does not mention
//! keeps its value), `UntagResource` removes the keys it names and nothing else,
//! and the set is bounded by what `TagMap` and `TagKey` are modelled with. An ARN
//! this service holds no resource under is the modelled 404 rather than a tag
//! written nowhere.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::application_lifecycle::{
    create_a_policy, delete, emulator, get, post, Reply, ACCOUNT, REGION,
};

/// The most tags one resource carries: `TagMap`'s modelled `@length` maximum
/// (`smithy_query(query="shape",shape="TagMap")`: length 1..50), written out here
/// rather than read from `crate::state` so a wrong bound in `src/` cannot make this
/// suite pass.
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `TagKey`'s modelled `@length` maximum, from the same shape
/// (`smithy_query(query="shape",shape="TagKey")`: length 1..128).
const TAG_KEY_MAX_LENGTH: usize = 128;

/// Percent-encode a path segment, so an ARN can travel in one.
fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The JSON object a list of pairs spells, which is how a `TagMap` travels.
fn tag_map(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<Map<String, Value>>(),
    )
}

/// `TagResource` for one ARN.
async fn tag(router: &MultiServiceRouter, resource_arn: &str, tags: &Value) -> Reply {
    post(
        router,
        &format!("/tags/{}", urlencode(resource_arn)),
        &json!({"tags": tags}),
    )
    .await
}

/// `UntagResource` for one ARN: `tagKeys` is a repeated query parameter.
async fn untag(router: &MultiServiceRouter, resource_arn: &str, keys: &[&str]) -> Reply {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", urlencode(key)))
        .collect();
    delete(
        router,
        &format!("/tags/{}?{}", urlencode(resource_arn), query.join("&")),
    )
    .await
}

/// `ListTagsForResource` for one ARN.
async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Reply {
    get(router, &format!("/tags/{}", urlencode(resource_arn))).await
}

/// One taggable resource of this service: how it is created and how a read of the
/// resource itself answers its tags.
struct Subject {
    label: &'static str,
    arn: String,
    /// The reading operation's path.
    describe_target: &'static str,
    /// The input member that operation addresses the resource by.
    id_member: &'static str,
    /// The output member carrying the resource.
    body_member: &'static str,
}

impl Subject {
    /// The `tags` member of a read of the resource itself.
    async fn own_tags(&self, router: &MultiServiceRouter) -> Value {
        let reply = post(
            router,
            self.describe_target,
            &json!({self.id_member: self.arn}),
        )
        .await;
        reply.ok(self.label)[self.body_member]["tags"].clone()
    }
}

/// An application, and its ARN.
async fn create_an_app(router: &MultiServiceRouter, name: &str) -> String {
    let reply = post(router, "/create-app", &json!({"name": name})).await;
    reply.ok(name)["app"]["appArn"]
        .as_str()
        .expect("a minted appArn")
        .to_string()
}

/// An application with a published release and an assessment of it, answering the
/// assessment's ARN.
async fn create_an_assessment(router: &MultiServiceRouter, name: &str) -> String {
    let app_arn = create_an_app(router, name).await;
    post(
        router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "resourceName": "assessed-bucket",
            "logicalResourceId": {"identifier": "AssessedBucket"},
            "physicalResourceId": "assessed-bucket",
            "resourceType": "AWS::S3::Bucket",
            "appComponents": ["storage-tier"],
        }),
    )
    .await
    .ok(name);
    post(router, "/publish-app-version", &json!({"appArn": app_arn}))
        .await
        .ok(name);
    let started = post(
        router,
        "/start-app-assessment",
        &json!({
            "appArn": app_arn,
            "appVersion": "release",
            "assessmentName": name,
        }),
    )
    .await;
    started.ok(name)["assessment"]["assessmentArn"]
        .as_str()
        .expect("a minted assessmentArn")
        .to_string()
}

/// The three taggable resources this service stores, each freshly created.
async fn subjects(router: &MultiServiceRouter) -> Vec<Subject> {
    vec![
        Subject {
            label: "an application",
            arn: create_an_app(router, "tagged-app").await,
            describe_target: "/describe-app",
            id_member: "appArn",
            body_member: "app",
        },
        Subject {
            label: "a resiliency policy",
            arn: create_a_policy(router, "tagged-policy").await,
            describe_target: "/describe-resiliency-policy",
            id_member: "policyArn",
            body_member: "policy",
        },
        Subject {
            label: "an application assessment",
            arn: create_an_assessment(router, "tagged-assessment").await,
            describe_target: "/describe-app-assessment",
            id_member: "assessmentArn",
            body_member: "assessment",
        },
    ]
}

/// One step of the table: a `TagResource` or an `UntagResource` applied to a
/// resource that already carries whatever the steps before it left.
enum Step {
    /// Assign these key/value pairs.
    Assign(&'static [(&'static str, &'static str)]),
    /// Remove these keys.
    Remove(&'static [&'static str]),
}

/// One row: what the step is called, and the step.
struct Row {
    label: &'static str,
    step: Step,
}

/// The table. The rows differ along what the rule generalises over — how many tags
/// one call assigns, the form of the key and of the value, whether the key is one
/// the resource already carries, and whether the call assigns or removes.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "one tag",
            step: Step::Assign(&[("Env", "prod")]),
        },
        Row {
            label: "three tags in one call",
            step: Step::Assign(&[
                ("team", "payments"),
                ("owner", "ops"),
                ("cost-center", "1234"),
            ]),
        },
        Row {
            // `TagValue` is modelled `@length(min: 0, max: 256)`, so the empty
            // string is a value and not a missing tag.
            label: "a tag whose value is empty",
            step: Step::Assign(&[("optional", "")]),
        },
        Row {
            label: "a key the resource already carries, with a new value",
            step: Step::Assign(&[("Env", "staging")]),
        },
        Row {
            // Every character `TagKey`'s pattern `^[^\x00-\x1f\x22]+$` admits
            // beyond the alphanumerics, in one key.
            label: "a key holding the punctuation the key pattern admits",
            step: Step::Assign(&[("project/name.v1+x@y=z -_", "punctuated")]),
        },
        Row {
            label: "two keys removed, one of which the resource never carried",
            step: Step::Remove(&["Env", "never-assigned"]),
        },
        Row {
            label: "a key assigned again after it was removed",
            step: Step::Assign(&[("Env", "prod")]),
        },
    ]
}

/// Every read of a resource answers the tag set currently assigned to it: the one
/// `TagResource` merged in and `UntagResource` took keys out of, for an
/// application, a resiliency policy and an assessment alike.
#[tokio::test]
async fn every_read_of_a_resource_answers_the_tag_set_assigned_to_it() {
    let router = emulator();

    for subject in subjects(&router).await {
        // A resource nobody has tagged carries an EMPTY tag set, never an absent
        // member, so a client can read the map unconditionally.
        let empty = tag_map(&[]);
        assert_eq!(
            list_tags(&router, &subject.arn).await.ok(subject.label),
            &json!({"tags": empty}),
            "case {:?}: an untagged resource lists an empty tag set",
            subject.label
        );
        assert_eq!(
            subject.own_tags(&router).await,
            empty,
            "case {:?}: an untagged resource answers an empty tags member",
            subject.label
        );

        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        for row in rows() {
            let case = format!("{} / {}", subject.label, row.label);
            match &row.step {
                Step::Assign(pairs) => {
                    let assigned: Vec<(String, String)> = pairs
                        .iter()
                        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                        .collect();
                    tag(&router, &subject.arn, &tag_map(&assigned))
                        .await
                        .ok(&case);
                    for (key, value) in assigned {
                        expected.insert(key, value);
                    }
                }
                Step::Remove(keys) => {
                    untag(&router, &subject.arn, keys).await.ok(&case);
                    for key in *keys {
                        expected.remove(*key);
                    }
                }
            }

            let wanted = tag_map(
                &expected
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<(String, String)>>(),
            );
            let listed = list_tags(&router, &subject.arn).await;
            assert_eq!(
                listed.ok(&case)["tags"],
                wanted,
                "case {case:?}: ListTagsForResource must answer {wanted:?}, got {:?}",
                listed.body
            );
            let own = subject.own_tags(&router).await;
            assert_eq!(
                own, wanted,
                "case {case:?}: a read of the resource must answer {wanted:?} as its \
                 tags, got {own:?}"
            );
        }
    }
}

/// The read-after-write a client performs on an application it has just tagged:
/// `UpdateApp` answers the whole `App`, so the tag set it carries is the one
/// assigned to the application, and an update that touches another member leaves
/// it alone.
#[tokio::test]
async fn update_app_answers_the_tag_set_the_application_carries() {
    let router = emulator();
    let app_arn = create_an_app(&router, "updated-and-tagged").await;

    // The key the tag is assigned under is no part of the rule: a key naming
    // another member of the API is as ordinary a key as any other.
    let assigned = [
        ("resourceArn".to_string(), app_arn.clone()),
        ("Env".to_string(), "prod".to_string()),
    ];
    tag(&router, &app_arn, &tag_map(&assigned))
        .await
        .ok("tag the application");

    for (case, description) in [
        ("the first update", "a described application"),
        ("a second update", "a re-described application"),
    ] {
        let updated = post(
            &router,
            "/update-app",
            &json!({"appArn": app_arn, "description": description}),
        )
        .await;
        let app = updated.ok(case)["app"].clone();
        assert_eq!(
            app["tags"],
            tag_map(&assigned),
            "case {case:?}: UpdateApp must answer the application's tag set, got {app:?}"
        );
        assert_eq!(
            app["description"],
            json!(description),
            "case {case:?}: the update must persist the description it carried, got {app:?}"
        );
    }

    // Untagging through the tag API is visible to the same read.
    untag(&router, &app_arn, &["Env"])
        .await
        .ok("untag the application");
    let updated = post(
        &router,
        "/update-app",
        &json!({"appArn": app_arn, "description": "after untagging"}),
    )
    .await;
    assert_eq!(
        updated.ok("after untagging")["app"]["tags"],
        tag_map(&[("resourceArn".to_string(), app_arn.clone())]),
        "case \"after untagging\": UpdateApp must answer the remaining tag, got {:?}",
        updated.body
    );
}

/// The tag set of one resource is its own: tagging one leaves every other
/// resource's tags alone.
#[tokio::test]
async fn tagging_one_resource_leaves_the_others_untouched() {
    let router = emulator();
    let subjects = subjects(&router).await;

    for (index, subject) in subjects.iter().enumerate() {
        let case = format!("only {} is tagged", subject.label);
        let only = [("owner".to_string(), format!("owner-{index}"))];
        tag(&router, &subject.arn, &tag_map(&only)).await.ok(&case);

        for (other_index, other) in subjects.iter().enumerate() {
            let expected = if other_index <= index {
                tag_map(&[("owner".to_string(), format!("owner-{other_index}"))])
            } else {
                tag_map(&[])
            };
            let listed = list_tags(&router, &other.arn).await;
            assert_eq!(
                listed.ok(&case)["tags"],
                expected,
                "case {case:?}: {} must answer {expected:?}, got {:?}",
                other.label,
                listed.body
            );
        }
    }
}

/// `ListTagsForResource` is a REPORT: reading tags changes nothing a later caller
/// sees.
#[tokio::test]
async fn listing_tags_changes_nothing() {
    let router = emulator();
    let app_arn = create_an_app(&router, "listed-twice").await;
    tag(
        &router,
        &app_arn,
        &tag_map(&[("Env".to_string(), "prod".to_string())]),
    )
    .await
    .ok("tag the application");

    let first = list_tags(&router, &app_arn).await.ok("the first read").clone();
    let second = list_tags(&router, &app_arn).await.ok("a repeated read").clone();
    assert_eq!(
        first, second,
        "case \"a repeated read\": the same request must answer identically, got \
         {first:?} and {second:?}"
    );
    let described = post(&router, "/describe-app", &json!({"appArn": app_arn})).await;
    assert_eq!(
        described.ok("the application after two reads")["app"]["tags"],
        first["tags"],
        "case \"the application after two reads\": the reads must not have changed \
         the application's tags, got {:?}",
        described.body
    );
}

/// One row of the refusal table: a tag request outside a modelled constraint.
struct Refusal {
    label: &'static str,
    tags: Value,
}

/// A tag key or a tag set outside what `TagKey` and `TagMap` are modelled with is
/// refused with the modelled 400, and the resource keeps the tags it had.
#[tokio::test]
async fn a_tag_set_outside_its_modelled_bounds_is_refused() {
    let router = emulator();
    let app_arn = create_an_app(&router, "bounded-tags").await;

    let kept = [("Env".to_string(), "prod".to_string())];
    tag(&router, &app_arn, &tag_map(&kept)).await.ok("one tag");

    // A key AT `TagKey`'s maximum is accepted; one past it is refused.
    let longest_key = "k".repeat(TAG_KEY_MAX_LENGTH);
    let mut accepted = kept.to_vec();
    accepted.push((longest_key.clone(), "at the bound".to_string()));
    tag(
        &router,
        &app_arn,
        &tag_map(&[(longest_key.clone(), "at the bound".to_string())]),
    )
    .await
    .ok("a key at the key-length bound");

    let too_long_key = "k".repeat(TAG_KEY_MAX_LENGTH + 1);
    let refusals = vec![
        Refusal {
            label: "an empty tag set",
            tags: json!({}),
        },
        Refusal {
            label: "a key one character past the key-length bound",
            tags: json!({too_long_key.clone(): "past the bound"}),
        },
        Refusal {
            label: "an empty key",
            tags: json!({"": "no key at all"}),
        },
        Refusal {
            label: "a key holding a control character",
            tags: json!({"new\nline": "outside the key pattern"}),
        },
        Refusal {
            label: "a key holding the one printable character the key pattern excludes",
            tags: json!({"quo\"te": "outside the key pattern"}),
        },
        Refusal {
            // `TagMap` is modelled `@length(min: 1, max: 50)`, so a request
            // carrying one more pair than that is outside the map's own bound.
            label: "one more pair than the tag map holds",
            tags: tag_map(
                &(0..=MAX_TAGS_PER_RESOURCE)
                    .map(|index| (format!("key-{index}"), index.to_string()))
                    .collect::<Vec<(String, String)>>(),
            ),
        },
    ];

    for refusal in &refusals {
        let reply = tag(&router, &app_arn, &refusal.tags).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {:?}: expected HTTP 400 ValidationException, got {} {:?} {:?}",
            refusal.label,
            reply.status,
            reply.error_code,
            reply.body
        );
        let listed = list_tags(&router, &app_arn).await;
        assert_eq!(
            listed.ok(refusal.label)["tags"],
            tag_map(&accepted),
            "case {:?}: a refused request must leave the tag set alone, got {:?}",
            refusal.label,
            listed.body
        );
    }
}

/// The stored tag set is bounded by `TagMap`'s modelled maximum: the call that
/// would take a resource past it is refused, having written none of its tags,
/// while a call that only replaces values the resource already has is not.
#[tokio::test]
async fn a_resource_carries_no_more_tags_than_the_modelled_maximum() {
    let router = emulator();
    let app_arn = create_an_app(&router, "tag-ceiling").await;

    let full: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    tag(&router, &app_arn, &tag_map(&full))
        .await
        .ok("the whole tag set at once");

    let over = tag(
        &router,
        &app_arn,
        &tag_map(&[("one-too-many".to_string(), "refused".to_string())]),
    )
    .await;
    assert_eq!(
        (over.status, over.error_code.as_str()),
        (400, "ValidationException"),
        "case \"one tag past the ceiling\": expected HTTP 400 ValidationException, \
         got {} {:?} {:?}",
        over.status,
        over.error_code,
        over.body
    );
    let listed = list_tags(&router, &app_arn).await;
    assert_eq!(
        listed.ok("one tag past the ceiling")["tags"],
        tag_map(&full),
        "case \"one tag past the ceiling\": the refused call must have written \
         nothing, got {:?}",
        listed.body
    );

    // Replacing a value the resource already carries does not grow the set, so it
    // is accepted at the ceiling.
    let mut replaced = full.clone();
    replaced[0].1 = "replaced".to_string();
    tag(
        &router,
        &app_arn,
        &tag_map(&[(replaced[0].0.clone(), "replaced".to_string())]),
    )
    .await
    .ok("a replaced value at the ceiling");
    let listed = list_tags(&router, &app_arn).await;
    assert_eq!(
        listed.ok("a replaced value at the ceiling")["tags"],
        tag_map(&replaced),
        "case \"a replaced value at the ceiling\": expected the replaced value, got \
         {:?}",
        listed.body
    );

    // And a key freed by an untag can be used again.
    untag(&router, &app_arn, &[&replaced[0].0])
        .await
        .ok("free one key");
    tag(
        &router,
        &app_arn,
        &tag_map(&[("one-too-many".to_string(), "now it fits".to_string())]),
    )
    .await
    .ok("a key that now fits");
}

/// An ARN this service holds no resource under is the modelled 404 — the same
/// sentence from each of the three tagging operations, naming the member that did
/// not resolve and the value it carried.
#[tokio::test]
async fn an_unknown_resource_arn_is_refused_by_every_tagging_operation() {
    let router = emulator();
    let app_arn = create_an_app(&router, "tagging-refusals").await;

    let unknown = [
        (
            "an application that was never created",
            format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:app/00000000-0000-0000-0000-000000000000"),
        ),
        (
            "a resiliency policy that was never created",
            format!(
                "arn:aws:resiliencehub:{REGION}:{ACCOUNT}:resiliency-policy/\
                 00000000-0000-0000-0000-000000000000"
            ),
        ),
        (
            "a deleted application",
            {
                let deleted = create_an_app(&router, "tagging-deleted").await;
                post(&router, "/delete-app", &json!({"appArn": deleted}))
                    .await
                    .ok("delete the application");
                deleted
            },
        ),
    ];

    for (case, resource_arn) in &unknown {
        let expected = format!("Resource not found for resourceArn {resource_arn}");
        let replies = [
            (
                "TagResource",
                tag(
                    &router,
                    resource_arn,
                    &tag_map(&[("Env".to_string(), "prod".to_string())]),
                )
                .await,
            ),
            ("UntagResource", untag(&router, resource_arn, &["Env"]).await),
            ("ListTagsForResource", list_tags(&router, resource_arn).await),
        ];
        for (operation, reply) in &replies {
            assert_eq!(
                (
                    reply.status,
                    reply.error_code.as_str(),
                    reply.body["message"].as_str().unwrap_or_default()
                ),
                (404, "ResourceNotFoundException", expected.as_str()),
                "case {case:?} / {operation}: expected HTTP 404 \
                 ResourceNotFoundException {expected:?}, got {} {:?} {:?}",
                reply.status,
                reply.error_code,
                reply.body
            );
        }
    }

    // Not vacuous: the application that DOES exist is tagged by the same calls.
    tag(
        &router,
        &app_arn,
        &tag_map(&[("Env".to_string(), "prod".to_string())]),
    )
    .await
    .ok("the application that exists");
}
