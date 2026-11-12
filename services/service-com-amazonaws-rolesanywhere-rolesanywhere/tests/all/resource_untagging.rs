// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UntagResource`, proved through the `ListTagsForResource`
//! read and the `TagResource` write that put the tags there.
//!
//! Every request goes through the real `MultiServiceRouter` — the same dispatch the
//! emulator binary uses — so routing, restJson1 (de)serialization, boundary constraint
//! validation and the handler all run exactly as they do in production.
//!
//! The rule under test: `UntagResource` detaches from the tag set of the resource its
//! `resourceArn` names exactly the keys its `tagKeys` list names — a key the resource
//! carries loses its value, a key it does not carry leaves the set unchanged, and a key
//! the request does not name is kept — so the stored set afterwards is the held set minus
//! the named keys, and `ListTagsForResource` answers with it. The tables below therefore
//! span what the rule generalises over: every taggable resource type of this service, the
//! number and overlap of the named keys, their order and repetition, the published
//! `TagKey` bounds and character class, and an ARN that resolves to nothing.
//!
//! The harness below is local to this suite: each sibling suite's client is private to
//! its own module, and the targets are compiled into one test binary only as separate
//! modules.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_rolesanywhere_rolesanywhere::state::MAX_TAGS_PER_RESOURCE;
use smithy_server_runtime::MultiServiceRouter;

/// A placeholder PEM bundle for a trust anchor's certificate source.
/// `SourceData$x509CertificateData` is `@length(1, 204800)` with no `@pattern`.
const CERTIFICATE_BUNDLE_PEM: &str =
    "-----BEGIN CERTIFICATE-----\nZW11bGF0ZWQtdGVzdC1jZXJ0\n-----END CERTIFICATE-----\n";

/// A placeholder CRL in PEM form; `ImportCrlRequest$crlData` is an opaque `Blob`.
const CRL_PEM: &str = "-----BEGIN X509 CRL-----\nZW11bGF0ZWQtdGVzdC1jcmw=\n-----END X509 CRL-----\n";

/// The Region the tests sign for.
const REGION: &str = "us-west-2";

/// The account every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The published `TagKey` `@length` bounds
/// (`smithy_query(query="shape", shape="TagKey")`: `@length(1, 128)`).
const MIN_TAG_KEY_LENGTH: usize = 1;
const MAX_TAG_KEY_LENGTH: usize = 128;

/// The published `TagKeyList` `@length` maximum, which bounds the keys ONE
/// `UntagResource` call may name
/// (`smithy_query(query="shape", shape="TagKeyList")`: `@length(0, 200)`).
const MAX_TAG_KEY_LIST_LENGTH: usize = 200;

/// A client over one emulator: one engine, one mounted service, so every request in a
/// test observes the state the previous one wrote.
struct Client {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status, the parsed JSON body (`Value::Null` for an empty body)
/// and the wire error code when the answer is a failure.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
    error_code: String,
}

impl Client {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_rolesanywhere_rolesanywhere::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            // The SigV4 credential scope is how the router resolves the service and how
            // the handler learns the request's Region.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/rolesanywhere/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = if header_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit(['#', '/'])
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    async fn tag_resource(&self, resource_arn: &str, tags: &[(String, String)]) -> Answer {
        self.send(
            "POST",
            "/TagResource",
            Some(json!({"resourceArn": resource_arn, "tags": tag_json(tags)})),
        )
        .await
    }

    async fn untag_resource(&self, resource_arn: &str, tag_keys: &[String]) -> Answer {
        self.send(
            "POST",
            "/UntagResource",
            Some(json!({"resourceArn": resource_arn, "tagKeys": tag_keys})),
        )
        .await
    }

    async fn list_tags(&self, resource_arn: &str) -> Answer {
        self.send(
            "GET",
            &format!("/ListTagsForResource?resourceArn={resource_arn}"),
            None,
        )
        .await
    }

