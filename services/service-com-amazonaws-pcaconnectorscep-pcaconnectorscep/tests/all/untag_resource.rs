// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UntagResource`.
//!
//! The rule under test: `UntagResource` drops exactly the tags its `TagKeys` names from the
//! connector or challenge its `ResourceArn` names — a key the resource carries goes with its
//! value, a key it does not carry drops nothing, and every other key it carries stays —
//! answers the modelled HTTP 204 with an empty body, and what is left is what a later
//! `ListTagsForResource` reports. Because a key that is not there drops nothing, the call is
//! idempotent, as the model marks it. A request naming an ARN this service holds no record
//! under answers the tagging layer's own refusal (400 `NotFoundException`, see
//! [`harness::assert_tagging_not_found`]); a request naming something that is not a tag
//! key answers 400 `ValidationException` and drops nothing.
//!
//! `TagKeys` is modelled as a `list<String>` bound to the `tagKeys` query parameter, with no
//! `@length` and no `@pattern` on the list or on its member, and the output shape is `Unit`
//! (smithy_query operation=UntagResource), so the tag-key rules are the handler's own — the
//! same ones every other tagging operation enforces — and 204 with no body is the whole
//! success answer.
//!
//! Every case drives the service the way a client does: one HTTP request per call, through the
//! same `MultiServiceRouter` the emulator boots. The rows differ along what the rule
//! generalises over: the resource kind, how many tags the resource carried, how many keys the
//! request names, whether each named key is one the resource carries, the order and
//! repetition of those keys, the characters and scripts they use, and each way a named key can
//! fall outside what a tag key is.

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

/// The per-resource tag ceiling and the tag-key length bound, named from the service's own
/// constants so a row that sits on a bound cannot drift off it.
const MAX_TAGS_PER_RESOURCE: usize = scep::state::MAX_TAGS_PER_RESOURCE;
const MAX_TAG_KEY_LENGTH: usize = scep::state::MAX_TAG_KEY_LENGTH;

/// The service's own status and reason vocabulary.
const STATUS_DELETING: &str = scep::state::STATUS_DELETING;
const REASON_INVALID_STATE: &str = scep::state::REASON_INVALID_STATE;

async fn create_connector(router: &MultiServiceRouter, request: Value) -> Answer {
    harness::call(router, REGION, "POST", "/connectors", Some(request)).await
}

async fn create_challenge(router: &MultiServiceRouter, request: Value) -> Answer {
    harness::call(router, REGION, "POST", "/challenges", Some(request)).await
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

async fn get_challenge_metadata(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "GET",
        &format!("/challengeMetadata/{}", label(arn)),
        None,
    )
    .await
}

async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(router, REGION, "GET", &format!("/tags/{}", label(arn)), None).await
}

async fn tag_resource(router: &MultiServiceRouter, arn: &str, tags: &[(String, String)]) -> Answer {
    harness::call(
        router,
        REGION,
        "POST",
        &format!("/tags/{}", label(arn)),
        Some(json!({ "Tags": tags_member(tags) })),
    )
    .await
}

/// One `UntagResource` call: `TagKeys` is bound to the `tagKeys` query parameter, so each key
/// is one repetition of it, percent-encoded the way an SDK encodes a query value.
async fn untag_resource(router: &MultiServiceRouter, arn: &str, keys: &[String]) -> Answer {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", label(key)))
        .collect();
    let path = if query.is_empty() {
        format!("/tags/{}", label(arn))
    } else {
        format!("/tags/{}?{}", label(arn), query.join("&"))
    };
    harness::call(router, REGION, "DELETE", &path, None).await
}

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// A `Tags` member for a request body.
fn tags_member(tags: &[(String, String)]) -> Value {
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

/// Which of this service's two taggable resources a row untags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Connector,
    Challenge,
}

/// One row: the tags the resource carries, the keys the `UntagResource` call names, and whether
/// the call is one the service accepts.
#[derive(Debug)]
struct Case {
    name: String,
    initial: Vec<(String, String)>,
    remove: Vec<String>,
    accepted: bool,
}

