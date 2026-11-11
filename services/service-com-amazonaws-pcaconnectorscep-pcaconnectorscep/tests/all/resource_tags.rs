// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListTagsForResource`.
//!
//! The rule under test: a tag listing reports exactly the tag map stored on the connector or
//! challenge its `ResourceArn` names — whichever of the two kinds the ARN names — and answers
//! the tagging layer's own refusal (400 `NotFoundException`, see
//! [`harness::assert_tagging_not_found`]) when this service holds no record under that ARN.
//!
//! Every case drives the service the way a client does: one HTTP request per call, through the
//! same `MultiServiceRouter` the emulator boots. The rows of the table differ along what the
//! rule generalises over (resource kind, how many tags the create carried, whether the member
//! was there at all, and the characters in the keys and values), so no assertion is pinned to
//! one request.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};

/// A region that is NOT the emulator default, so an ARN built from the request's own SigV4
/// credential scope is distinguishable from a hardcoded one.
const REGION: &str = "ap-southeast-2";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The per-resource tag ceiling this service enforces, named from the service's own constant
/// so the row that fills it cannot drift away from the limit.
const MAX_TAGS: usize =
    service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::state::MAX_TAGS_PER_RESOURCE;

/// A private-CA ARN matching `CertificateAuthorityArn`'s own pattern
/// (`^arn:aws(-[a-z]+)*:acm-pca:<region>:<account>:certificate-authority/<uuid>$`).
const CA_ARN: &str = "arn:aws:acm-pca:ap-southeast-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555";

async fn create_connector(router: &MultiServiceRouter, request: Value) -> Answer {
    harness::call(router, REGION, "POST", "/connectors", Some(request)).await
}

async fn create_challenge(router: &MultiServiceRouter, request: Value) -> Answer {
    harness::call(router, REGION, "POST", "/challenges", Some(request)).await
}

async fn get_challenge_password(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "GET",
        &format!("/challengePasswords/{}", label(arn)),
        None,
    )
    .await
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

async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(router, REGION, "GET", &format!("/tags/{}", label(arn)), None).await
}

/// Create a connector and make the `ACTIVE` the clock owes it due, so it can mint challenges.
async fn active_connector(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    request: Value,
) -> String {
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

/// Which of this service's two taggable resources a row tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Connector,
    Challenge,
}

/// One row: a resource kind and the `Tags` the create carried (`None` omits the member).
#[derive(Debug)]
struct Case {
    name: &'static str,
    kind: Kind,
    tags: Option<Vec<(String, String)>>,
}

fn pairs(entries: &[(&str, &str)]) -> Option<Vec<(String, String)>> {
    Some(
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    )
}

/// The rows, differing along resource kind, tag count (none at all, one, a handful, and the
/// whole per-resource ceiling) and the characters the keys and values carry.
fn cases() -> Vec<Case> {
    let mut cases = vec![
        Case {
            name: "connector created without a Tags member",
            kind: Kind::Connector,
            tags: None,
        },
        Case {
            name: "connector with the single tag the operation's own doc describes",
            kind: Kind::Connector,
            tags: pairs(&[("customer", "Example Corp")]),
        },
        Case {
            name: "connector with an empty value, a long key and non-ASCII text",
            kind: Kind::Connector,
            tags: pairs(&[
                ("environment", "production"),
                ("owner", ""),
                (
                    "a-key-that-is-quite-long-but-still-a-single-tag-key-0123456789",
                    "équipe sécurité",
                ),
                ("cost:center", "4711"),
            ]),
        },
        Case {
            name: "challenge created without a Tags member",
            kind: Kind::Challenge,
            tags: None,
        },
        Case {
            name: "challenge with one tag",
            kind: Kind::Challenge,
            tags: pairs(&[("purpose", "device-enrollment")]),
        },
        Case {
            name: "challenge with keys supplied in reverse order",
            kind: Kind::Challenge,
            tags: pairs(&[("zone", "c"), ("stage", "b"), ("app", "a")]),
        },
    ];
    // The ceiling itself: a resource carrying every tag this service lets one hold.
    for kind in [Kind::Connector, Kind::Challenge] {
        cases.push(Case {
            name: "resource filled to the per-resource tag ceiling",
            kind,
            tags: Some(
                (0..MAX_TAGS)
                    .map(|index| (format!("key-{index:02}"), format!("value-{index}")))
                    .collect(),
            ),
        });
    }
    cases
}

