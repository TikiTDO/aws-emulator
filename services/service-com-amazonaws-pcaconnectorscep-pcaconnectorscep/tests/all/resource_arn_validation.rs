// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for how the tagging operations read their `ResourceArn`.
//!
//! The rule under test: `ResourceArn` is parsed BEFORE anything is looked up. A value that
//! could not name a resource — not an ARN at all — and an ARN of THIS service that is not one
//! of the resource-ARN forms it publishes are input-validation failures, refused with HTTP 400
//! `ValidationException` in the `N validation error(s) detected: Value '<value>' at '<member>'
//! failed to satisfy constraint: <requirement>` envelope, naming the member and every
//! requirement the value broke. The tagging layer's own
//! not-found refusal is left to a well-formed resource ARN that this service simply holds no
//! record under — which includes an ARN of another service, because such a value does name a
//! resource.
//!
//! `ResourceArn` is modelled as a bare `String` with no `@pattern` and no `@length`
//! (smithy_query operation=ListTagsForResource,TagResource), so nothing is refused for it at
//! the request boundary and the handler is the only place the check can live. That also means
//! nothing recorded shows how the service words its own check of this member, which is why
//! this envelope — the tree's wording for the class — is what it keeps, where a value outside
//! a `@pattern` the MODEL declares is worded as that refusal was recorded (see
//! `model_pattern_refusal`).
//!
//! Every case drives the service the way a client does: one HTTP request per call, through the
//! same `MultiServiceRouter` the emulator boots, and each refusable value is sent to BOTH
//! implemented tagging operations. The rows differ along what the rule generalises over: how
//! the value fails to be an ARN (shape, field count, an empty field), which part of this
//! service's published form it breaks (resource type, connector id, challenge id, account,
//! region), which of the two resource kinds it looks like, both published length bounds and
//! one past each, a value past the ceiling on what a clause echoes back, and — on the other
//! side of the rule — well-formed ARNs of either kind, of another partition and of another
//! service.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};

/// A region that is NOT the emulator default, so an ARN built from the request's own SigV4
/// credential scope is distinguishable from a hardcoded one.
const REGION: &str = "eu-west-3";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A private-CA ARN matching `CertificateAuthorityArn`'s own pattern
/// (`^arn:aws(-[a-z]+)*:acm-pca:<region>:<account>:certificate-authority/<uuid>$`).
const CA_ARN: &str =
    "arn:aws:acm-pca:eu-west-3:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555";

/// A connector id in the UUID form the ARN patterns require.
const CONNECTOR_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// A challenge id in the same form.
const CHALLENGE_ID: &str = "99999999-8888-7777-6666-555544443333";

/// The published length bounds of a resource ARN: the `@length(5, 200)` that both
/// `ConnectorArn` and `ChallengeArn` declare (smithy_query shape=ConnectorArn,ChallengeArn).
const MIN_ARN_LENGTH: usize = 5;
const MAX_ARN_LENGTH: usize = 200;

/// The expression a refusal quotes as the requirement for `ResourceArn`: `ConnectorArn`'s own
/// `@pattern` with `ChallengeArn`'s challenge segment made optional — the challenge pattern is
/// the connector pattern plus that segment (smithy_query shape=ConnectorArn,ChallengeArn).
/// Written out here rather than read from the service, so a drift in either copy fails a case.
const RESOURCE_ARN_PATTERN: &str = concat!(
    r"^arn:aws(-[a-z]+)*:pca-connector-scep:[a-z]+(-[a-z]+)+-[1-9]\d*:\d{12}",
    r":connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}",
    r"(\/challenge\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12})?$"
);

async fn create_connector(router: &MultiServiceRouter, request: Value) -> Answer {
    harness::call(router, REGION, "POST", "/connectors", Some(request)).await
}

async fn create_challenge(router: &MultiServiceRouter, request: Value) -> Answer {
    harness::call(router, REGION, "POST", "/challenges", Some(request)).await
}

async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(router, REGION, "GET", &format!("/tags/{}", label(arn)), None).await
}

async fn tag_resource(router: &MultiServiceRouter, arn: &str, tags: Value) -> Answer {
    harness::call(
        router,
        REGION,
        "POST",
        &format!("/tags/{}", label(arn)),
        Some(json!({ "Tags": tags })),
    )
    .await
}

/// A connector ARN of this service, in the published form.
fn connector_arn(partition: &str, region: &str, account: &str, connector_id: &str) -> String {
    format!("arn:{partition}:pca-connector-scep:{region}:{account}:connector/{connector_id}")
}

