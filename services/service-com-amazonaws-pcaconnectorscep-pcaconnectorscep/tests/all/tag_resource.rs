// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `TagResource`.
//!
//! The rule under test: `TagResource` merges the `Tags` it is given into the tag map of the
//! connector or challenge its `ResourceArn` names — a key the resource does not carry yet is
//! added, a key it does carry takes the request's value, and every other key it carries stays
//! — answers the modelled HTTP 204 with an empty body, and the merged map is what a later
//! `ListTagsForResource` reports. A request naming an ARN this service holds no record under
//! answers the tagging layer's own refusal (400 `NotFoundException`, see
//! [`harness::assert_tagging_not_found`]); a request whose entries are not a caller's to set,
//! or that would leave the resource carrying more than the per-resource ceiling, answers 400
//! `ValidationException` and writes nothing.
//!
//! `Tags` is modelled as a plain `map<String, String>` with no `@length` and no `@pattern` on
//! the map or on either half of an entry (smithy_query operation=TagResource), so the tagging
//! rules are the handler's own — the same ones both creates enforce — and the output shape is
//! `Unit`, so 204 with no body is the whole success answer.
//!
//! Every case drives the service the way a client does: one HTTP request per call, through the
//! same `MultiServiceRouter` the emulator boots. The rows differ along what the rule
//! generalises over: the resource kind, how many tags the resource already carried, whether a
//! key is new or already present, the characters and scripts the entries use, the number of
//! entries, and both sides of the per-resource ceiling.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};
use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep as scep;

/// A region that is NOT the emulator default, so an ARN built from the request's own SigV4
/// credential scope is distinguishable from a hardcoded one.
const REGION: &str = "us-west-2";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A private-CA ARN matching `CertificateAuthorityArn`'s own pattern
/// (`^arn:aws(-[a-z]+)*:acm-pca:<region>:<account>:certificate-authority/<uuid>$`).
const CA_ARN: &str = "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555";

/// The per-resource tag ceiling and the two length bounds, named from the service's own
/// constants so a row that sits on a bound cannot drift off it.
const MAX_TAGS_PER_RESOURCE: usize = scep::state::MAX_TAGS_PER_RESOURCE;
const MAX_TAG_KEY_LENGTH: usize = scep::state::MAX_TAG_KEY_LENGTH;
const MAX_TAG_VALUE_LENGTH: usize = scep::state::MAX_TAG_VALUE_LENGTH;

async fn create_connector(router: &MultiServiceRouter, request: Value) -> Answer {
    harness::call(router, REGION, "POST", "/connectors", Some(request)).await
}

async fn create_challenge(router: &MultiServiceRouter, request: Value) -> Answer {
    harness::call(router, REGION, "POST", "/challenges", Some(request)).await
}

async fn delete_challenge(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "DELETE",
        &format!("/challenges/{}", label(arn)),
        None,
    )
    .await
}

async fn delete_connector(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "DELETE",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await
}

async fn get_connector(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "GET",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await
}

async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(router, REGION, "GET", &format!("/tags/{}", label(arn)), None).await
}

async fn tag_resource(router: &MultiServiceRouter, arn: &str, tags: &Tags) -> Answer {
    harness::call(
        router,
        REGION,
        "POST",
        &format!("/tags/{}", label(arn)),
        Some(json!({ "Tags": tags_member(tags) })),
    )
    .await
}

/// The tag pairs of a row, in the order the request sends them.
type Tags = Vec<(String, String)>;

fn pairs(entries: &[(&str, &str)]) -> Tags {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// A `Tags` member for a request body.
fn tags_member(tags: &Tags) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// The `Tags` body a map should come back as from a listing.
fn expected_body(tags: &BTreeMap<String, String>) -> Map<String, Value> {
    tags.iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect()
}

/// Which of this service's two taggable resources a row tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Connector,
    Challenge,
}

/// One row: the tags the create stored, the tags the `TagResource` call sends, and whether
/// they are a caller's to set.
#[derive(Debug)]
struct Case {
    name: String,
    initial: Tags,
    add: Tags,
    accepted: bool,
}

/// The map a row expects the resource to carry afterwards: the merge for an accepted row, and
/// exactly what the create stored for a refused one, since a refusal writes nothing.
fn expected_tags(case: &Case) -> BTreeMap<String, String> {
    let mut tags: BTreeMap<String, String> = case.initial.iter().cloned().collect();
    if case.accepted {
        for (key, value) in &case.add {
            tags.insert(key.clone(), value.clone());
        }
    }
    tags
}