    async fn delete_trust_anchor(&self, id: &str) -> Answer {
        self.send("DELETE", &format!("/trustanchor/{id}"), None).await
    }
}

/// The wire form of a tag list: `TagList` of `Tag { key, value }`.
fn tag_json(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    )
}

/// Owned pairs from literal rows, so a table row reads as `&[("env", "test")]`.
fn pairs(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Owned keys from literal rows, so a table row reads as `&["env"]`.
fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// The tags a `ListTagsForResource` answer reports, keyed by tag key. Keyed rather than
/// ordered because `ListTagsForResourceResponse$tags` is a list the model attaches no
/// ordering to; a duplicated key would collapse here, so the entry count is asserted
/// against the list length too.
fn listed_tags(case: &str, answer: &Answer) -> BTreeMap<String, String> {
    let tags = answer
        .body
        .get("tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags list, got {answer:?}"));
    let keyed: BTreeMap<String, String> = tags
        .iter()
        .map(|tag| {
            let key = tag
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a tag without a key in {answer:?}"))
                .to_string();
            let value = tag
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a tag without a value in {answer:?}"))
                .to_string();
            (key, value)
        })
        .collect();
    assert_eq!(
        keyed.len(),
        tags.len(),
        "case {case:?}: every key must appear at most once, got {answer:?}"
    );
    keyed
}

/// Which taggable resource a case untags. Every one of this service's stored resources
/// carries its own tag list, so the removal rule has to hold for each of them.
#[derive(Debug, Clone, Copy)]
enum Kind {
    TrustAnchor,
    Profile,
    Crl,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::TrustAnchor => "trust-anchor",
            Kind::Profile => "profile",
            Kind::Crl => "crl",
        }
    }
}

