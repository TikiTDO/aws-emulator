// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for a tag WRITE aimed at a resource whose teardown has been accepted.
//!
//! The rule under test: a connector whose `DeleteConnector` has been accepted is a record this
//! service only REPORTS until its teardown completes, so a call that would write new state
//! onto it — or onto a challenge hanging off it, which the teardown takes with it — is refused
//! 400 `ValidationException` with `Reason` `INVALID_STATE` and changes nothing, while every
//! reporting call keeps answering for the same record. A connector that has NOT been accepted
//! for teardown, `CREATING` included, is still writable, and once the teardown comes due the
//! record is gone and the write is the tagging layer's own not-found refusal instead.
//!
//! `ValidationExceptionReason` carries `INVALID_STATE` for exactly a call a resource's status
//! does not allow (smithy_query: shape ValidationExceptionReason), while the tagging endpoints
//! report an ARN that resolves to no resource with the tag subsystem's own 400
//! `NotFoundException` ([`harness::assert_tagging_not_found`]), so the two windows answer
//! differently.
//!
//! The rows differ along what the rule generalises over: which of the two taggable resource
//! kinds the write names, the status the connector was in when its delete was accepted, how
//! many tags the resource already carried, and whether the request would add keys, overwrite
//! one, or merge an empty map that would change nothing at all.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};
use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep as scep;

/// A Region that is not the emulator default, so every ARN comes from the request's own SigV4
/// credential scope.
const REGION: &str = "eu-west-1";

/// A private-CA ARN matching `CertificateAuthorityArn`'s own pattern, in this Region.
const CA_ARN: &str = "arn:aws:acm-pca:eu-west-1:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555";

/// The service's own status and reason vocabulary, named from its constants so a case cannot
/// drift off them.
const STATUS_ACTIVE: &str = scep::state::STATUS_ACTIVE;
const STATUS_CREATING: &str = scep::state::STATUS_CREATING;
const STATUS_DELETING: &str = scep::state::STATUS_DELETING;
const REASON_INVALID_STATE: &str = scep::state::REASON_INVALID_STATE;

/// The tag every row writes BEFORE the delete is accepted, so each row proves the same call
/// was accepted on the same resource until the teardown was asked for.
const PROBE: (&str, &str) = ("before-teardown", "accepted");

/// Which of this service's two taggable resources a row names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Connector,
    Challenge,
}

/// The status the connector is in when its `DeleteConnector` is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeleteFrom {
    Active,
    Creating,
}

#[derive(Debug)]
struct Case {
    name: &'static str,
    kind: Kind,
    delete_from: DeleteFrom,
    /// The tags the create stored on the resource the row tags.
    initial: &'static [(&'static str, &'static str)],
    /// The entries the refused call sends.
    add: &'static [(&'static str, &'static str)],
}

/// The rows: both resource kinds, both statuses a delete can be accepted from, resources that
/// carried no tags and resources that carried several, and requests that would add keys,
/// overwrite a key that is already there, or merge nothing at all.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a first tag on a connector with none",
            kind: Kind::Connector,
            delete_from: DeleteFrom::Active,
            initial: &[],
            add: &[("owner", "platform")],
        },
        Case {
            name: "a new value for a key the connector already carries",
            kind: Kind::Connector,
            delete_from: DeleteFrom::Active,
            initial: &[("environment", "staging"), ("owner", "platform")],
            add: &[("environment", "production")],
        },
        Case {
            name: "an empty Tags map, which would change nothing",
            kind: Kind::Connector,
            delete_from: DeleteFrom::Active,
            initial: &[("keep", "me")],
            add: &[],
        },
        Case {
            name: "three keys on a connector torn down from CREATING",
            kind: Kind::Connector,
            delete_from: DeleteFrom::Creating,
            initial: &[("keep", "me")],
            add: &[("a", "1"), ("b", "2"), ("c", "3")],
        },
        Case {
            name: "a first tag on a challenge of the connector being torn down",
            kind: Kind::Challenge,
            delete_from: DeleteFrom::Active,
            initial: &[],
            add: &[("owner", "platform")],
        },
        Case {
            name: "one key added and one overwritten on such a challenge",
            kind: Kind::Challenge,
            delete_from: DeleteFrom::Active,
            initial: &[("stage", "one"), ("zone", "a")],
            add: &[("zone", "b"), ("app", "checkout")],
        },
    ]
}

async fn create_connector(router: &MultiServiceRouter, tags: &Value) -> Answer {
    harness::call(
        router,
        REGION,
        "POST",
        "/connectors",
        Some(json!({ "CertificateAuthorityArn": CA_ARN, "Tags": tags })),
    )
    .await
}