/// The `Tags` body a row's pairs should come back as.
fn expected_tags(tags: Option<&Vec<(String, String)>>) -> Map<String, Value> {
    tags.into_iter()
        .flatten()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect()
}

/// A `Tags` member for a create request, or no member at all.
fn tags_member(tags: Option<&Vec<(String, String)>>) -> Value {
    match tags {
        Some(pairs) => Value::Object(
            pairs
                .iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect(),
        ),
        None => Value::Null,
    }
}

#[tokio::test]
async fn list_tags_for_resource_reports_the_tags_stored_on_the_named_resource() {
    for case in cases() {
        let (router, engine) = new_router();
        let expected = expected_tags(case.tags.as_ref());

        let arn = match case.kind {
            Kind::Connector => {
                let mut request = json!({ "CertificateAuthorityArn": CA_ARN });
                if let Value::Object(tags) = tags_member(case.tags.as_ref()) {
                    request["Tags"] = Value::Object(tags);
                }
                let created = create_connector(&router, request).await;
                assert_eq!(
                    created.status, 202,
                    "case {:?}: expected 202 from CreateConnector, got {} / {:?}",
                    case.name, created.status, created.body
                );
                created.text("ConnectorArn")
            }
            Kind::Challenge => {
                let connector =
                    active_connector(&router, &engine, json!({ "CertificateAuthorityArn": CA_ARN }))
                        .await;
                let mut request = json!({ "ConnectorArn": connector });
                if let Value::Object(tags) = tags_member(case.tags.as_ref()) {
                    request["Tags"] = Value::Object(tags);
                }
                let created = create_challenge(&router, request).await;
                assert_eq!(
                    created.status, 202,
                    "case {:?}: expected 202 from CreateChallenge, got {} / {:?}",
                    case.name, created.status, created.body
                );
                created.body["Challenge"]["Arn"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {:?}: expected a challenge Arn in {:?}",
                            case.name, created.body
                        )
                    })
                    .to_string()
            }
        };

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected 200 from ListTagsForResource, got {} / {:?}",
            case.name, listed.status, listed.body
        );
        assert_eq!(
            listed.tags(),
            &expected,
            "case {:?}: expected the stored tags {expected:?}, got {:?}",
            case.name,
            listed.tags()
        );

        // Reading is not a mutation: the same ARN answers the same map again, and a
        // connector settling into its ready status does not change what it carries.
        if case.kind == Kind::Connector {
            harness::release_pending_if_owed(&engine, &arn);
        }
        let again = list_tags(&router, &arn).await;
        assert_eq!(
            again.status, 200,
            "case {:?}: expected 200 from a second ListTagsForResource, got {} / {:?}",
            case.name, again.status, again.body
        );
        assert_eq!(
            again.tags(),
            &expected,
            "case {:?}: a read must not change what a later caller sees; expected {expected:?}, got {:?}",
            case.name,
            again.tags()
        );
    }
}