/// The rows.
///
/// The accepted ones walk adding, overwriting and both at once, over resources that started
/// with no tags, with some, and with the whole per-resource ceiling; the refused ones walk each
/// way a request can fall outside the rule — the character alphabet, both length bounds, the
/// reserved key namespace, and the entry ceiling from the other side. The punctuation row
/// carries the whole published set of non-alphanumeric tag characters (`_ . : / = + - @` and
/// the space), from the tag alphabet the service's own `state` module documents.
fn cases() -> Vec<Case> {
    let mut cases = vec![
        Case {
            name: "the first tag on a resource created without any".to_string(),
            initial: Tags::new(),
            add: pairs(&[("customer", "Example Corp")]),
            accepted: true,
        },
        Case {
            name: "a second key beside the one the create stored".to_string(),
            initial: pairs(&[("environment", "production")]),
            add: pairs(&[("owner", "platform")]),
            accepted: true,
        },
        Case {
            name: "a new value for a key the resource already carries".to_string(),
            initial: pairs(&[("environment", "staging"), ("owner", "platform")]),
            add: pairs(&[("environment", "production")]),
            accepted: true,
        },
        Case {
            name: "one key added and one overwritten in the same call".to_string(),
            initial: pairs(&[("stage", "one"), ("zone", "a")]),
            add: pairs(&[("zone", "b"), ("app", "checkout")]),
            accepted: true,
        },
        Case {
            name: "an empty value replacing a non-empty one".to_string(),
            initial: pairs(&[("owner", "platform")]),
            add: pairs(&[("owner", "")]),
            accepted: true,
        },
        Case {
            name: "an empty Tags map, which changes nothing".to_string(),
            initial: pairs(&[("keep", "me")]),
            add: Tags::new(),
            accepted: true,
        },
        Case {
            name: "letters and numbers from outside ASCII, and every punctuation character the \
                   tag alphabet allows"
                .to_string(),
            initial: pairs(&[("ascii", "yes")]),
            add: pairs(&[
                ("équipe-sécurité", "東京 ٣"),
                ("a_b.c:d/e=f+g-h@i", "1 2_3.4:5/6=7+8-9@0"),
            ]),
            accepted: true,
        },
        Case {
            name: "the reserved namespace away from the start of the key".to_string(),
            initial: Tags::new(),
            add: pairs(&[("team:aws:owner", "platform")]),
            accepted: true,
        },
        Case {
            name: "a key in the reserved namespace".to_string(),
            initial: pairs(&[("environment", "production")]),
            add: pairs(&[("aws:created-by", "me")]),
            accepted: false,
        },
        Case {
            name: "the reserved namespace spelled in capitals".to_string(),
            initial: Tags::new(),
            add: pairs(&[("AWS:Owner", "mine")]),
            accepted: false,
        },
        Case {
            name: "an empty key".to_string(),
            initial: Tags::new(),
            add: pairs(&[("", "empty-key")]),
            accepted: false,
        },
        Case {
            name: "a character outside the tag alphabet in the key".to_string(),
            initial: pairs(&[("keep", "me")]),
            add: pairs(&[("bad*key", "x")]),
            accepted: false,
        },
        Case {
            name: "a character outside the tag alphabet in the value".to_string(),
            initial: pairs(&[("keep", "me")]),
            add: pairs(&[("payload", "<not-a-tag-value>")]),
            accepted: false,
        },
        Case {
            name: "a tab in the value, which is a control character and not a space".to_string(),
            initial: Tags::new(),
            add: pairs(&[("indented", "a\tb")]),
            accepted: false,
        },
        Case {
            name: "one entry a caller may not set among entries it may".to_string(),
            initial: pairs(&[("keep", "me")]),
            add: pairs(&[
                ("environment", "production"),
                ("aws:created-by", "me"),
                ("owner", "platform"),
            ]),
            accepted: false,
        },
    ];

    // Both length bounds, each at its published maximum and one character past it.
    for (half, length, accepted) in [
        ("key", MAX_TAG_KEY_LENGTH, true),
        ("key", MAX_TAG_KEY_LENGTH + 1, false),
        ("value", MAX_TAG_VALUE_LENGTH, true),
        ("value", MAX_TAG_VALUE_LENGTH + 1, false),
    ] {
        let filler: String = std::iter::repeat_n('é', length).collect();
        let (key, value) = if half == "key" {
            (filler, "v".to_string())
        } else {
            ("sized".to_string(), filler)
        };
        cases.push(Case {
            name: format!("a {half} of {length} characters"),
            initial: Tags::new(),
            add: vec![(key, value)],
            accepted,
        });
    }

    // The per-resource ceiling, from both sides and with an overwrite that does not grow the
    // map: the bound is on what the resource ENDS UP carrying, not on what one call sends.
    let filled: Tags = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:03}"), format!("value-{index}")))
        .collect();
    let one_short: Tags = filled[..MAX_TAGS_PER_RESOURCE - 1].to_vec();
    cases.push(Case {
        name: "the entry that fills the per-resource ceiling exactly".to_string(),
        initial: one_short.clone(),
        add: pairs(&[("last", "one")]),
        accepted: true,
    });
    cases.push(Case {
        name: "a new key on a resource already at the per-resource ceiling".to_string(),
        initial: filled.clone(),
        add: pairs(&[("one-too-many", "x")]),
        accepted: false,
    });
    cases.push(Case {
        name: "an overwrite on a resource already at the per-resource ceiling".to_string(),
        initial: filled.clone(),
        add: pairs(&[("key-000", "rewritten")]),
        accepted: true,
    });
    cases.push(Case {
        name: "one call carrying more entries than a resource may have".to_string(),
        initial: Tags::new(),
        add: (0..MAX_TAGS_PER_RESOURCE + 1)
            .map(|index| (format!("sent-{index:03}"), format!("value-{index}")))
            .collect(),
        accepted: false,
    });
    cases
}

