// SPDX-License-Identifier: CC0-1.0

//! The refusal of a `ResourceArn` the resource-tagging layer cannot parse as an ARN.
//!
//! The rule under test: the layer that serves `/tags/{ResourceArn}` parses that value as an
//! ARN before it resolves anything, and a value that does not parse — six colon-separated
//! fields opening with the literal `arn` and carrying a partition, a service and a resource —
//! is refused by that layer itself with HTTP 400, the `BadRequestException` error code and one
//! fixed sentence under the lower-case `message` member, with nothing of the request
//! interpolated into it. A value that DOES parse is passed on, so it is answered about the
//! resource — the layer's not-found refusal for an ARN that resolves to nothing here, or this
//! service's own `ValidationException` for an ARN of this service that is not one of the
//! resource ARNs it publishes.
//!
//! The ARN form is the general one, not this service's: the region and the account field are
//! allowed to be empty, because the ARNs of global and account-less resources leave them so,
//! and the resource field keeps any further colons, because a resource id may carry them.
//!
//! All three tagging operations share that one route, so every row is sent to all three — the
//! two that read or write tags and the one that removes them — and each row also asserts that a
//! refused call wrote nothing: the resource that does exist still reports exactly the tags it
//! was created with.
//!
//! The rows differ along what the rule generalises over: the field count (none, too few, too
//! many colons inside the resource), which required field is empty, the case and spelling of
//! the `arn` prefix, the separator, leading and trailing whitespace, a value that is only
//! separators, a very long value, a non-ASCII value — and, on the other side of the rule, the
//! shortest value that parses, ARNs with an empty region and account, an ARN whose resource
//! field carries colons, and this service's own resource ARNs.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};

/// The Region every call in this suite is signed for.
const REGION: &str = "eu-west-3";

/// The canonical emulator account id.
const ACCOUNT: &str = "000000000000";

/// A private-CA ARN matching `CertificateAuthorityArn`'s own pattern
/// (`^arn:aws(-[a-z]+)*:acm-pca:<region>:<account>:certificate-authority/<uuid>$`).
const CA_ARN: &str =
    "arn:aws:acm-pca:eu-west-3:000000000000:certificate-authority/22222222-3333-4444-5555-666666666666";

/// A connector id in the UUID form this service's ARN patterns require. Nothing is created
/// under it, so an ARN carrying it is answered only by how the value reads.
const CONNECTOR_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(router, REGION, "GET", &format!("/tags/{}", label(arn)), None).await
}

async fn tag_resource(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "POST",
        &format!("/tags/{}", label(arn)),
        Some(json!({ "Tags": { "owner": "platform" } })),
    )
    .await
}

async fn untag_resource(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "DELETE",
        &format!("/tags/{}?tagKeys=owner", label(arn)),
        None,
    )
    .await
}

/// The three answers to one `ResourceArn`, one per tagging operation.
async fn answers(router: &MultiServiceRouter, arn: &str) -> Vec<(&'static str, Answer)> {
    vec![
        ("ListTagsForResource", list_tags(router, arn).await),
        ("TagResource", tag_resource(router, arn).await),
        ("UntagResource", untag_resource(router, arn).await),
    ]
}

/// Values that are not ARNs: each row names how it fails the parse.
fn unparseable() -> Vec<(&'static str, String)> {
    vec![
        ("no fields at all", "banana".to_string()),
        ("the empty resource field alone", ":".to_string()),
        ("only separators", ":::::".to_string()),
        ("one field short", format!("arn:aws:pca-connector-scep:{REGION}:{ACCOUNT}")),
        ("three fields", "arn:aws:pca-connector-scep".to_string()),
        ("two fields", "arn:aws".to_string()),
        ("the prefix and a separator", "arn:".to_string()),
        ("an empty partition", format!("arn::pca-connector-scep:{REGION}:{ACCOUNT}:connector/{CONNECTOR_ID}")),
        ("an empty service", format!("arn:aws::{REGION}:{ACCOUNT}:connector/{CONNECTOR_ID}")),
        ("an empty resource", format!("arn:aws:pca-connector-scep:{REGION}:{ACCOUNT}:")),
        ("an upper-case prefix", format!("ARN:aws:pca-connector-scep:{REGION}:{ACCOUNT}:connector/{CONNECTOR_ID}")),
        ("a misspelled prefix", format!("srn:aws:pca-connector-scep:{REGION}:{ACCOUNT}:connector/{CONNECTOR_ID}")),
        ("no prefix at all", format!("aws:pca-connector-scep:{REGION}:{ACCOUNT}:connector/{CONNECTOR_ID}:extra")),
        ("leading whitespace before the prefix", format!(" arn:aws:pca-connector-scep:{REGION}:{ACCOUNT}:connector/{CONNECTOR_ID}")),
        ("another separator entirely", format!("arn|aws|pca-connector-scep|{REGION}|{ACCOUNT}|connector/{CONNECTOR_ID}")),
        ("a URL", "https://example.test/connectors/1".to_string()),
        ("a bare resource id", CONNECTOR_ID.to_string()),
        ("a very long value with no separators", format!("not-an-arn-{}", "x".repeat(400))),
        ("a non-ASCII value", "идентификатор".to_string()),
    ]
}