/// A challenge ARN: a connector ARN plus the challenge segment.
fn challenge_arn(connector: &str, challenge_id: &str) -> String {
    format!("{connector}/challenge/{challenge_id}")
}

/// A region in the form the ARN patterns require (`[a-z]+(-[a-z]+)+-[1-9]\d*`), padded so the
/// whole connector ARN built from it is exactly `total` characters long.
fn region_padding_arn_to(total: usize) -> String {
    let fixed = connector_arn("aws", "", ACCOUNT, CONNECTOR_ID).len();
    let width = total - fixed;
    // `<letters>-<letters>-1`: the two letter runs take whatever is left after the separators
    // and the trailing `-1`.
    let letters = width - 3;
    let first = letters / 2;
    format!(
        "{}-{}-1",
        "u".repeat(first),
        "e".repeat(letters - first)
    )
}

/// Which constraint of `ResourceArn` a refused row broke. One clause per constraint, in the
/// order the request boundary states them for a patterned label: length, then pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clause {
    TooShort,
    TooLong,
    Pattern,
}

/// Ceiling, in bytes, on the value a clause echoes back, and the elision that marks a value
/// cut at it: the rendering the request boundary gives the same clause for an over-long value.
const MAX_ECHOED_VALUE: usize = 256;

/// The value as a clause carries it.
fn echoed(value: &str) -> String {
    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}

/// The clause a broken constraint states, built from the published bounds and pattern.
fn clause_text(clause: Clause, value: &str) -> String {
    let requirement = match clause {
        Clause::TooShort => {
            format!("Member must have length greater than or equal to {MIN_ARN_LENGTH}")
        }
        Clause::TooLong => {
            format!("Member must have length less than or equal to {MAX_ARN_LENGTH}")
        }
        Clause::Pattern => {
            format!("Member must satisfy regular expression pattern: {RESOURCE_ARN_PATTERN}")
        }
    };
    format!(
        "Value '{}' at 'resourceArn' failed to satisfy constraint: {requirement}",
        echoed(value)
    )
}

/// The whole `Message` a refusal carries: AWS's envelope over the clauses.
fn expected_message(value: &str, clauses: &[Clause]) -> String {
    let rendered: Vec<String> = clauses
        .iter()
        .map(|clause| clause_text(*clause, value))
        .collect();
    match rendered.len() {
        1 => format!("1 validation error detected: {}", rendered[0]),
        count => format!(
            "{count} validation errors detected: {}",
            rendered.join("; ")
        ),
    }
}

/// What a row expects of both tagging operations.
#[derive(Debug)]
enum Expected {
    /// 400 `ValidationException`, stating exactly these clauses.
    Refused(Vec<Clause>),
    /// The tagging layer's own not-found refusal: the value could name a resource of this
    /// service, and names none here, so the tag subsystem reports it as the resource it could
    /// not resolve ([`harness::assert_tagging_not_found`]) — one fixed sentence naming nothing
    /// of the request.
    NotFound,
}

/// One row: a `ResourceArn` and the answer the rule gives it.
#[derive(Debug)]
struct Case {
    name: &'static str,
    arn: String,
    expected: Expected,
}

fn refused(name: &'static str, arn: String, clauses: &[Clause]) -> Case {
    Case {
        name,
        arn,
        expected: Expected::Refused(clauses.to_vec()),
    }
}

fn not_found(name: &'static str, arn: String) -> Case {
    Case {
        name,
        arn,
        expected: Expected::NotFound,
    }
}