#[tokio::test]
async fn every_resource_reports_its_own_tags() {
    let (router, engine) = new_router();

    // Two connectors and two challenges, each tagged differently, so a listing that answered
    // from anything other than the named record would be caught.
    let first = active_connector(
        &router,
        &engine,
        json!({ "CertificateAuthorityArn": CA_ARN, "Tags": { "which": "first-connector" } }),
    )
    .await;
    let second = active_connector(
        &router,
        &engine,
        json!({ "CertificateAuthorityArn": CA_ARN, "Tags": { "which": "second-connector", "extra": "yes" } }),
    )
    .await;

    let mut expected: Vec<(String, Map<String, Value>)> = Vec::new();
    expected.push((
        first.clone(),
        expected_tags(pairs(&[("which", "first-connector")]).as_ref()),
    ));
    expected.push((
        second.clone(),
        expected_tags(pairs(&[("which", "second-connector"), ("extra", "yes")]).as_ref()),
    ));

    for (connector, which) in [(&first, "first"), (&second, "second")] {
        let created = create_challenge(
            &router,
            json!({ "ConnectorArn": connector, "Tags": { "which": format!("{which}-challenge") } }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {which:?}: expected 202 from CreateChallenge, got {} / {:?}",
            created.status, created.body
        );
        let arn = created.body["Challenge"]["Arn"]
            .as_str()
            .expect("a challenge Arn")
            .to_string();
        expected.push((
            arn,
            expected_tags(pairs(&[("which", &format!("{which}-challenge"))]).as_ref()),
        ));
    }

    for (arn, tags) in &expected {
        let listed = list_tags(&router, arn).await;
        assert_eq!(
            listed.status, 200,
            "case {arn:?}: expected 200 from ListTagsForResource, got {} / {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.tags(),
            tags,
            "case {arn:?}: expected that resource's own tags {tags:?}, got {:?}",
            listed.tags()
        );
    }
}

#[tokio::test]
async fn an_arn_this_service_holds_no_record_under_is_not_found() {
    let (router, engine) = new_router();

    // A connector and a challenge that exist, so the refusals below are about the ARN and
    // not about an empty store.
    let connector = active_connector(
        &router,
        &engine,
        json!({ "CertificateAuthorityArn": CA_ARN, "Tags": { "keep": "me" } }),
    )
    .await;
    let created = create_challenge(
        &router,
        json!({ "ConnectorArn": connector, "Tags": { "keep": "me-too" } }),
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

    // A well-formed ARN of each kind that nothing was ever created under, and an ARN of
    // another service entirely: none of them names a resource this service holds.
    let never_created_connector = format!(
        "arn:aws:pca-connector-scep:{REGION}:{ACCOUNT}:connector/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
    );
    let never_created_challenge =
        format!("{never_created_connector}/challenge/99999999-8888-7777-6666-555544443333");
    let foreign = format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/not-a-scep-resource");

    // The challenge is deleted, and then the connector: each delete takes the tags of what it
    // removed with it, the connector's teardown taking its remaining challenges too.
    let gone_challenge = format!("{connector}/challenge/11111111-1111-1111-1111-111111111111");

    for (name, arn) in [
        (
            "connector ARN nothing was created under",
            never_created_connector.clone(),
        ),
        (
            "challenge ARN nothing was created under",
            never_created_challenge.clone(),
        ),
        ("ARN of another service", foreign.clone()),
        (
            "challenge of a live connector that was never created",
            gone_challenge.clone(),
        ),
    ] {
        let listed = list_tags(&router, &arn).await;
        harness::assert_tagging_not_found(&listed, name);
    }

    // Deleting the challenge drops its tags with it.
    let deleted = delete_challenge(&router, &challenge).await;
    assert_eq!(
        deleted.status, 202,
        "expected 202 from DeleteChallenge, got {} / {:?}",
        deleted.status, deleted.body
    );
    let listed = list_tags(&router, &challenge).await;
    harness::assert_tagging_not_found(&listed, "deleted challenge");

    // A second challenge, to prove the connector's teardown takes its challenges' tags too.
    let created = create_challenge(
        &router,
        json!({ "ConnectorArn": connector, "Tags": { "keep": "until-the-connector-goes" } }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "expected 202 from CreateChallenge, got {} / {:?}",
        created.status, created.body
    );
    let survivor = created.body["Challenge"]["Arn"]
        .as_str()
        .expect("a challenge Arn")
        .to_string();
    assert_eq!(
        get_challenge_password(&router, &survivor).await.status,
        200,
        "the second challenge must be readable before the connector is deleted"
    );

    let deleted = delete_connector(&router, &connector).await;
    assert_eq!(
        deleted.status, 202,
        "expected 202 from DeleteConnector, got {} / {:?}",
        deleted.status, deleted.body
    );
    // While the teardown is still owed, the connector is reported with its tags.
    if harness::hold_pending_if_owed(&engine, &connector) {
        let listed = list_tags(&router, &connector).await;
        assert_eq!(
            listed.status, 200,
            "case \"connector whose teardown has not come due\": expected 200, got {} / {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.tags(),
            &expected_tags(pairs(&[("keep", "me")]).as_ref()),
            "case \"connector whose teardown has not come due\": expected its stored tags, got {:?}",
            listed.tags()
        );
    }

    // Once it comes due the record is gone, and so are its tags and its challenges'.
    harness::release_pending_if_owed(&engine, &connector);
    for (name, arn) in [
        ("deleted connector", connector.as_str()),
        ("challenge of a deleted connector", survivor.as_str()),
    ] {
        let listed = list_tags(&router, arn).await;
        harness::assert_tagging_not_found(&listed, name);
    }
}