/// Create a connector carrying `tags` and make the `ACTIVE` the clock owes it due, so it can
/// mint challenges.
async fn active_connector(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    tags: &Tags,
) -> String {
    let mut request = json!({ "CertificateAuthorityArn": CA_ARN });
    request["Tags"] = tags_member(tags);
    let created = create_connector(router, request).await;
    assert_eq!(
        created.status, 202,
        "expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    let arn = created.text("ConnectorArn");
    harness::release_pending_if_owed(engine, &arn);
    arn
}

/// Create the resource a row tags, carrying the tags the row says the create stored, and
/// answer its ARN.
async fn resource_of(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    kind: Kind,
    name: &str,
    tags: &Tags,
) -> String {
    match kind {
        Kind::Connector => active_connector(router, engine, tags).await,
        Kind::Challenge => {
            let connector = active_connector(router, engine, &Tags::new()).await;
            let created = create_challenge(
                router,
                json!({ "ConnectorArn": connector, "Tags": tags_member(tags) }),
            )
            .await;
            assert_eq!(
                created.status, 202,
                "case {name:?}: expected 202 from CreateChallenge, got {} / {:?}",
                created.status, created.body
            );
            created.body["Challenge"]["Arn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {name:?}: expected a challenge Arn in {:?}", created.body)
                })
                .to_string()
        }
    }
}

#[tokio::test]
async fn tag_resource_merges_the_requested_tags_into_the_named_resource() {
    for kind in [Kind::Connector, Kind::Challenge] {
        for case in cases() {
            let case_name = format!("{} on a {kind:?}", case.name);
            let (router, engine) = new_router();
            let arn = resource_of(&router, &engine, kind, &case_name, &case.initial).await;
            let expected = expected_body(&expected_tags(&case));

            let tagged = tag_resource(&router, &arn, &case.add).await;
            if case.accepted {
                assert_eq!(
                    tagged.status, 204,
                    "case {case_name:?}: expected the modelled 204, got {} / {:?}",
                    tagged.status, tagged.body
                );
                assert!(
                    tagged.raw.is_empty(),
                    "case {case_name:?}: TagResource models no output members, so the body must \
                     be empty, got {:?}",
                    String::from_utf8_lossy(&tagged.raw)
                );
            } else {
                assert_eq!(
                    tagged.status, 400,
                    "case {case_name:?}: expected 400, got {} / {:?}",
                    tagged.status, tagged.body
                );
                assert_eq!(
                    tagged.error_type, "ValidationException",
                    "case {case_name:?}: expected ValidationException, got {:?} / {:?}",
                    tagged.error_type, tagged.body
                );
                assert!(
                    !tagged.body["Message"]
                        .as_str()
                        .unwrap_or_default()
                        .is_empty(),
                    "case {case_name:?}: expected a Message saying why, got {:?}",
                    tagged.body
                );
            }

            // The merge (or, for a refusal, the untouched map) is what the resource reports.
            let listed = list_tags(&router, &arn).await;
            assert_eq!(
                listed.status, 200,
                "case {case_name:?}: expected 200 from ListTagsForResource, got {} / {:?}",
                listed.status, listed.body
            );
            assert_eq!(
                listed.tags(),
                &expected,
                "case {case_name:?}: expected {expected:?}, got {:?}",
                listed.tags()
            );

            // Sending the same request again lands on the same map: a merge of what is
            // already there changes nothing.
            let again = tag_resource(&router, &arn, &case.add).await;
            assert_eq!(
                again.status,
                if case.accepted { 204 } else { 400 },
                "case {case_name:?}: expected the repeated call to answer the same, got {} / {:?}",
                again.status,
                again.body
            );
            let relisted = list_tags(&router, &arn).await;
            assert_eq!(
                relisted.tags(),
                &expected,
                "case {case_name:?}: repeating the call must leave the same tags {expected:?}, \
                 got {:?}",
                relisted.tags()
            );
        }
    }
}