/// Values that DO parse as ARNs, so the parse is not what answers them. None of them names a
/// resource this emulator holds, so each is the tagging layer's not-found refusal or this
/// service's own check of its own ARN space — never the bad-syntax one.
fn parseable() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest value that parses", "arn:a:b:::c".to_string()),
        (
            "an empty region and account, as a global resource's ARN has",
            "arn:aws:s3:::not-a-scep-resource".to_string(),
        ),
        (
            "a resource field carrying further colons",
            format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:/aws/anything:*"),
        ),
        (
            "a connector ARN of this service that nothing was created under",
            format!("arn:aws:pca-connector-scep:{REGION}:{ACCOUNT}:connector/{CONNECTOR_ID}"),
        ),
        (
            "an ARN of this service whose resource field is not one it publishes",
            format!("arn:aws:pca-connector-scep:{REGION}:{ACCOUNT}:widget/{CONNECTOR_ID}"),
        ),
        (
            "an ARN of another service in this account",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/not-a-scep-resource"),
        ),
    ]
}

/// A connector that DOES exist, with the tags it was created with, so every row below is
/// answered beside live state rather than out of an empty store — and a refused call can be
/// shown to have left that state alone.
async fn live_connector(router: &MultiServiceRouter, tags: Value) -> String {
    let created = harness::call(
        router,
        REGION,
        "POST",
        "/connectors",
        Some(json!({ "CertificateAuthorityArn": CA_ARN, "Tags": tags })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    created.text("ConnectorArn")
}

#[tokio::test]
async fn a_resource_arn_the_tagging_layer_cannot_parse_is_its_own_bad_request() {
    let (router, engine) = new_router();
    let connector = live_connector(&router, json!({ "keep": "me" })).await;
    harness::release_pending_if_owed(&engine, &connector);

    for (case, arn) in unparseable() {
        for (operation, answer) in answers(&router, &arn).await {
            harness::assert_tagging_bad_request(&answer, &format!("{case} on {operation}"));
        }
    }

    // A refused call is refused before anything is looked up, so the resource that does exist
    // still carries exactly what it was created with.
    let listed = list_tags(&router, &connector).await;
    assert_eq!(
        listed.body["Tags"],
        json!({ "keep": "me" }),
        "a refused call must leave the stored tags alone, got {} / {:?}",
        listed.status,
        listed.body
    );
}

#[tokio::test]
async fn a_resource_arn_that_parses_is_answered_about_the_resource() {
    let (router, engine) = new_router();
    let connector = live_connector(&router, json!({ "keep": "me" })).await;
    harness::release_pending_if_owed(&engine, &connector);

    for (case, arn) in parseable() {
        for (operation, answer) in answers(&router, &arn).await {
            assert_eq!(
                answer.status, 400,
                "case {case:?} on {operation}: expected 400 for an ARN naming no resource \
                 here, got {} / {:?}",
                answer.status, answer.body
            );
            assert_ne!(
                answer.error_type.as_str(),
                "BadRequestException",
                "case {case:?} on {operation}: {arn:?} parses as an ARN, so the parse must not \
                 be what refuses it, got {:?}",
                answer.body
            );
            assert_ne!(
                answer.body["message"].as_str(),
                Some("Invalid input resource arn: bad syntax"),
                "case {case:?} on {operation}: {arn:?} parses as an ARN, so the bad-syntax \
                 sentence must not be stated for it, got {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn a_resource_arn_the_service_holds_a_record_under_is_still_served() {
    let (router, engine) = new_router();
    let connector = live_connector(&router, json!({ "which": "connector" })).await;
    harness::release_pending_if_owed(&engine, &connector);

    // The ARN the create answered with parses, so none of the three operations refuses it.
    let tagged = tag_resource(&router, &connector).await;
    assert_eq!(
        tagged.status, 204,
        "expected 204 from TagResource on {connector:?}, got {} / {:?}",
        tagged.status, tagged.body
    );
    let listed = list_tags(&router, &connector).await;
    assert_eq!(
        listed.body["Tags"],
        json!({ "which": "connector", "owner": "platform" }),
        "expected the written tag beside the created one, got {} / {:?}",
        listed.status,
        listed.body
    );
    let untagged = untag_resource(&router, &connector).await;
    assert_eq!(
        untagged.status, 204,
        "expected 204 from UntagResource on {connector:?}, got {} / {:?}",
        untagged.status, untagged.body
    );
    let listed = list_tags(&router, &connector).await;
    assert_eq!(
        listed.body["Tags"],
        json!({ "which": "connector" }),
        "expected the removal to be the one a read sees, got {} / {:?}",
        listed.status,
        listed.body
    );
}