/// One row of the timestamp table: the tags the resource carries, the keys the call names, and
/// whether the call drops anything at all — which is what decides whether the record changed.
#[derive(Debug)]
struct TimestampCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    remove: Vec<String>,
    drops: bool,
}

/// What the row's resource carries afterwards: the initial map minus every named key for an
/// accepted row, and exactly the initial map for a refused one, since a refusal drops nothing.
fn expected_tags(case: &Case) -> BTreeMap<String, String> {
    let mut tags: BTreeMap<String, String> = case.initial.iter().cloned().collect();
    if case.accepted {
        for key in &case.remove {
            tags.remove(key);
        }
    }
    tags
}

/// The rows.
///
/// The accepted ones walk removing one key, several, all of them, keys the resource does not
/// carry, a mix of both, the same key named twice, and keys written in scripts and punctuation
/// from the whole tag alphabet; the refused ones walk each way a named key can fall outside
/// what a tag key is — the character alphabet, both ends of the length bound, and the reserved
/// namespace. The punctuation row carries the whole published set of non-alphanumeric tag
/// characters (`_ . : / = + - @` and the space), from the tag alphabet the service's own
/// `state` module documents.
fn cases() -> Vec<Case> {
    let mut cases = vec![
        Case {
            name: "the only tag the resource carries".to_string(),
            initial: pairs(&[("owner", "platform")]),
            remove: owned(&["owner"]),
            accepted: true,
        },
        Case {
            name: "one of several tags, leaving the rest".to_string(),
            initial: pairs(&[
                ("environment", "production"),
                ("owner", "platform"),
                ("Name", "scep connector"),
            ]),
            remove: owned(&["owner"]),
            accepted: true,
        },
        Case {
            name: "two of three tags in one call".to_string(),
            initial: pairs(&[("a", "1"), ("b", "2"), ("c", "3")]),
            remove: owned(&["a", "c"]),
            accepted: true,
        },
        Case {
            name: "every tag the resource carries".to_string(),
            initial: pairs(&[("a", "1"), ("b", "2")]),
            remove: owned(&["b", "a"]),
            accepted: true,
        },
        Case {
            name: "a key the resource does not carry, which drops nothing".to_string(),
            initial: pairs(&[("keep", "me")]),
            remove: owned(&["absent"]),
            accepted: true,
        },
        Case {
            name: "one key the resource carries and one it does not".to_string(),
            initial: pairs(&[("keep", "me"), ("drop", "this")]),
            remove: owned(&["drop", "never-set"]),
            accepted: true,
        },
        Case {
            name: "the same key named twice in one call".to_string(),
            initial: pairs(&[("keep", "me"), ("drop", "this")]),
            remove: owned(&["drop", "drop"]),
            accepted: true,
        },
        Case {
            name: "a key whose tag carries an empty value".to_string(),
            initial: pairs(&[("owner", ""), ("keep", "me")]),
            remove: owned(&["owner"]),
            accepted: true,
        },
        Case {
            name: "a key on a resource with no tags at all".to_string(),
            initial: Vec::new(),
            remove: owned(&["anything"]),
            accepted: true,
        },
        Case {
            name: "keys of letters and numbers from outside ASCII, and of every punctuation \
                   character the tag alphabet allows"
                .to_string(),
            initial: pairs(&[
                ("équipe-sécurité", "東京 ٣"),
                ("a_b.c:d/e=f+g-h@i", "1 2_3.4:5/6=7+8-9@0"),
                ("ascii", "yes"),
            ]),
            remove: owned(&["équipe-sécurité", "a_b.c:d/e=f+g-h@i"]),
            accepted: true,
        },
        Case {
            name: "a key holding the reserved namespace away from its start".to_string(),
            initial: pairs(&[("team:aws:owner", "platform"), ("keep", "me")]),
            remove: owned(&["team:aws:owner"]),
            accepted: true,
        },
        Case {
            name: "a key differing from a stored one only in case, which is a different key"
                .to_string(),
            initial: pairs(&[("Owner", "platform")]),
            remove: owned(&["owner"]),
            accepted: true,
        },
        Case {
            name: "an empty key".to_string(),
            initial: pairs(&[("keep", "me")]),
            remove: owned(&[""]),
            accepted: false,
        },
        Case {
            name: "a character outside the tag alphabet in the key".to_string(),
            initial: pairs(&[("keep", "me")]),
            remove: owned(&["bad*key"]),
            accepted: false,
        },
        Case {
            name: "a newline in the key, which is a control character and not a space"
                .to_string(),
            initial: pairs(&[("keep", "me")]),
            remove: owned(&["two\nlines"]),
            accepted: false,
        },
        Case {
            name: "a key in the reserved namespace".to_string(),
            initial: pairs(&[("keep", "me")]),
            remove: owned(&["aws:cloudformation:stack-name"]),
            accepted: false,
        },
        Case {
            name: "the reserved namespace spelled in capitals".to_string(),
            initial: pairs(&[("keep", "me")]),
            remove: owned(&["AWS:Owner"]),
            accepted: false,
        },
        Case {
            name: "one key a caller may not name among keys it may".to_string(),
            initial: pairs(&[("keep", "me"), ("drop", "this")]),
            remove: owned(&["drop", "aws:created-by"]),
            accepted: false,
        },
    ];

    // The tag-key length bound, at its published maximum and one character past it, in a
    // multi-byte script: the bound counts CHARACTERS, so a key of 128 two-byte letters names a
    // tag and one of 129 names nothing. A key past the bound is one no create could store, so
    // that row's resource carries only the key the removal must leave alone.
    for (length, accepted) in [
        (1, true),
        (MAX_TAG_KEY_LENGTH, true),
        (MAX_TAG_KEY_LENGTH + 1, false),
    ] {
        let key: String = std::iter::repeat_n('é', length).collect();
        let mut initial = pairs(&[("keep", "me")]);
        if accepted {
            initial.push((key.clone(), "sized".to_string()));
        }
        cases.push(Case {
            name: format!("a key of {length} characters"),
            initial,
            remove: vec![key],
            accepted,
        });
    }

    // A call naming as many keys as a resource may carry, clearing a resource that carries the
    // whole per-resource ceiling: the ceiling bounds what a resource CARRIES, and a removal
    // never grows that map.
    let filled: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:03}"), format!("value-{index}")))
        .collect();
    cases.push(Case {
        name: format!("every one of the {MAX_TAGS_PER_RESOURCE} tags a resource may carry"),
        initial: filled.clone(),
        remove: filled.iter().map(|(key, _)| key.clone()).collect(),
        accepted: true,
    });
    cases.push(Case {
        name: "half the tags of a resource carrying the per-resource ceiling".to_string(),
        initial: filled.clone(),
        remove: filled
            .iter()
            .step_by(2)
            .map(|(key, _)| key.clone())
            .collect(),
        accepted: true,
    });
    cases.push(Case {
        name: format!(
            "more keys than the {MAX_TAGS_PER_RESOURCE} tags a resource may carry, none of them \
             stored"
        ),
        initial: pairs(&[("keep", "me")]),
        remove: (0..MAX_TAGS_PER_RESOURCE + 1)
            .map(|index| format!("absent-{index:03}"))
            .collect(),
        accepted: true,
    });
    cases
}