#[tokio::test]
async fn tagging_one_resource_leaves_every_other_resource_alone() {
    let (router, engine) = new_router();

    let first = active_connector(&router, &engine, &pairs(&[("which", "first")])).await;
    let second = active_connector(&router, &engine, &pairs(&[("which", "second")])).await;
    let created = create_challenge(
        &router,
        json!({ "ConnectorArn": first, "Tags": tags_member(&pairs(&[("which", "challenge")])) }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "expected 202 from CreateChallenge, got {} / {:?}",
        created.status, created.body
    );
    let challenge = created.body["Challenge"]["Arn"]
        .as_str()
        .expect("a challenge Arn")
        .to_string();

    let tagged = tag_resource(&router, &first, &pairs(&[("owner", "platform")])).await;
    assert_eq!(
        tagged.status, 204,
        "expected 204 from TagResource, got {} / {:?}",
        tagged.status, tagged.body
    );

    for (name, arn, expected) in [
        (
            "the resource that was tagged",
            first.as_str(),
            pairs(&[("owner", "platform"), ("which", "first")]),
        ),
        (
            "another connector",
            second.as_str(),
            pairs(&[("which", "second")]),
        ),
        (
            "a challenge of the tagged connector",
            challenge.as_str(),
            pairs(&[("which", "challenge")]),
        ),
    ] {
        let listed = list_tags(&router, arn).await;
        let expected = expected_body(&expected.into_iter().collect());
        assert_eq!(
            listed.tags(),
            &expected,
            "case {name:?}: expected {expected:?}, got {:?}",
            listed.tags()
        );
    }

    // Tagging is not a configuration change: the connector is still the one that was created,
    // in the status the clock settled it into.
    let read = get_connector(&router, &first).await;
    assert_eq!(
        read.status, 200,
        "expected 200 from GetConnector after tagging, got {} / {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.body["Connector"]["CertificateAuthorityArn"].as_str(),
        Some(CA_ARN),
        "expected the connector's own CA ARN after tagging, got {:?}",
        read.body
    );
    assert_eq!(
        read.body["Connector"]["Status"].as_str(),
        Some("ACTIVE"),
        "expected tagging to leave the connector's status alone, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn tagging_an_arn_this_service_holds_no_record_under_is_not_found() {
    let (router, engine) = new_router();

    // A live connector and challenge, so the refusals below are about the ARN and not about
    // an empty store.
    let connector = active_connector(&router, &engine, &pairs(&[("keep", "me")])).await;
    let created = create_challenge(&router, json!({ "ConnectorArn": connector })).await;
    assert_eq!(
        created.status, 202,
        "expected 202 from CreateChallenge, got {} / {:?}",
        created.status, created.body
    );
    let challenge = created.body["Challenge"]["Arn"]
        .as_str()
        .expect("a challenge Arn")
        .to_string();

    let never_created_connector = format!(
        "arn:aws:pca-connector-scep:{REGION}:{ACCOUNT}:connector/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
    );
    let never_created_challenge =
        format!("{never_created_connector}/challenge/99999999-8888-7777-6666-555544443333");
    let foreign = format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/not-a-scep-resource");
    let unknown_challenge_of_a_live_connector =
        format!("{connector}/challenge/11111111-1111-1111-1111-111111111111");

    // The challenge and then the connector are deleted, so the last two rows name records this
    // service held and no longer does.
    assert_eq!(
        delete_challenge(&router, &challenge).await.status,
        202,
        "expected 202 from DeleteChallenge"
    );
    assert_eq!(
        delete_connector(&router, &connector).await.status,
        202,
        "expected 202 from DeleteConnector"
    );
    harness::release_pending_if_owed(&engine, &connector);

    for (name, arn) in [
        (
            "connector ARN nothing was created under",
            never_created_connector.as_str(),
        ),
        (
            "challenge ARN nothing was created under",
            never_created_challenge.as_str(),
        ),
        (
            // No challenge segment, so the ARN can only have been meant for a connector.
            "ARN of another service",
            foreign.as_str(),
        ),
        (
            "challenge of a live connector that was never created",
            unknown_challenge_of_a_live_connector.as_str(),
        ),
        ("deleted challenge", challenge.as_str()),
        (
            "connector whose teardown has come due",
            connector.as_str(),
        ),
    ] {
        let tagged = tag_resource(&router, arn, &pairs(&[("owner", "platform")])).await;
        harness::assert_tagging_not_found(&tagged, name);

        // A refused call creates nothing: the ARN still names no record, and the listing of
        // its tags is refused the same way.
        let listed = list_tags(&router, arn).await;
        harness::assert_tagging_not_found(&listed, name);
    }
}