async fn create_challenge(router: &MultiServiceRouter, connector: &str, tags: &Value) -> Answer {
    harness::call(
        router,
        REGION,
        "POST",
        "/challenges",
        Some(json!({ "ConnectorArn": connector, "Tags": tags })),
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

/// The `UpdatedAt` the row's resource reports — the instant the record last changed, which an
/// accepted tag write moves and a refused one must leave alone — read through whichever reader
/// the resource's kind has, both of which keep answering for a record under teardown.
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

async fn tag_resource(
    router: &MultiServiceRouter,
    arn: &str,
    tags: &[(&str, &str)],
) -> Answer {
    harness::call(
        router,
        REGION,
        "POST",
        &format!("/tags/{}", label(arn)),
        Some(json!({ "Tags": tags_member(tags) })),
    )
    .await
}

/// A `Tags` member for a request body.
fn tags_member(tags: &[(&str, &str)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect(),
    )
}

/// The `Tags` body a map should come back as from a listing.
fn expected_body(tags: &BTreeMap<String, String>) -> Map<String, Value> {
    tags.iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect()
}

/// What the row's resource carries once the accepted probe has landed: its initial tags plus
/// the probe, and nothing from the refused call.
fn expected_tags(case: &Case) -> BTreeMap<String, String> {
    let mut tags: BTreeMap<String, String> = case
        .initial
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    tags.insert(PROBE.0.to_string(), PROBE.1.to_string());
    tags
}

/// Stand the row's resource up and answer `(connector ARN, the ARN the row tags)`.
///
/// A challenge can only be minted by an `ACTIVE` connector, so a challenge row's connector is
/// settled first; a connector row's connector is left `CREATING` when that is the status its
/// delete is to be accepted from.
async fn resources_of(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &Case,
) -> (String, String) {
    let name = case.name;
    let connector_tags = match case.kind {
        Kind::Connector => tags_member(case.initial),
        Kind::Challenge => tags_member(&[]),
    };
    let created = create_connector(router, &connector_tags).await;
    assert_eq!(
        created.status, 202,
        "case {name:?}: expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    let connector = created.text("ConnectorArn");

    if case.kind == Kind::Challenge || case.delete_from == DeleteFrom::Active {
        harness::release_pending(engine, &connector);
    } else {
        // The row's delete is accepted from `CREATING`, so hold that transition rather than
        // race the service's own transition span.
        harness::hold_pending(engine, &connector);
    }

    let resource = match case.kind {
        Kind::Connector => connector.clone(),
        Kind::Challenge => {
            let created = create_challenge(router, &connector, &tags_member(case.initial)).await;
            assert_eq!(
                created.status, 202,
                "case {name:?}: expected 202 from CreateChallenge, got {} / {:?}",
                created.status, created.body
            );
            created.challenge("Arn")
                .as_str()
                .unwrap_or_else(|| panic!("case {name:?}: expected a challenge Arn in {:?}", created.body))
                .to_string()
        }
    };
    (connector, resource)
}

#[tokio::test]
async fn tagging_a_resource_whose_teardown_was_accepted_is_refused_for_its_state() {
    for case in cases() {
        let name = case.name;
        let (router, engine) = new_router();
        let (connector, resource) = resources_of(&router, &engine, &case).await;

        // Until the delete is asked for, the very same call is accepted — on an `ACTIVE`
        // connector and equally on one still `CREATING`, which is a status a write is allowed
        // in: the refusal below is about the teardown, not about the row's entries.
        let expected_status_before = match case.delete_from {
            DeleteFrom::Active => STATUS_ACTIVE,
            DeleteFrom::Creating => STATUS_CREATING,
        };
        let read = get_connector(&router, &connector).await;
        assert_eq!(
            read.connector("Status"),
            &json!(expected_status_before),
            "case {name:?}: expected the connector to report {expected_status_before} before the \
             delete, got {:?}",
            read.body
        );
        let probed = tag_resource(&router, &resource, &[PROBE]).await;
        assert_eq!(
            probed.status, 204,
            "case {name:?}: a write before the delete must be accepted, got {} / {:?}",
            probed.status, probed.body
        );

        let deleted = delete_connector(&router, &connector).await;
        assert_eq!(
            deleted.status, 202,
            "case {name:?}: expected 202 from DeleteConnector, got {} / {:?}",
            deleted.status, deleted.body
        );
        // From here the case is about the teardown window, so pin it.
        harness::hold_pending(&engine, &connector);
        let read = get_connector(&router, &connector).await;
        assert_eq!(
            read.connector("Status"),
            &json!(STATUS_DELETING),
            "case {name:?}: expected the connector to report {STATUS_DELETING}, got {:?}",
            read.body
        );

        // The write the rule refuses, read first for the instant the record reports as its
        // last change: a refused write moves neither the tags nor that instant.
        let updated_before = updated_at(&router, case.kind, &resource).await;
        let refused = tag_resource(&router, &resource, case.add).await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (400, "ValidationException"),
            "case {name:?}: expected 400 ValidationException for a write onto a \
             {STATUS_DELETING} connector, got {} / {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.body["Reason"],
            json!(REASON_INVALID_STATE),
            "case {name:?}: expected Reason {REASON_INVALID_STATE}, got {:?}",
            refused.body
        );
        let message = refused.message().to_string();
        assert!(
            message.contains(&connector) && message.contains(STATUS_DELETING),
            "case {name:?}: expected a Message naming {connector:?} and {STATUS_DELETING}, got \
             {message:?}"
        );
        if case.kind == Kind::Challenge {
            assert!(
                message.contains(&resource),
                "case {name:?}: expected a Message naming the challenge {resource:?}, got \
                 {message:?}"
            );
        }

        // Nothing was written, and the record is still REPORTED with what it carried.
        let expected = expected_body(&expected_tags(&case));
        let listed = list_tags(&router, &resource).await;
        assert_eq!(
            listed.status, 200,
            "case {name:?}: a resource under teardown is still reported, got {} / {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.tags(),
            &expected,
            "case {name:?}: expected the refused call to leave {expected:?}, got {:?}",
            listed.tags()
        );

        let updated_after = updated_at(&router, case.kind, &resource).await;
        assert_eq!(
            updated_after, updated_before,
            "case {name:?}: a refused write must leave the record's UpdatedAt at \
             {updated_before}, got {updated_after}"
        );

        // Repeating it answers the same way and still writes nothing.
        let again = tag_resource(&router, &resource, case.add).await;
        assert_eq!(
            (again.status, again.error_type.as_str()),
            (400, "ValidationException"),
            "case {name:?}: expected the repeated write to be refused the same way, got {} / {:?}",
            again.status,
            again.body
        );
        assert_eq!(
            list_tags(&router, &resource).await.tags(),
            &expected,
            "case {name:?}: repeating the refused write must leave {expected:?}",
        );

        // Once the teardown comes due the record is gone, so the same write is the tagging
        // layer's not-found refusal instead of the state refusal.
        harness::release_pending(&engine, &connector);
        let gone = tag_resource(&router, &resource, case.add).await;
        harness::assert_tagging_not_found(&gone, name);
    }
}

#[tokio::test]
async fn one_connectors_teardown_leaves_every_other_resource_writable() {
    let (router, engine) = new_router();

    // Two connectors, each with a challenge: only the first is torn down.
    let mut arns = Vec::new();
    for which in ["first", "second"] {
        let created = create_connector(&router, &tags_member(&[("which", which)])).await;
        assert_eq!(
            created.status, 202,
            "expected 202 from CreateConnector, got {} / {:?}",
            created.status, created.body
        );
        let connector = created.text("ConnectorArn");
        harness::release_pending(&engine, &connector);
        let challenge = create_challenge(&router, &connector, &tags_member(&[("which", which)]))
            .await
            .challenge("Arn")
            .as_str()
            .expect("a challenge Arn")
            .to_string();
        arns.push((connector, challenge));
    }
    let (doomed, doomed_challenge) = arns[0].clone();
    let (survivor, survivor_challenge) = arns[1].clone();

    assert_eq!(
        delete_connector(&router, &doomed).await.status,
        202,
        "expected 202 from DeleteConnector"
    );
    harness::hold_pending(&engine, &doomed);

    for (name, arn, accepted) in [
        ("the connector being torn down", doomed.as_str(), false),
        (
            "a challenge of the connector being torn down",
            doomed_challenge.as_str(),
            false,
        ),
        ("another connector", survivor.as_str(), true),
        (
            "a challenge of another connector",
            survivor_challenge.as_str(),
            true,
        ),
    ] {
        let tagged = tag_resource(&router, arn, &[("owner", "platform")]).await;
        assert_eq!(
            tagged.status,
            if accepted { 204 } else { 400 },
            "case {name:?}: expected the write to be {}, got {} / {:?}",
            if accepted { "accepted" } else { "refused" },
            tagged.status,
            tagged.body
        );
        let expected: BTreeMap<String, String> = if accepted {
            [("owner", "platform"), ("which", "second")]
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect()
        } else {
            [("which", "first")]
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect()
        };
        let expected = expected_body(&expected);
        let listed = list_tags(&router, arn).await;
        assert_eq!(
            listed.tags(),
            &expected,
            "case {name:?}: expected {expected:?}, got {:?}",
            listed.tags()
        );
    }
}