/// Create a trust anchor and answer its ARN. A CRL import names one, and `ImportCrl`
/// refuses an ARN that resolves to nothing, so every CRL row needs a real one.
async fn create_trust_anchor_arn(client: &Client, name: &str) -> String {
    let answer = client
        .send(
            "POST",
            "/trustanchors",
            Some(json!({
                "name": name,
                "source": {
                    "sourceType": "CERTIFICATE_BUNDLE",
                    "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
                },
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "trust anchor {name:?}: expected CreateTrustAnchor 201, got {answer:?}"
    );
    answer
        .body
        .pointer("/trustAnchor/trustAnchorArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("trust anchor {name:?}: no trustAnchorArn in {answer:?}"))
        .to_string()
}

/// Create a resource of `kind` carrying `tags`, and answer its ARN.
async fn create_resource(
    client: &Client,
    kind: Kind,
    name: &str,
    tags: &[(String, String)],
) -> String {
    // A CRL is imported AGAINST a trust anchor, and `ImportCrl` resolves that reference
    // before it stores anything, so a CRL row mints the anchor it links to first.
    let crl_trust_anchor = match kind {
        Kind::Crl => create_trust_anchor_arn(client, &format!("{name}-anchor")).await,
        Kind::TrustAnchor | Kind::Profile => String::new(),
    };
    let (uri, mut body, pointer) = match kind {
        Kind::TrustAnchor => (
            "/trustanchors",
            json!({
                "name": name,
                "source": {
                    "sourceType": "CERTIFICATE_BUNDLE",
                    "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
                },
            }),
            "/trustAnchor/trustAnchorArn",
        ),
        Kind::Profile => (
            "/profiles",
            json!({
                "name": name,
                "roleArns": [format!("arn:aws:iam::{ACCOUNT}:role/UntaggingDemoRole")],
            }),
            "/profile/profileArn",
        ),
        Kind::Crl => (
            "/crls",
            json!({
                "name": name,
                "crlData": aws_smithy_types::base64::encode(CRL_PEM.as_bytes()),
                "trustAnchorArn": crl_trust_anchor,
            }),
            "/crl/crlArn",
        ),
    };
    if !tags.is_empty() {
        body["tags"] = tag_json(tags);
    }
    let answer = client.send("POST", uri, Some(body)).await;
    assert_eq!(
        answer.status,
        201,
        "case {:?}: expected the {} create to answer 201, got {answer:?}",
        name,
        kind.label()
    );
    answer
        .body
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {name:?}: no ARN at {pointer} in {answer:?}"))
        .to_string()
}

/// One row of the removal table: the tag set the resource holds, the successive
/// `UntagResource` calls applied to it, and the set a later read must report.
struct UntagCase {
    name: &'static str,
    held: Vec<(String, String)>,
    batches: Vec<Vec<String>>,
    expected: Vec<(String, String)>,
}

fn untag_cases() -> Vec<UntagCase> {
    // The whole published `TagKey` character class:
    // `@pattern("^[ a-zA-Z0-9_.:/=+@-]*$")`.
    let punctuated_key = " key_.:/=+@-0Z".to_string();
    let longest_key = "k".repeat(MAX_TAG_KEY_LENGTH);
    let shortest_key = "k".repeat(MIN_TAG_KEY_LENGTH);
    vec![
        UntagCase {
            name: "removes-the-only-key",
            held: pairs(&[("env", "test")]),
            batches: vec![keys(&["env"])],
            expected: Vec::new(),
        },
        UntagCase {
            name: "removes-one-key-and-keeps-the-rest",
            held: pairs(&[("a", "1"), ("b", "2"), ("c", "3")]),
            batches: vec![keys(&["b"])],
            expected: pairs(&[("a", "1"), ("c", "3")]),
        },
        UntagCase {
            name: "removes-several-keys-in-one-call",
            held: pairs(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            batches: vec![keys(&["a", "c"])],
            expected: pairs(&[("b", "2"), ("d", "4")]),
        },
        UntagCase {
            name: "removes-every-key",
            held: pairs(&[("a", "1"), ("b", "2")]),
            batches: vec![keys(&["b", "a"])],
            expected: Vec::new(),
        },
        UntagCase {
            name: "an-empty-key-list-changes-nothing",
            held: pairs(&[("kept", "value")]),
            batches: vec![Vec::new()],
            expected: pairs(&[("kept", "value")]),
        },
        UntagCase {
            name: "a-key-the-resource-does-not-carry-changes-nothing",
            held: pairs(&[("kept", "value")]),
            batches: vec![keys(&["absent"])],
            expected: pairs(&[("kept", "value")]),
        },
        UntagCase {
            name: "held-and-absent-keys-mixed-in-one-call",
            held: pairs(&[("kept", "value"), ("dropped", "value")]),
            batches: vec![keys(&["absent", "dropped", "also-absent"])],
            expected: pairs(&[("kept", "value")]),
        },
        UntagCase {
            name: "repeating-a-key-inside-one-call-removes-it-once",
            held: pairs(&[("dup", "value"), ("kept", "value")]),
            batches: vec![keys(&["dup", "dup", "dup"])],
            expected: pairs(&[("kept", "value")]),
        },
        UntagCase {
            name: "repeating-the-whole-call-is-idempotent",
            held: pairs(&[("gone", "value"), ("kept", "value")]),
            batches: vec![keys(&["gone"]), keys(&["gone"]), keys(&["gone"])],
            expected: pairs(&[("kept", "value")]),
        },
        UntagCase {
            name: "successive-calls-each-remove-their-own-key",
            held: pairs(&[("a", "1"), ("b", "2"), ("c", "3")]),
            batches: vec![keys(&["a"]), keys(&["c"])],
            expected: pairs(&[("b", "2")]),
        },
        UntagCase {
            name: "published-key-length-bounds",
            held: vec![
                (longest_key.clone(), "longest".to_string()),
                (shortest_key.clone(), "shortest".to_string()),
                ("kept".to_string(), "value".to_string()),
            ],
            batches: vec![vec![longest_key, shortest_key]],
            expected: pairs(&[("kept", "value")]),
        },
        UntagCase {
            name: "the-whole-published-character-class",
            held: vec![
                (punctuated_key.clone(), "punctuated".to_string()),
                ("kept".to_string(), "value".to_string()),
            ],
            batches: vec![vec![punctuated_key]],
            expected: pairs(&[("kept", "value")]),
        },
        UntagCase {
            name: "a-key-whose-value-is-empty-is-still-removable",
            held: vec![
                ("empty-valued".to_string(), String::new()),
                ("kept".to_string(), "value".to_string()),
            ],
            batches: vec![keys(&["empty-valued"])],
            expected: pairs(&[("kept", "value")]),
        },
        UntagCase {
            name: "a-key-differing-only-in-case-is-a-different-key",
            held: pairs(&[("Env", "upper"), ("env", "lower")]),
            batches: vec![keys(&["env"])],
            expected: pairs(&[("Env", "upper")]),
        },
    ]
}

/// `UntagResource` removes exactly the named keys from the stored set, and
/// `ListTagsForResource` answers with what is left — for every taggable resource type of
/// this service, and whether the tags came from the create or from `TagResource`.
#[tokio::test]
async fn untag_resource_removes_the_named_keys_and_leaves_the_rest() {
    let client = Client::new();

    for (index, case) in untag_cases().into_iter().enumerate() {
        for kind in [Kind::TrustAnchor, Kind::Profile, Kind::Crl] {
            // The same row is run twice: once with the tags attached at create time and
            // once with them attached afterwards by `TagResource`, because the rule is
            // about the STORED set, not about how it got there.
            for via_tag_resource in [false, true] {
                let case_name = format!(
                    "{}/{}/{}",
                    case.name,
                    kind.label(),
                    if via_tag_resource {
                        "tagged-after-create"
                    } else {
                        "tagged-at-create"
                    }
                );
                // `ResourceName` is `@pattern("^[ a-zA-Z0-9-_]*$")`, so the row index
                // names the resource rather than the case's own punctuation.
                let resource_name = format!(
                    "untagging-{}-{index}-{}",
                    kind.label(),
                    u8::from(via_tag_resource)
                );
                let arn = if via_tag_resource {
                    let arn = create_resource(&client, kind, &resource_name, &[]).await;
                    let tagged = client.tag_resource(&arn, &case.held).await;
                    assert_eq!(
                        tagged.status, 201,
                        "case {case_name:?}: expected TagResource 201, got {tagged:?}"
                    );
                    arn
                } else {
                    create_resource(&client, kind, &resource_name, &case.held).await
                };

                let before = client.list_tags(&arn).await;
                assert_eq!(
                    listed_tags(&case_name, &before),
                    case.held.iter().cloned().collect::<BTreeMap<_, _>>(),
                    "case {case_name:?}: the set before any removal is the one that was stored"
                );

                for (batch_index, batch) in case.batches.iter().enumerate() {
                    let untagged = client.untag_resource(&arn, batch).await;
                    assert_eq!(
                        untagged.status, 200,
                        "case {case_name:?} batch {batch_index}: expected UntagResource 200, \
                         got {untagged:?}"
                    );
                    assert!(
                        untagged.error_code.is_empty(),
                        "case {case_name:?} batch {batch_index}: expected no error code, \
                         got {untagged:?}"
                    );
                }

                let listed = client.list_tags(&arn).await;
                assert_eq!(
                    listed.status, 200,
                    "case {case_name:?}: expected ListTagsForResource 200, got {listed:?}"
                );
                assert_eq!(
                    listed_tags(&case_name, &listed),
                    case.expected.iter().cloned().collect::<BTreeMap<_, _>>(),
                    "case {case_name:?}: the remaining tag set must be what the read reports"
                );

                // Removing every remaining key leaves the resource with no tags at all,
                // and the resource itself still resolves: untagging never deletes it.
                let remaining: Vec<String> =
                    case.expected.iter().map(|(key, _)| key.clone()).collect();
                let cleared = client.untag_resource(&arn, &remaining).await;
                assert_eq!(
                    cleared.status, 200,
                    "case {case_name:?}: expected the clearing UntagResource 200, got {cleared:?}"
                );
                let empty = client.list_tags(&arn).await;
                assert_eq!(
                    listed_tags(&case_name, &empty),
                    BTreeMap::new(),
                    "case {case_name:?}: removing every key must leave an empty tag set"
                );
            }
        }
    }
}

/// Tags belong to the resource the ARN names: untagging one resource leaves every other
/// resource's tags exactly as they were, whatever its type.
#[tokio::test]
async fn untagging_one_resource_leaves_the_others_untouched() {
    let client = Client::new();
    // Each resource, with the tag set it must report at every point in the walk below.
    let mut resources: Vec<(&'static str, String, BTreeMap<String, String>)> = Vec::new();
    for (label, kind) in [
        ("anchor-a", Kind::TrustAnchor),
        ("anchor-b", Kind::TrustAnchor),
        ("profile-a", Kind::Profile),
        ("crl-a", Kind::Crl),
    ] {
        let held = pairs(&[("owner", label), ("shared", "shared-value")]);
        let arn = create_resource(&client, kind, label, &held).await;
        resources.push((label, arn, held.into_iter().collect()));
    }

    for index in 0..resources.len() {
        let (target, target_arn, _) = resources[index].clone();
        // "shared" is a key EVERY resource carries, so a removal that reached another
        // resource's set would show up immediately.
        let untagged = client
            .untag_resource(&target_arn, &keys(&["shared"]))
            .await;
        assert_eq!(
            untagged.status, 200,
            "case {target:?}: expected UntagResource 200, got {untagged:?}"
        );
        resources[index].2.remove("shared");

        for (other, other_arn, expected) in &resources {
            let listed = client.list_tags(other_arn).await;
            assert_eq!(
                listed_tags(other, &listed),
                *expected,
                "case untag-{target:?}/read-{other:?}: untagging one resource must not change \
                 another's tags, got {listed:?}"
            );
        }
    }
}

/// An ARN no stored resource carries is a 404 `ResourceNotFoundException` on the removal
/// path too — never a canned success.
#[tokio::test]
async fn untag_resource_refuses_an_arn_no_resource_carries() {
    let client = Client::new();
    // A real tagged resource, so the store is not simply empty, plus one that is deleted
    // again: an ARN stops resolving exactly when the resource it names is gone.
    let live = create_resource(
        &client,
        Kind::TrustAnchor,
        "live-anchor",
        &pairs(&[("env", "test")]),
    )
    .await;
    let doomed = create_resource(&client, Kind::TrustAnchor, "doomed-anchor", &[]).await;
    let doomed_id = doomed.rsplit('/').next().unwrap_or_default().to_string();
    let deleted = client.delete_trust_anchor(&doomed_id).await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteTrustAnchor 200, got {deleted:?}"
    );

    let unknown = [
        (
            "never-minted-trust-anchor",
            format!(
                "arn:aws:rolesanywhere:{REGION}:{ACCOUNT}:trust-anchor/\
                 00000000-0000-4000-8000-000000000000"
            ),
        ),
        (
            "never-minted-profile",
            format!(
                "arn:aws:rolesanywhere:{REGION}:{ACCOUNT}:profile/\
                 abcdef12-3456-7890-abcd-ef1234567890"
            ),
        ),
        (
            "never-minted-crl",
            format!(
                "arn:aws:rolesanywhere:{REGION}:{ACCOUNT}:crl/\
                 11111111-2222-3333-4444-555555555555"
            ),
        ),
        (
            "another-partition",
            format!(
                "arn:aws-us-gov:rolesanywhere:us-gov-west-1:{ACCOUNT}:trust-anchor/\
                 22222222-3333-4444-5555-666666666666"
            ),
        ),
        ("deleted-trust-anchor", doomed.clone()),
        ("another-region", live.replace(REGION, "eu-central-1")),
    ];

    for (case, arn) in &unknown {
        let untagged = client.untag_resource(arn, &keys(&["env"])).await;
        assert_eq!(
            untagged.status, 404,
            "case {case:?}: expected UntagResource 404 for {arn:?}, got {untagged:?}"
        );
        assert_eq!(
            untagged.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {untagged:?}"
        );
        assert_eq!(
            untagged.body.get("message").and_then(Value::as_str),
            Some(format!("The item doesn't exist. arn: {arn} ; aborting.").as_str()),
            "case {case:?}: expected the ARN-keyed not-found wording, got {untagged:?}"
        );

        // An empty key list is still a 404: the ARN is resolved before anything else.
        let empty_keys = client.untag_resource(arn, &[]).await;
        assert_eq!(
            (empty_keys.status, empty_keys.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected UntagResource 404 for an empty key list, got {empty_keys:?}"
        );
    }

    // The live resource still carries its tag: a refused call wrote nothing anywhere.
    let listed = client.list_tags(&live).await;
    assert_eq!(
        listed_tags("live-anchor", &listed),
        pairs(&[("env", "test")]).into_iter().collect::<BTreeMap<_, _>>(),
        "a refused UntagResource must not untag any other resource, got {listed:?}"
    );
}

/// The removal is real state, not a projection: the keys it frees are available again
/// under the per-resource tag quota, and a key list past the published `TagKeyList`
/// maximum is refused at the boundary.
#[tokio::test]
async fn untag_resource_frees_room_under_the_per_resource_tag_quota() {
    let client = Client::new();
    let arn = create_resource(&client, Kind::TrustAnchor, "bounded-anchor", &[]).await;

    let full: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
        .collect();
    let at_bound = client.tag_resource(&arn, &full).await;
    assert_eq!(
        at_bound.status, 201,
        "case \"at-the-bound\": {MAX_TAGS_PER_RESOURCE} tags is the per-resource maximum, \
         got {at_bound:?}"
    );

    // At the bound, one more KEY is refused.
    let over = client
        .tag_resource(&arn, &pairs(&[("one-too-many", "x")]))
        .await;
    assert_eq!(
        (over.status, over.error_code.as_str()),
        (400, "ValidationException"),
        "case \"over-the-bound\": a key past the bound must be refused, got {over:?}"
    );

    // Removing one key frees exactly one slot: the same call now succeeds, and the set
    // afterwards is the held one minus the removed key plus the new one.
    let untagged = client.untag_resource(&arn, &keys(&["key-000"])).await;
    assert_eq!(
        untagged.status, 200,
        "case \"free-one-slot\": expected UntagResource 200, got {untagged:?}"
    );
    let accepted = client
        .tag_resource(&arn, &pairs(&[("one-too-many", "x")]))
        .await;
    assert_eq!(
        accepted.status, 201,
        "case \"free-one-slot\": the freed slot must accept a new key, got {accepted:?}"
    );

    let mut expected: BTreeMap<String, String> = full.iter().cloned().collect();
    expected.remove("key-000");
    expected.insert("one-too-many".to_string(), "x".to_string());
    let listed = client.list_tags(&arn).await;
    assert_eq!(
        listed_tags("free-one-slot", &listed),
        expected,
        "case \"free-one-slot\": the read must report the set the removal and the write left"
    );

    // A single call naming more keys than the published `TagKeyList` maximum is refused
    // at the boundary, before the handler runs, and stores nothing.
    let too_many: Vec<String> = (0..=MAX_TAG_KEY_LIST_LENGTH)
        .map(|index| format!("extra-{index:03}"))
        .collect();
    let refused = client.untag_resource(&arn, &too_many).await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (400, "ValidationException"),
        "case \"over-the-list-bound\": {} keys in one call must be refused, got {refused:?}",
        too_many.len()
    );
    let unchanged = client.list_tags(&arn).await;
    assert_eq!(
        listed_tags("over-the-list-bound", &unchanged),
        expected,
        "case \"over-the-list-bound\": a refused call must leave the stored set alone"
    );
}