/// The rows. Nothing in the store is created under any of these ARNs, so the only thing that
/// can distinguish them is how the value itself reads.
fn cases() -> Vec<Case> {
    let good_connector = connector_arn("aws", REGION, ACCOUNT, CONNECTOR_ID);
    let at_max = connector_arn(
        "aws",
        &region_padding_arn_to(MAX_ARN_LENGTH),
        ACCOUNT,
        CONNECTOR_ID,
    );
    let past_max = connector_arn(
        "aws",
        &region_padding_arn_to(MAX_ARN_LENGTH + 1),
        ACCOUNT,
        CONNECTOR_ID,
    );
    vec![
        // Not an ARN at all.
        refused("a bare word", "banana".to_string(), &[Clause::Pattern]),
        refused(
            "colon-separated text that is not an ARN",
            "not:an:arn".to_string(),
            &[Clause::Pattern],
        ),
        refused(
            "a truncated ARN naming this service and nothing else",
            "arn:aws:pca-connector-scep".to_string(),
            &[Clause::Pattern],
        ),
        refused(
            "an ARN whose partition is empty",
            "arn::pca-connector-scep:eu-west-3:000000000000:connector/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
                .to_string(),
            &[Clause::Pattern],
        ),
        refused(
            "an ARN whose resource field is empty",
            format!("arn:aws:pca-connector-scep:{REGION}:{ACCOUNT}:"),
            &[Clause::Pattern],
        ),
        refused(
            "a URL rather than an ARN",
            "https://example.test/connectors/1".to_string(),
            &[Clause::Pattern],
        ),
        // This service's own ARN space, broken in one part each.
        refused(
            "a connector id that is not a UUID",
            connector_arn("aws", REGION, ACCOUNT, "not-a-uuid"),
            &[Clause::Pattern],
        ),
        refused(
            "a connector id missing a group",
            connector_arn("aws", REGION, ACCOUNT, "aaaaaaaa-bbbb-cccc-eeeeeeeeeeee"),
            &[Clause::Pattern],
        ),
        refused(
            "a connector id that is not hexadecimal",
            connector_arn("aws", REGION, ACCOUNT, "gggggggg-bbbb-cccc-dddd-eeeeeeeeeeee"),
            &[Clause::Pattern],
        ),
        refused(
            "a challenge id that is not a UUID",
            challenge_arn(&good_connector, "not-a-uuid"),
            &[Clause::Pattern],
        ),
        refused(
            "a challenge segment hanging off a malformed connector ARN",
            challenge_arn(&connector_arn("aws", REGION, ACCOUNT, "nope"), CHALLENGE_ID),
            &[Clause::Pattern],
        ),
        refused(
            "a resource type this service does not have",
            format!("arn:aws:pca-connector-scep:{REGION}:{ACCOUNT}:widget/{CONNECTOR_ID}"),
            &[Clause::Pattern],
        ),
        refused(
            "an account id that is not twelve digits",
            connector_arn("aws", REGION, "12345", CONNECTOR_ID),
            &[Clause::Pattern],
        ),
        refused(
            "a region that is not in the published form",
            connector_arn("aws", "EU_WEST_3", ACCOUNT, CONNECTOR_ID),
            &[Clause::Pattern],
        ),
        // The published length bounds, and one past each.
        refused(
            "one character below the published minimum length",
            "arn:".to_string(),
            &[Clause::TooShort, Clause::Pattern],
        ),
        refused(
            "at the published minimum length, and still not an ARN",
            "arn:a".to_string(),
            &[Clause::Pattern],
        ),
        refused(
            "one character past the published maximum length",
            past_max,
            &[Clause::TooLong],
        ),
        refused(
            "far longer than the value a clause echoes back",
            format!("not-an-arn-{}", "x".repeat(400)),
            &[Clause::TooLong, Clause::Pattern],
        ),
        // Well formed, and naming nothing here.
        not_found("at the published maximum length", at_max),
        not_found(
            "a connector ARN nothing was created under",
            good_connector.clone(),
        ),
        not_found(
            "a challenge ARN nothing was created under",
            challenge_arn(&good_connector, CHALLENGE_ID),
        ),
        not_found(
            "a connector ARN in another partition",
            connector_arn("aws-us-gov", "us-gov-west-1", ACCOUNT, CONNECTOR_ID),
        ),
        not_found(
            "an ARN of another service entirely",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/not-a-scep-resource"),
        ),
        not_found(
            "an ARN of another service with no region or account",
            "arn:aws:s3:::not-a-scep-resource".to_string(),
        ),
    ]
}

/// Assert one row against one operation's answer.
fn assert_row(case: &Case, operation: &str, answer: &Answer) {
    match &case.expected {
        Expected::Refused(clauses) => {
            assert_eq!(
                (answer.status, answer.error_type.as_str()),
                (400, "ValidationException"),
                "case {:?} on {operation}: expected 400 ValidationException for {:?}, got {} / \
                 {:?}",
                case.name,
                case.arn,
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.message(),
                expected_message(&case.arn, clauses),
                "case {:?} on {operation}: expected the validation envelope naming resourceArn, \
                 got {:?}",
                case.name,
                answer.message()
            );
            // A value that named no resource must not be reported as one: the refusal carries
            // no `ResourceId` / `ResourceType`, so no resource kind is invented for it.
            for member in ["ResourceId", "ResourceType"] {
                assert!(
                    answer.body[member].is_null(),
                    "case {:?} on {operation}: a refusal must not name a {member}, got {:?}",
                    case.name,
                    answer.body
                );
            }
        }
        Expected::NotFound => {
            harness::assert_tagging_not_found(
                answer,
                &format!("{} on {operation}", case.name),
            );
        }
    }
}