/// Create a connector carrying `tags` and make the `ACTIVE` the clock owes it due, so it can
/// mint challenges.
async fn active_connector(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    tags: &[(String, String)],
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

/// Create the resource a row untags, carrying the row's tags, and answer its ARN.
async fn resource_of(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    kind: Kind,
    name: &str,
    tags: &[(String, String)],
) -> String {
    match kind {
        Kind::Connector => active_connector(router, engine, tags).await,
        Kind::Challenge => {
            let connector = active_connector(router, engine, &[]).await;
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
            created
                .challenge("Arn")
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {name:?}: expected a challenge Arn in {:?}", created.body)
                })
                .to_string()
        }
    }
}

/// The `UpdatedAt` a resource reports — the instant the record last changed — read through
/// whichever reader its kind has.
async fn updated_at(router: &MultiServiceRouter, kind: Kind, arn: &str) -> f64 {
    let (answer, member) = match kind {
        Kind::Connector => (get_connector(router, arn).await, "Connector"),
        Kind::Challenge => (
            get_challenge_metadata(router, arn).await,
            "ChallengeMetadata",
        ),
    };
    assert_eq!(
        answer.status, 200,
        "expected 200 reading {arn}, got {} / {:?}",
        answer.status, answer.body
    );
    answer.body[member]["UpdatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("expected a numeric UpdatedAt in {:?}", answer.body))
}

#[tokio::test]
async fn untag_resource_drops_exactly_the_named_keys_from_the_named_resource() {
    for kind in [Kind::Connector, Kind::Challenge] {
        for case in cases() {
            let case_name = format!("{} on a {kind:?}", case.name);
            let (router, engine) = new_router();
            let arn = resource_of(&router, &engine, kind, &case_name, &case.initial).await;
            let expected = expected_body(&expected_tags(&case));

            let untagged = untag_resource(&router, &arn, &case.remove).await;
            if case.accepted {
                assert_eq!(
                    untagged.status, 204,
                    "case {case_name:?}: expected the modelled 204, got {} / {:?}",
                    untagged.status, untagged.body
                );
                assert!(
                    untagged.raw.is_empty(),
                    "case {case_name:?}: UntagResource models no output members, so the body \
                     must be empty, got {:?}",
                    String::from_utf8_lossy(&untagged.raw)
                );
            } else {
                assert_eq!(
                    (untagged.status, untagged.error_type.as_str()),
                    (400, "ValidationException"),
                    "case {case_name:?}: expected 400 ValidationException, got {} / {:?}",
                    untagged.status,
                    untagged.body
                );
                assert!(
                    !untagged.message().is_empty(),
                    "case {case_name:?}: expected a Message saying why, got {:?}",
                    untagged.body
                );
            }

            // What is left (or, for a refusal, the untouched map) is what the resource reports.
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

            // The modelled idempotence: the same call again lands on the same map, because a
            // key that is no longer there drops nothing.
            let again = untag_resource(&router, &arn, &case.remove).await;
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
async fn a_removal_moves_updated_at_only_when_it_dropped_something() {
    let rows = vec![
        TimestampCase {
            name: "a key the resource carries",
            initial: pairs(&[("owner", "platform"), ("keep", "me")]),
            remove: owned(&["owner"]),
            drops: true,
        },
        TimestampCase {
            name: "every key the resource carries",
            initial: pairs(&[("owner", "platform"), ("keep", "me")]),
            remove: owned(&["owner", "keep"]),
            drops: true,
        },
        TimestampCase {
            name: "a key the resource does not carry",
            initial: pairs(&[("keep", "me")]),
            remove: owned(&["absent"]),
            drops: false,
        },
        TimestampCase {
            name: "the same key named twice, which drops it once",
            initial: pairs(&[("keep", "me")]),
            remove: owned(&["keep", "keep"]),
            drops: true,
        },
    ];

    for kind in [Kind::Connector, Kind::Challenge] {
        for TimestampCase {
            name,
            initial,
            remove,
            drops,
        } in &rows
        {
            let case_name = format!("{name} on a {kind:?}");
            let (router, engine) = new_router();
            let arn = resource_of(&router, &engine, kind, &case_name, initial).await;

            let before = updated_at(&router, kind, &arn).await;
            let untagged = untag_resource(&router, &arn, remove).await;
            assert_eq!(
                untagged.status, 204,
                "case {case_name:?}: expected 204, got {} / {:?}",
                untagged.status, untagged.body
            );
            let after = updated_at(&router, kind, &arn).await;
            if *drops {
                assert!(
                    after > before,
                    "case {case_name:?}: a removal that dropped a tag must move UpdatedAt past \
                     {before}, got {after}"
                );
            } else {
                assert_eq!(
                    after, before,
                    "case {case_name:?}: a removal that dropped nothing must leave UpdatedAt at \
                     {before}, got {after}"
                );
            }

            // Repeating it now drops nothing, so the instant stays where the first call left it.
            let repeated = untag_resource(&router, &arn, remove).await;
            assert_eq!(
                repeated.status, 204,
                "case {case_name:?}: expected 204 from the repeated call, got {} / {:?}",
                repeated.status, repeated.body
            );
            let settled = updated_at(&router, kind, &arn).await;
            assert_eq!(
                settled, after,
                "case {case_name:?}: a repeated removal drops nothing, so UpdatedAt must stay at \
                 {after}, got {settled}"
            );
        }
    }
}

#[tokio::test]
async fn untagging_one_resource_leaves_every_other_resource_alone() {
    let (router, engine) = new_router();

    let shared = pairs(&[("which", "mine"), ("owner", "platform")]);
    let first = active_connector(&router, &engine, &shared).await;
    let second = active_connector(&router, &engine, &shared).await;
    let created = create_challenge(
        &router,
        json!({ "ConnectorArn": first, "Tags": tags_member(&shared) }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "expected 202 from CreateChallenge, got {} / {:?}",
        created.status, created.body
    );
    let challenge = created
        .challenge("Arn")
        .as_str()
        .expect("a challenge Arn")
        .to_string();

    let untagged = untag_resource(&router, &first, &owned(&["owner"])).await;
    assert_eq!(
        untagged.status, 204,
        "expected 204 from UntagResource, got {} / {:?}",
        untagged.status, untagged.body
    );

    for (name, arn, expected) in [
        (
            "the resource that was untagged",
            first.as_str(),
            pairs(&[("which", "mine")]),
        ),
        ("another connector", second.as_str(), shared.clone()),
        (
            "a challenge of the untagged connector",
            challenge.as_str(),
            shared.clone(),
        ),
    ] {
        let expected = expected_body(&expected.into_iter().collect());
        let listed = list_tags(&router, arn).await;
        assert_eq!(
            listed.tags(),
            &expected,
            "case {name:?}: expected {expected:?}, got {:?}",
            listed.tags()
        );
    }

    // Untagging is not a configuration change: the connector is still the one that was
    // created, in the status the clock settled it into.
    let read = get_connector(&router, &first).await;
    assert_eq!(
        read.connector("CertificateAuthorityArn"),
        &json!(CA_ARN),
        "expected the connector's own CA ARN after untagging, got {:?}",
        read.body
    );
    assert_eq!(
        read.connector("Status"),
        &json!(scep::state::STATUS_ACTIVE),
        "expected untagging to leave the connector's status alone, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn untagging_an_arn_this_service_holds_no_record_under_is_not_found() {
    let (router, engine) = new_router();

    // A live connector, so the refusals below are about the ARN and not about an empty store.
    let connector = active_connector(&router, &engine, &pairs(&[("keep", "me")])).await;
    let created = create_challenge(&router, json!({ "ConnectorArn": connector })).await;
    assert_eq!(
        created.status, 202,
        "expected 202 from CreateChallenge, got {} / {:?}",
        created.status, created.body
    );
    let challenge = created
        .challenge("Arn")
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

    // The connector is torn down, so the last two rows name records this service held and no
    // longer does — the challenge goes with its connector.
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
        (
            "challenge whose connector's teardown has come due",
            challenge.as_str(),
        ),
        (
            "connector whose teardown has come due",
            connector.as_str(),
        ),
    ] {
        let untagged = untag_resource(&router, arn, &owned(&["keep"])).await;
        harness::assert_tagging_not_found(&untagged, name);

        // A refused call creates nothing: the ARN still names no record, and the listing of
        // its tags is refused the same way.
        let listed = list_tags(&router, arn).await;
        harness::assert_tagging_not_found(&listed, name);
    }
}

#[tokio::test]
async fn untagging_a_resource_whose_teardown_was_accepted_is_refused_for_its_state() {
    for kind in [Kind::Connector, Kind::Challenge] {
        let case_name = format!("a {kind:?} whose connector is being torn down");
        let (router, engine) = new_router();
        let initial = pairs(&[("owner", "platform"), ("keep", "me")]);
        let connector = active_connector(&router, &engine, &initial).await;
        let resource = match kind {
            Kind::Connector => connector.clone(),
            Kind::Challenge => {
                let created = create_challenge(
                    &router,
                    json!({ "ConnectorArn": connector, "Tags": tags_member(&initial) }),
                )
                .await;
                assert_eq!(
                    created.status, 202,
                    "case {case_name:?}: expected 202 from CreateChallenge, got {} / {:?}",
                    created.status, created.body
                );
                created
                    .challenge("Arn")
                    .as_str()
                    .expect("a challenge Arn")
                    .to_string()
            }
        };

        // Until the delete is asked for, the very same call is accepted, so the refusal below
        // is about the teardown and not about the keys.
        let accepted = untag_resource(&router, &resource, &owned(&["owner"])).await;
        assert_eq!(
            accepted.status, 204,
            "case {case_name:?}: a removal before the delete must be accepted, got {} / {:?}",
            accepted.status, accepted.body
        );

        assert_eq!(
            delete_connector(&router, &connector).await.status,
            202,
            "case {case_name:?}: expected 202 from DeleteConnector"
        );
        // From here the case is about the teardown window, so pin it.
        harness::hold_pending(&engine, &connector);

        let before = updated_at(&router, kind, &resource).await;
        let refused = untag_resource(&router, &resource, &owned(&["keep"])).await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (400, "ValidationException"),
            "case {case_name:?}: expected 400 ValidationException for a removal on a \
             {STATUS_DELETING} connector, got {} / {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.body["Reason"],
            json!(REASON_INVALID_STATE),
            "case {case_name:?}: expected Reason {REASON_INVALID_STATE}, got {:?}",
            refused.body
        );
        let message = refused.message().to_string();
        assert!(
            message.contains(&connector) && message.contains(STATUS_DELETING),
            "case {case_name:?}: expected a Message naming {connector:?} and {STATUS_DELETING}, \
             got {message:?}"
        );

        // Nothing was dropped, and the record is still REPORTED with what it carried.
        let expected = expected_body(&[("keep".to_string(), "me".to_string())].into_iter().collect());
        let listed = list_tags(&router, &resource).await;
        assert_eq!(
            listed.status, 200,
            "case {case_name:?}: a resource under teardown is still reported, got {} / {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.tags(),
            &expected,
            "case {case_name:?}: expected the refused call to leave {expected:?}, got {:?}",
            listed.tags()
        );
        let after = updated_at(&router, kind, &resource).await;
        assert_eq!(
            after, before,
            "case {case_name:?}: a refused removal must leave the record's UpdatedAt at \
             {before}, got {after}"
        );

        // Once the teardown comes due the record is gone, so the same removal is the tagging
        // layer's not-found refusal instead of the state refusal.
        harness::release_pending(&engine, &connector);
        let gone = untag_resource(&router, &resource, &owned(&["keep"])).await;
        harness::assert_tagging_not_found(&gone, &case_name);
    }
}

#[tokio::test]
async fn a_tag_removed_can_be_written_again() {
    let (router, engine) = new_router();
    let arn = active_connector(&router, &engine, &pairs(&[("owner", "platform")])).await;

    for round in 1..=2 {
        let untagged = untag_resource(&router, &arn, &owned(&["owner"])).await;
        assert_eq!(
            untagged.status, 204,
            "round {round}: expected 204 from UntagResource, got {} / {:?}",
            untagged.status, untagged.body
        );
        let listed = list_tags(&router, &arn).await;
        assert!(
            listed.tags().is_empty(),
            "round {round}: expected the removal to leave no tags, got {:?}",
            listed.tags()
        );

        let tagged = tag_resource(&router, &arn, &pairs(&[("owner", "security")])).await;
        assert_eq!(
            tagged.status, 204,
            "round {round}: expected 204 from TagResource, got {} / {:?}",
            tagged.status, tagged.body
        );
        let expected = expected_body(&pairs(&[("owner", "security")]).into_iter().collect());
        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.tags(),
            &expected,
            "round {round}: expected a removed key to be writable again as {expected:?}, got {:?}",
            listed.tags()
        );
    }
}