#[tokio::test]
async fn a_resource_arn_that_names_no_resource_is_a_validation_failure_not_a_404() {
    let (router, engine) = new_router();
    // A connector and a challenge that DO exist, so no row below is answered out of an empty
    // store.
    let created = create_connector(&router, json!({ "CertificateAuthorityArn": CA_ARN })).await;
    assert_eq!(
        created.status, 202,
        "expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    let live_connector = created.text("ConnectorArn");
    harness::release_pending_if_owed(&engine, &live_connector);
    let minted = create_challenge(&router, json!({ "ConnectorArn": live_connector })).await;
    assert_eq!(
        minted.status, 202,
        "expected 202 from CreateChallenge, got {} / {:?}",
        minted.status, minted.body
    );

    for case in cases() {
        let listed = list_tags(&router, &case.arn).await;
        assert_row(&case, "ListTagsForResource", &listed);

        let tagged = tag_resource(&router, &case.arn, json!({ "owner": "platform" })).await;
        assert_row(&case, "TagResource", &tagged);
    }

    // The rows changed nothing: the resources that do exist still report their own tags.
    let listed = list_tags(&router, &live_connector).await;
    assert_eq!(
        listed.status, 200,
        "the live connector must still be readable, got {} / {:?}",
        listed.status, listed.body
    );
}

#[tokio::test]
async fn a_resource_arn_the_service_holds_a_record_under_is_still_served() {
    let (router, engine) = new_router();
    let created = create_connector(
        &router,
        json!({ "CertificateAuthorityArn": CA_ARN, "Tags": { "which": "connector" } }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    let connector = created.text("ConnectorArn");
    harness::release_pending_if_owed(&engine, &connector);

    let minted = create_challenge(
        &router,
        json!({ "ConnectorArn": connector, "Tags": { "which": "challenge" } }),
    )
    .await;
    assert_eq!(
        minted.status, 202,
        "expected 202 from CreateChallenge, got {} / {:?}",
        minted.status, minted.body
    );
    let challenge = minted.body["Challenge"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a challenge Arn in {:?}", minted.body))
        .to_string();

    for (kind, arn, which) in [
        ("connector", connector.as_str(), "connector"),
        ("challenge", challenge.as_str(), "challenge"),
    ] {
        let listed = list_tags(&router, arn).await;
        assert_eq!(
            listed.status, 200,
            "case {kind:?}: expected 200 from ListTagsForResource, got {} / {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.body["Tags"]["which"].as_str(),
            Some(which),
            "case {kind:?}: expected the stored tags, got {:?}",
            listed.body
        );

        let tagged = tag_resource(&router, arn, json!({ "stage": "beta" })).await;
        assert_eq!(
            tagged.status, 204,
            "case {kind:?}: expected 204 from TagResource, got {} / {:?}",
            tagged.status, tagged.body
        );
        let again = list_tags(&router, arn).await;
        assert_eq!(
            again.body["Tags"]["stage"].as_str(),
            Some("beta"),
            "case {kind:?}: the tag just written must be the one read back, got {:?}",
            again.body
        );
    }
}

#[tokio::test]
async fn a_refused_resource_arn_writes_nothing() {
    let (router, engine) = new_router();
    let created = create_connector(
        &router,
        json!({ "CertificateAuthorityArn": CA_ARN, "Tags": { "keep": "me" } }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    let connector = created.text("ConnectorArn");
    harness::release_pending_if_owed(&engine, &connector);

    // The same connector id, in values the rule refuses: a different case, a missing group,
    // and the id on its own.
    let connector_id = connector
        .rsplit('/')
        .next()
        .expect("a connector ARN ends in its id")
        .to_string();
    for malformed in [
        connector.to_uppercase(),
        connector.replace(':', "|"),
        connector_id.clone(),
    ] {
        let tagged = tag_resource(&router, &malformed, json!({ "keep": "overwritten" })).await;
        assert_eq!(
            (tagged.status, tagged.error_type.as_str()),
            (400, "ValidationException"),
            "case {malformed:?}: expected 400 ValidationException, got {} / {:?}",
            tagged.status,
            tagged.body
        );
    }

    let listed = list_tags(&router, &connector).await;
    assert_eq!(
        listed.body["Tags"],
        json!({ "keep": "me" }),
        "a refused call must leave the resource's tags alone, got {:?}",
        listed.body
    );
}
